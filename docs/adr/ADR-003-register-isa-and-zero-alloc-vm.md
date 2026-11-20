# ADR-003: Fixed-Width 32-Bit Register ISA & Zero-Allocation VM Execution

## Status
Accepted

## Context
High-throughput Elo tournaments simulate tens of thousands of bot ticks per second across worker threads. Stack-based bytecode VMs incur extra push/pop dispatch overhead, while heap-allocated call stacks or register files create allocator contention across threads.

## Decision
1. **Fixed-width 32-bit RISC register encoding (`Instruction(u32)`)**:
   - Format A: `[op:8][a:8][b:8][c:8]` for 3-register ALU, comparisons, state arrays, calls, and syscalls.
   - Format B: `[op:8][a:8][imm16:16]` for signed 16-bit immediates, relative branch offsets, constants, and scalar state slots.
   - Typed ALU opcodes (`AddI` vs `AddFix`, `LtI` vs `LtFix`, `EqBool`) eliminate runtime value tagging inside registers: every register holds a dense `i32`.
2. **Pre-allocated flat VM state (`VmInstance`)**:
   - `frames: [Frame; MAX_CALL_DEPTH]` (`16 * Frame`) and `state: [i32; MAX_STATE_SLOTS]` live inline inside `VmInstance`.
   - `begin_tick`, `step`, and `run_tick` perform **zero heap allocations**, verified in CI by a custom `#[global_allocator]` integration test (`crates/phalanx-vm/tests/zero_alloc.rs`).
3. **Reserved call staging window in register allocation**: `phalanx-lang`'s linear-scan allocator uses `0..(MAX_REGS - max_call_arity)` for live variables and maps call/syscall arguments into the top window `arg_base..MAX_REGS` immediately prior to invocation, guaranteeing contiguous `r[arg_base..arg_base + arity]` without spilling caller locals.

## Consequences
- Single-word instruction decode via bit-shifts with zero heap churn during simulation ticks.
- Scales linearly across CPU cores in `phalanx-tourney` without `malloc`/allocator lock contention.
