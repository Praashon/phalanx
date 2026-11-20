# Phalanx Architecture Guide

Phalanx is organized as a five-crate Rust workspace with strict dependency boundaries: zero external runtime dependencies in the VM, compiler, and simulation crates (`phalanx-vm`, `phalanx-lang`, `phalanx-sim`), and `#![forbid(unsafe_code)]` across all four core engine crates.

```
   .phx source ──▶ phalanx-lang ──▶ Program ──▶ phalanx-vm::verify() ──▶ VerifiedProgram
                   (Lexer, Pratt,               (CFG abstract interp,     (Typestate token)
                    Typecheck, IR,               call-graph DAG check,           │
                    Opt, RegAlloc)               CRC32C `.phxb` codec)           ▼
                                                                           VmInstance (0 allocs)
                                                                                 │
   Ratatui TUI ◀── FrameSnapshot ◀── phalanx-sim ◀──── 45 Host Syscalls ─────────┘
   & .phxr Replay                    (Q16.16 Physics, SpatialHashGrid,
                                      LOS Raycasting, Simultaneous Intents,
                                      FNV-1a Per-Tick State Hashing)
                                                │
                                                ▼
                                         phalanx-tourney
                                         (64B-Aligned Work-Stealing Deques,
                                          Deterministic Elo Reduction, Audit)
```

## 1. Compiler Pipeline (`crates/phalanx-lang`)

1. **Lexer (`lexer.rs`)**: Hand-written scanner parsing decimal, hexadecimal (`0x..`), and exact Q16.16 decimal literals (`fix`) without floating-point rounding drift.
2. **Parser (`parser.rs`)**: Recursive-descent module/statement parser paired with a Pratt precedence expression parser supporting `const`, scalar `state`, fixed-size `state` arrays `[T; N]`, `if/else`, `while`, `for i in a..b`, `break`, `continue`, and short-circuiting `&&`/`||`.
3. **Type Checker (`typecheck.rs`)**: Constant-evaluates `const` and `state` initializers, rejects implicit `i32`/`fix` coercions (`to_fix`/`to_int` required), enforces `mut` immutability, validates all 45 syscall signatures against `phalanx_vm::SYSCALLS`, and formats `rustc`-style span diagnostics via `SourceMap`.
4. **Basic-Block IR & Optimizer (`ir.rs`, `opt.rs`)**: Lowers AST into typed CFG basic blocks. When `OptLevel::Standard` is enabled, runs fixed-point passes for single-def constant folding, algebraic simplification, constant branch folding, copy propagation, unreachable block pruning, and trapping-aware dead code elimination.
5. **Loop-Aware Register Allocator (`codegen.rs`)**: Orders blocks in Reverse Post-Order (RPO), solves backward dataflow liveness (`live_in`/`live_out`) so loop-carried virtual registers stay live across back-edges, runs linear scan over `0..(MAX_REGS - max_call_arity)`, and stages call/syscall arguments into the top register window.

## 2. Bytecode VM & Static Verifier (`crates/phalanx-vm`)

- **32-bit Register ISA (`isa.rs`)**: 50 opcodes (`0..=49`) operating over `32` typed `i32` registers per frame (`MAX_CALL_DEPTH = 16`, `MAX_STATE_SLOTS = 64`, `MAX_STATE_ARRAYS = 16`, `MAX_INSTRUCTIONS = 8192`).
- **Abstract-Interpretation Verifier (`verifier.rs`)**: Performs monotone fixpoint analysis over `RegState::{Uninit, Typed(ValType)}` per instruction. At control-flow join points, registers with differing incoming types demote to `Uninit`. Verifies bounded jumps, complete return paths, call-graph acyclicity, and stack depth bounds before returning `VerifiedProgram`.
- **Zero-Allocation Execution (`vm.rs`)**: `VmInstance` embeds fixed arrays `[Frame; 16]` and `[i32; 64]`. Single-stepping (`step`) powers the interactive TUI bytecode debugger while `run_tick` enforces per-tick gas budgets.

## 3. Deterministic Arena Simulation (`crates/phalanx-sim`)

- **Bit-Exact Math (`fixed.rs`, `rng.rs`)**: Pure-integer Q16.16 arithmetic and `Xoshiro256**` per-bot PRNG streams derived via SplitMix64.
- **Zero-Alloc Spatial Hash (`spatial.rs`)**: Pre-allocated intrusive linked-list buckets (`cell_heads` + `next`) supporting `O(k)` cone scans and projectile queries.
- **Simultaneous Tick Pipeline (`world.rs`)**:
  1. Snapshot start-of-tick world state and rebuild bot/projectile spatial grids.
  2. Execute every alive bot's `VmInstance` against the immutable snapshot (`BotHostView`), buffering movement, dash, shield, and fire intents.
  3. Apply all intents simultaneously (preventing slot-index turn-order bias).
  4. Integrate velocities/drag, resolve wall, pillar (`arena.rs`), and bot collisions, step projectiles with segment-AABB occlusion, apply shrinking storm ring damage, and compute a 64-bit FNV-1a `state_hash`.
- **Self-Contained Replays (`replay.rs`)**: `.phxr` containers bundle arena configuration, embedded `.phxb` bytecodes, and every tick's 64-bit hash under a Castagnoli CRC32C checksum. `verify_replay` re-simulates from scratch and verifies every tick hash.

## 4. Lock-Free Work-Stealing Tournament Engine (`crates/phalanx-tourney`)

- **`WorkShard` (`pool.rs`)**: Each worker owns a cache-line-aligned (`#[repr(align(64))]`) 64-bit atomic `(head: u32, tail: u32)` slice descriptor. Owners pop from `head` and idle workers steal from `tail` using lock-free CAS over `std::thread::scope`, with zero mutexes and zero `unsafe`.
- **Deterministic Reduction (`elo.rs`, `scheduler.rs`)**: Completed match records are sorted by `match_idx` prior to Elo updates, making standings invariant to thread count (`--threads 1` vs `--threads N`). Optional `--audit-determinism` executes every match twice across worker shards and verifies 100% hash equality.
