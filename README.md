# Phalanx

**Sandboxed Autonomous Bot Battle Arena — Custom Typed Language, Static Bytecode Verifier, Zero-Allocation Register VM & Bit-Exact Deterministic Physics Engine in Rust**

Write autonomous combat AIs in `.phx`, compile them through an optimizing SSA-style CFG compiler, prove memory/control-flow safety statically before execution (inspired by Linux eBPF), and watch them battle inside a deterministic Q16.16 fixed-point arena with a live terminal visualizer, time-travel tick scrubbing, and a step-through VM bytecode debugger.

```
  .phx source ──▶ phalanx-lang ──▶ phalanx-vm::verify() ──▶ VerifiedProgram (typestate)
                   (Pratt, Type,   (Abstract interp, CFG        │
                    Opt, RegAlloc)  bounds, DAG call-graph)     ▼
                                                          VmInstance (0 heap allocs/tick)
                                                                │
  Ratatui TUI ◀── FrameSnapshot ◀── phalanx-sim ◀── 45 Host Syscalls
  & .phxr Replay                    (Q16.16 physics, SpatialHashGrid,
                                     LOS raycast, FNV-1a tick hashes)
                                           │
                                           ▼
                                    phalanx-tourney
                                    (64B-aligned CAS work-stealing pool,
                                     Deterministic Elo & cross-thread audit)
```

---

## Why Phalanx?

Phalanx explores what happens when **sandboxed language runtimes**, **deterministic simulation**, and **lock-free parallel systems** meet in one cohesive codebase:

- **Custom Typed Language (`.phx`) & Optimizing Compiler (`phalanx-lang`)**: Hand-written lexer (pure-integer decimal-to-Q16.16 parsing with zero float drift), Pratt parser, strict `i32`/`fix`/`bool` type checker with `rustc`-style caret diagnostics, basic-block CFG optimizer (constant folding, algebraic identities, copy propagation, dead block & pure DCE), and backward-dataflow loop-aware linear-scan register allocation.
- **eBPF-Inspired Static Bytecode Verifier (`phalanx-vm`)**: Enforces a Rust typestate security boundary (`VerifiedProgram` can only be produced by `verify()`). Runs monotone abstract interpretation over `RegState::{Uninit, Typed(ValType)}` across control-flow merges, proves all branch targets remain within function bounds, bans cycles via static call-graph DAG verification, and enforces `MAX_CALL_DEPTH = 16`.
- **Zero-Allocation 32-Bit RISC Register VM (`phalanx-vm`)**: Fixed-width 32-bit instructions (`50` opcodes, `32` registers/frame, inline `[Frame; 16]` and `[i32; 64]` persistent state). Proven **0 heap allocations per tick** via custom `#[global_allocator]` testing ([zero_alloc.rs](file:///q:/humor-getting/crates/phalanx-vm/tests/zero_alloc.rs)).
- **Bit-Exact Q16.16 Fixed-Point Physics (`phalanx-sim`)**: Zero IEEE-754 floating-point instructions in physics, trigonometry (`const` Q32.32-generated sine LUT + octant-reduced `atan2` + binary `isqrt`), or `Xoshiro256**` PRNG streams. Uses a pre-allocated intrusive `SpatialHashGrid`, slab ray-box cover occlusion, simultaneous intent resolution, and per-tick 64-bit FNV-1a world state hashing.
- **Self-Contained Verified Replays (`.phxr`)**: Bundles arena config, embedded `.phxb` bytecodes, and every tick's 64-bit hash under Castagnoli `CRC32C`. `phalanx replay <file> --verify` re-simulates from scratch and validates every single tick hash.
- **Lock-Free Work-Stealing Tournament Engine (`phalanx-tourney`)**: `#[repr(align(64))]` cache-line-aligned packed 64-bit `(head, tail)` atomic work-stealing shards over `std::thread::scope` (`#![forbid(unsafe_code)]`), deterministic `match_idx`-ordered Elo updates, and cross-thread determinism auditing.
- **Interactive Browser Studio (`phalanx serve`, default) & Terminal TUI (`phalanx watch`)**: Embedded zero-dependency Rust HTTP server (`std::net::TcpListener`) serving a self-contained 60 FPS HTML5 `<canvas>` Battle Arena, **Bot Lab** live `.phx` code editor + static verifier inspector, **Step-Through VM Debugger**, and **1-Click Multi-Core Tournament** dashboard at `http://127.0.0.1:8080` — plus a Ratatui terminal UI for headless/SSH workflows.

---

## Workspace Architecture

| Crate | Purpose | Runtime Deps | `unsafe` |
| :--- | :--- | :--- | :--- |
| [phalanx-vm](file:///q:/humor-getting/crates/phalanx-vm/src/lib.rs) | 32-bit ISA, 45 syscalls, `.phxb` CRC32C codec, disassembler, static verifier, 0-alloc VM | **0 external** | `#![forbid(unsafe_code)]` |
| [phalanx-lang](file:///q:/humor-getting/crates/phalanx-lang/src/lib.rs) | `.phx` lexer, Pratt parser, type checker, CFG IR, optimizer, register allocator | `phalanx-vm` only | `#![forbid(unsafe_code)]` |
| [phalanx-sim](file:///q:/humor-getting/crates/phalanx-sim/src/lib.rs) | Q16.16 math/trig, `SpatialHashGrid`, LOS raycasting, physics arena, `.phxr` replays | `phalanx-vm` only | `#![forbid(unsafe_code)]` |
| [phalanx-tourney](file:///q:/humor-getting/crates/phalanx-tourney/src/lib.rs) | Lock-free CAS work-stealing pool, Elo engine, multi-core determinism audit | `phalanx-*`, `serde` | `#![forbid(unsafe_code)]` |
| [phalanx-cli](file:///q:/humor-getting/crates/phalanx-cli/src/main.rs) | `phalanx` binary (`serve` Web Studio, `watch`, `compile`, `verify`, `replay`, `tourney`) | `clap`, `ratatui`, `crossterm`, `serde_json` | Safe Rust |

Deep-dive docs: [Architecture Guide](file:///q:/humor-getting/docs/ARCHITECTURE.md) | [ADR-001 Fixed-Point Determinism](file:///q:/humor-getting/docs/adr/ADR-001-fixed-point-determinism.md) | [ADR-002 Sandboxing & Verifier](file:///q:/humor-getting/docs/adr/ADR-002-defense-in-depth-sandboxing.md) | [ADR-003 Register ISA & 0-Alloc VM](file:///q:/humor-getting/docs/adr/ADR-003-register-isa-and-zero-alloc-vm.md)

---

## Quickstart

### 1. Launch the Interactive Web Studio (Recommended)

```bash
cargo run --release
```

Then open **http://127.0.0.1:8080** in your browser:
- **Battle Arena**: Watch all 5 bots fight on a 60 FPS HTML5 Canvas, scrub backward/forward through time with the timeline slider, inspect live gas/HP/energy meters, verify the per-tick deterministic 64-bit hash, and single-step VM bytecode instructions.
- **Bot Lab**: Edit `.phx` bot AI code in the browser, inspect the eBPF-style verifier certificate, optimized CFG IR, and VM disassembly, and click **Compile & Fight in Arena!** to test your bot live.
- **Multi-Core Tournament**: Run 100+ matches across CPU cores with 1 click and inspect the live Elo leaderboard, VM MIPS, and cross-thread determinism audit.

### 2. Terminal CLI Commands

```bash
# Watch all 5 built-in champion bots live in the Ratatui terminal TUI
cargo run --release -- watch

# Run headless and save a verifiable deterministic replay
cargo run --release -- watch --headless --seed 42 --save-replay demo.phxr

# Re-simulate every tick and verify every 64-bit world hash
cargo run --release -- replay demo.phxr --verify

# Inspect compiler IR, VM disassembly, and static verifier report
cargo run --release -- compile bots/kiter.phx --dump-ir --disasm --verify-report

# Run a parallel multi-core Elo tournament + cross-thread determinism audit
cargo run --release -- tourney --rounds 10 --audit-determinism
```

### Interactive TUI Controls

| Key | Action |
| :--- | :--- |
| `Space` | Play / Pause simulation |
| `<-` / `->` | Time-travel scrub backward / step 1 tick forward |
| `d` | Toggle **Live VM Bytecode & Register Debugger** |
| `s` | Single-step 1 VM instruction on the selected bot |
| `Tab` | Cycle inspected bot |
| `+` / `-` | Adjust playback speed (`1x` – `8x`) |
| `r` / `R` | Restart with next seed / same seed |
| `q` / `Esc` | Quit cleanly |

---

## Writing a Bot in `.phx`

```rust
// Excerpt from bots/kiter.phx
const PROJ_SPEED_MED: fix = 1.85;
state strafe_dir: i32 = 1;

fn lead_angle(tid: i32, proj_speed: fix) -> fix {
    let t = target_dist(tid) / proj_speed;
    let px = target_x(tid) + target_vx(tid) * t;
    let py = target_y(tid) + target_vy(tid) * t;
    return atan2(py - self_y(), px - self_x());
}

fn tick() {
    let bid = nearest_bullet();
    if bid >= 0 && bullet_dist(bid) < 13.0 {
        let dodge = atan2(bullet_vy(bid), bullet_vx(bid)) + HALF_PI * to_fix(strafe_dir);
        let _d = dash(dodge);
        thrust(dodge, 1.0);
    }

    let tid = radar_nearest();
    if tid >= 0 && self_energy() >= 24 {
        let _f = fire(lead_angle(tid, PROJ_SPEED_MED), 2);
    }
}
```

Built-in showcase bots in [bots/](file:///q:/humor-getting/bots): `kiter`, `berserker`, `sentry`, `vanguard`, `ghost`, plus [rogue_loop.phx](file:///q:/humor-getting/bots/examples/rogue_loop.phx) (demonstrating deterministic gas-ceiling containment on an infinite loop).

---

## Measured Performance

Measured in `--release` on Windows (`rustc 1.99.1`, 16 worker threads for tournament run):

| Benchmark / Workload | Measured Result | Source |
| :--- | :--- | :--- |
| `vm_250_iter_loop_1000_insns` (Criterion) | **8.43 µs** mean (`[8.07 µs .. 8.84 µs]`) | [vm_bench.rs](file:///q:/humor-getting/crates/phalanx-vm/benches/vm_bench.rs) |
| `sim_5bot_arena_step_tick` (Criterion, 5 bots + physics + grids + hash) | **3.97 µs / tick** (`[3.78 µs .. 4.14 µs]`) | [sim_bench.rs](file:///q:/humor-getting/crates/phalanx-sim/benches/sim_bench.rs) |
| 10-round 1v1 Tournament + 100% Audit (`200` total matches, 16 threads) | **0.053 s** (**3,794.5 matches/s**, **257.40 VM MIPS**) | `phalanx tourney --rounds 10 --audit-determinism` |
| VM heap allocations per tick (`1,000` ticks) | **0 allocations** | [zero_alloc.rs](file:///q:/humor-getting/crates/phalanx-vm/tests/zero_alloc.rs) |

---

## Testing & Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo bench -p phalanx-vm -p phalanx-sim
```

The test suite includes adversarial raw-bytecode verification tests, `proptest` bytecode/decoder fuzzing, compiler optimization equivalence fuzzing (`OptLevel::None` vs `OptLevel::Standard`), spatial-hash vs $O(N^2)$ brute-force oracle checks, custom `GlobalAlloc` zero-allocation proofs, replay tamper detection, and multi-thread tournament invariance checks.
