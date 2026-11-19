#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Phalanx Virtual Machine (`phalanx-vm`).
//!
//! Provides a fixed-width 32-bit RISC register ISA, Castagnoli CRC32C integrity
//! checks, an eBPF-inspired static abstract-interpretation bytecode verifier,
//! a human-readable disassembler, and a zero-allocation gas-metered execution
//! engine with bounded call depth.

pub mod crc32c;
pub mod disasm;
pub mod format;
pub mod isa;
pub mod syscall;
pub mod verifier;
pub mod vm;

pub use format::{CodecError, decode_program, encode_program};
pub use isa::{
    ConstEntry, DebugLine, FunctionMeta, Instruction, MAX_CALL_DEPTH, MAX_CONSTANTS, MAX_FUNCTIONS,
    MAX_INSTRUCTIONS, MAX_REGS, MAX_STATE_ARRAYS, MAX_STATE_SLOTS, Opcode, Program, StateArrayDesc,
    ValType,
};
pub use syscall::{HostApi, SYSCALLS, SyscallId, SyscallSpec};
pub use verifier::{VerificationReport, VerifiedProgram, VerifyError, verify};
pub use vm::{ExecStatus, TickResult, TrapReason, VmInstance};
