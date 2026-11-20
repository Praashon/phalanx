# ADR-002: Defense-in-Depth Bot Sandboxing & Typestate Verification Boundary

## Status
Accepted

## Context
Phalanx executes user-written autonomous bot programs inside a shared multi-threaded arena tournament engine. Untrusted or adversarial `.phxb` bytecode must never crash the host process, read uninitialized stack/register memory, escape array bounds, infinitely recurse, hang a worker thread, or pass mismatched value types across control-flow merges.

## Decision
1. **Rust typestate boundary (`VerifiedProgram`)**: `phalanx_vm::VmInstance` refuses raw `Program` instances; it only accepts `VerifiedProgram`, whose constructor is private to `phalanx_vm::verifier::verify`.
2. **eBPF-style abstract-interpretation verifier**:
   - Enforces structural caps (`MAX_REGS = 32`, `MAX_CALL_DEPTH = 16`, `MAX_STATE_SLOTS = 64`, `MAX_STATE_ARRAYS = 16`, `MAX_FUNCTIONS = 64`, `MAX_INSTRUCTIONS = 8192`).
   - Runs a monotone worklist abstract interpreter per function tracking `RegState::Uninit | RegState::Typed(ValType)` across all reachable CFG edges. On join, disagreeing paths demote the register to `Uninit`, rejecting any subsequent read without re-initialization.
   - Verifies every jump target remains strictly inside `[entry_pc, entry_pc + len)`, rejects paths falling off the end of a function without `ret`/`trap`, enforces exact syscall/function call signatures, and proves acyclicity and bounded depth (`<= MAX_CALL_DEPTH`) on the static call graph.
3. **Deterministic runtime traps & gas metering**: Runtime operations that remain dynamically fallible (`DivI`/`ModI`/`DivFix` by zero, `LoadStateArr`/`StoreStateArr` out-of-bounds) trap cleanly via `ExecStatus::Trapped(TrapReason)` while an instruction-level gas budget enforces bounded execution (`ExecStatus::OutOfGas`).
4. **Memory safety & container integrity**: `phalanx-vm`, `phalanx-lang`, `phalanx-sim`, and `phalanx-tourney` all enforce `#![forbid(unsafe_code)]`, and both `.phxb` and `.phxr` binary containers carry Castagnoli CRC32C footers checked before parsing.

## Consequences
- Adversarial or fuzzed `.phxb` files are rejected statically before a single VM instruction executes, and verified programs are guaranteed panic-free under `VmInstance::run_tick`.
- Direct recursion in `.phx` is disallowed statically in favor of bounded iteration (`while` / `for`) and acyclic helper calls.
