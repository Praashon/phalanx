//! Zero-allocation gas-metered 32-bit register VM execution engine.

use core::fmt;

use crate::isa::{MAX_CALL_DEPTH, MAX_REGS, MAX_STATE_SLOTS, Opcode};
use crate::syscall::{HostApi, SyscallSpec};
use crate::verifier::VerifiedProgram;

/// Runtime fault that halts execution of a bot tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapReason {
    /// Integer or fixed-point division/modulo by zero.
    DivByZero,
    /// Persistent state array index `< 0` or `>= len`.
    ArrayOutOfBounds,
    /// Program executed an explicit `trap` instruction.
    ExplicitTrap,
    /// Dynamic call depth exceeded `MAX_CALL_DEPTH`.
    CallStackOverflow,
    /// Internal VM state corruption (unreachable on verified programs).
    InvalidState,
}

impl fmt::Display for TrapReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DivByZero => f.write_str("division by zero"),
            Self::ArrayOutOfBounds => f.write_str("state array index out of bounds"),
            Self::ExplicitTrap => f.write_str("explicit trap"),
            Self::CallStackOverflow => f.write_str("call stack overflow"),
            Self::InvalidState => f.write_str("invalid VM state"),
        }
    }
}

/// Execution state of a VM instance during or after a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecStatus {
    /// Execution is in progress (ready for single-stepping).
    Running,
    /// Entry `tick()` function returned normally.
    Completed,
    /// Gas budget reached zero before `tick()` returned.
    OutOfGas,
    /// Execution trapped due to a runtime error.
    Trapped(TrapReason),
}

/// Summary returned by [`VmInstance::run_tick`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickResult {
    /// Final execution status for this tick.
    pub status: ExecStatus,
    /// Total gas consumed during this tick.
    pub gas_used: u32,
    /// Total bytecode instructions executed during this tick.
    pub instructions_executed: u32,
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    func_id: u16,
    pc: u32,
    ret_dst: u8,
    has_ret: bool,
    regs: [i32; MAX_REGS],
}

impl Frame {
    const EMPTY: Self = Self {
        func_id: 0,
        pc: 0,
        ret_dst: 0,
        has_ret: false,
        regs: [0; MAX_REGS],
    };
}

/// Sandboxed VM instance with fixed pre-allocated call stack and persistent state memory.
///
/// Once constructed via [`VmInstance::new`], [`VmInstance::begin_tick`],
/// [`VmInstance::step`], and [`VmInstance::run_tick`] perform zero heap allocations.
#[derive(Debug, Clone)]
pub struct VmInstance {
    state: [i32; MAX_STATE_SLOTS],
    frames: [Frame; MAX_CALL_DEPTH],
    depth: usize,
    gas_remaining: u32,
    gas_used: u32,
    instructions_executed: u32,
    status: ExecStatus,
}

impl VmInstance {
    /// Allocates and initializes a VM instance from a [`VerifiedProgram`].
    #[must_use]
    pub fn new(prog: &VerifiedProgram) -> Self {
        let mut state = [0i32; MAX_STATE_SLOTS];
        let raw = prog.program();
        for (i, &val) in raw.state_init.iter().take(MAX_STATE_SLOTS).enumerate() {
            state[i] = val;
        }
        Self {
            state,
            frames: [Frame::EMPTY; MAX_CALL_DEPTH],
            depth: 0,
            gas_remaining: 0,
            gas_used: 0,
            instructions_executed: 0,
            status: ExecStatus::Completed,
        }
    }

    /// Resets call stack and gas meters for a new simulation tick (zero heap allocations).
    pub fn begin_tick(&mut self, prog: &VerifiedProgram, gas_budget: u32) {
        let raw = prog.program();
        let entry = raw.entry_func;
        let entry_pc = raw.functions[usize::from(entry)].entry_pc;
        self.depth = 1;
        let f = &mut self.frames[0];
        f.func_id = entry;
        f.pc = entry_pc;
        f.ret_dst = 0;
        f.has_ret = false;
        f.regs = [0; MAX_REGS];
        self.gas_remaining = gas_budget;
        self.gas_used = 0;
        self.instructions_executed = 0;
        self.status = ExecStatus::Running;
    }

    /// Executes a full tick up to `gas_budget` without any heap allocation.
    pub fn run_tick(
        &mut self,
        prog: &VerifiedProgram,
        host: &mut impl HostApi,
        gas_budget: u32,
    ) -> TickResult {
        self.begin_tick(prog, gas_budget);
        while self.status == ExecStatus::Running {
            self.step(prog, host);
        }
        TickResult {
            status: self.status,
            gas_used: self.gas_used,
            instructions_executed: self.instructions_executed,
        }
    }

    /// Executes a single bytecode instruction for debugger single-stepping.
    pub fn step(&mut self, prog: &VerifiedProgram, host: &mut impl HostApi) -> ExecStatus {
        if self.status != ExecStatus::Running {
            return self.status;
        }
        if self.depth == 0 || self.depth > MAX_CALL_DEPTH {
            self.status = ExecStatus::Trapped(TrapReason::InvalidState);
            return self.status;
        }

        let raw = prog.program();
        let frame_idx = self.depth - 1;
        let pc = self.frames[frame_idx].pc as usize;
        let Some(&insn) = raw.code.get(pc) else {
            self.status = ExecStatus::Trapped(TrapReason::InvalidState);
            return self.status;
        };
        let Some(op) = insn.opcode() else {
            self.status = ExecStatus::Trapped(TrapReason::InvalidState);
            return self.status;
        };

        let cost = if op == Opcode::Syscall {
            match SyscallSpec::from_id(insn.b()) {
                Some(spec) => op.gas_cost() + spec.gas,
                None => {
                    self.status = ExecStatus::Trapped(TrapReason::InvalidState);
                    return self.status;
                }
            }
        } else {
            op.gas_cost()
        };

        if self.gas_remaining < cost {
            self.status = ExecStatus::OutOfGas;
            return self.status;
        }
        self.gas_remaining -= cost;
        self.gas_used += cost;
        self.instructions_executed += 1;

        let a = usize::from(insn.a());
        let b = usize::from(insn.b());
        let c = usize::from(insn.c());

        match op {
            Opcode::Nop => {
                self.frames[frame_idx].pc += 1;
            }
            Opcode::Trap => {
                self.status = ExecStatus::Trapped(TrapReason::ExplicitTrap);
            }
            Opcode::LoadImmI => {
                self.frames[frame_idx].regs[a] = i32::from(insn.imm16_i16());
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LoadImmBool => {
                self.frames[frame_idx].regs[a] = i32::from(insn.imm16() != 0);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LoadConst => {
                let idx = usize::from(insn.imm16());
                self.frames[frame_idx].regs[a] = raw.constants[idx].value;
                self.frames[frame_idx].pc += 1;
            }
            Opcode::Move => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::AddI => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_add(regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::SubI => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_sub(regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::MulI => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_mul(regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::DivI => {
                let regs = &mut self.frames[frame_idx].regs;
                let rhs = regs[c];
                if rhs == 0 {
                    self.status = ExecStatus::Trapped(TrapReason::DivByZero);
                    return self.status;
                }
                regs[a] = regs[b].wrapping_div(rhs);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::ModI => {
                let regs = &mut self.frames[frame_idx].regs;
                let rhs = regs[c];
                if rhs == 0 {
                    self.status = ExecStatus::Trapped(TrapReason::DivByZero);
                    return self.status;
                }
                regs[a] = regs[b].wrapping_rem(rhs);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::NegI => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_neg();
                self.frames[frame_idx].pc += 1;
            }
            Opcode::AddFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].saturating_add(regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::SubFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].saturating_sub(regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::MulFix => {
                let regs = &mut self.frames[frame_idx].regs;
                let prod = (i64::from(regs[b]) * i64::from(regs[c])) >> 16;
                regs[a] = prod.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                self.frames[frame_idx].pc += 1;
            }
            Opcode::DivFix => {
                let regs = &mut self.frames[frame_idx].regs;
                let rhs = regs[c];
                if rhs == 0 {
                    self.status = ExecStatus::Trapped(TrapReason::DivByZero);
                    return self.status;
                }
                let quot = (i64::from(regs[b]) << 16) / i64::from(rhs);
                regs[a] = quot.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                self.frames[frame_idx].pc += 1;
            }
            Opcode::NegFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].saturating_neg();
                self.frames[frame_idx].pc += 1;
            }
            Opcode::BitAnd => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b] & regs[c];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::BitOr => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b] | regs[c];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::BitXor => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b] ^ regs[c];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::BitNot => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = !regs[b];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::Shl => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_shl((regs[c] as u32) & 31);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::Shr => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b].wrapping_shr((regs[c] as u32) & 31);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::EqI | Opcode::EqFix | Opcode::EqBool => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] == regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::NeI | Opcode::NeFix | Opcode::NeBool => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] != regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LtI | Opcode::LtFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] < regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LeI | Opcode::LeFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] <= regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::GtI | Opcode::GtFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] > regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::GeI | Opcode::GeFix => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] >= regs[c]);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::NotBool => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = i32::from(regs[b] == 0);
                self.frames[frame_idx].pc += 1;
            }
            Opcode::ToFix => {
                let regs = &mut self.frames[frame_idx].regs;
                let shifted = i64::from(regs[b]) << 16;
                regs[a] = shifted.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                self.frames[frame_idx].pc += 1;
            }
            Opcode::ToInt => {
                let regs = &mut self.frames[frame_idx].regs;
                regs[a] = regs[b] >> 16;
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LoadState => {
                let slot = usize::from(insn.imm16());
                self.frames[frame_idx].regs[a] = self.state[slot];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::StoreState => {
                let slot = usize::from(insn.imm16());
                self.state[slot] = self.frames[frame_idx].regs[a];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::LoadStateArr => {
                let desc = &raw.state_arrays[b];
                let idx = self.frames[frame_idx].regs[c];
                if idx < 0 || (idx as u32) >= u32::from(desc.len) {
                    self.status = ExecStatus::Trapped(TrapReason::ArrayOutOfBounds);
                    return self.status;
                }
                let slot = usize::from(desc.base) + (idx as usize);
                self.frames[frame_idx].regs[a] = self.state[slot];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::StoreStateArr => {
                let desc = &raw.state_arrays[b];
                let idx = self.frames[frame_idx].regs[c];
                if idx < 0 || (idx as u32) >= u32::from(desc.len) {
                    self.status = ExecStatus::Trapped(TrapReason::ArrayOutOfBounds);
                    return self.status;
                }
                let slot = usize::from(desc.base) + (idx as usize);
                self.state[slot] = self.frames[frame_idx].regs[a];
                self.frames[frame_idx].pc += 1;
            }
            Opcode::Jump => {
                let target = (pc as i32) + i32::from(insn.imm16_i16());
                self.frames[frame_idx].pc = target as u32;
            }
            Opcode::JumpIfFalse => {
                if self.frames[frame_idx].regs[a] == 0 {
                    let target = (pc as i32) + i32::from(insn.imm16_i16());
                    self.frames[frame_idx].pc = target as u32;
                } else {
                    self.frames[frame_idx].pc += 1;
                }
            }
            Opcode::JumpIfTrue => {
                if self.frames[frame_idx].regs[a] != 0 {
                    let target = (pc as i32) + i32::from(insn.imm16_i16());
                    self.frames[frame_idx].pc = target as u32;
                } else {
                    self.frames[frame_idx].pc += 1;
                }
            }
            Opcode::Call => {
                if self.depth >= MAX_CALL_DEPTH {
                    self.status = ExecStatus::Trapped(TrapReason::CallStackOverflow);
                    return self.status;
                }
                let callee = &raw.functions[b];
                let arity = callee.params.len();
                let mut next_regs = [0i32; MAX_REGS];
                next_regs[..arity].copy_from_slice(&self.frames[frame_idx].regs[c..c + arity]);
                self.frames[frame_idx].pc += 1;

                let child = &mut self.frames[self.depth];
                child.func_id = b as u16;
                child.pc = callee.entry_pc;
                child.ret_dst = insn.a();
                child.has_ret = callee.ret.is_some();
                child.regs = next_regs;
                self.depth += 1;
            }
            Opcode::Ret => {
                let ret_val = if b != 0 {
                    self.frames[frame_idx].regs[a]
                } else {
                    0
                };
                let ret_dst = usize::from(self.frames[frame_idx].ret_dst);
                let has_ret = self.frames[frame_idx].has_ret;
                self.depth -= 1;
                if self.depth == 0 {
                    self.status = ExecStatus::Completed;
                } else if has_ret {
                    self.frames[self.depth - 1].regs[ret_dst] = ret_val;
                }
            }
            Opcode::Syscall => {
                let spec = SyscallSpec::from_id(insn.b()).expect("verified syscall id");
                let arity = spec.params.len();
                let mut args_buf = [0i32; MAX_REGS];
                args_buf[..arity].copy_from_slice(&self.frames[frame_idx].regs[c..c + arity]);
                let res = host.invoke(spec.id, &args_buf[..arity]);
                if spec.ret.is_some() {
                    self.frames[frame_idx].regs[a] = res;
                }
                self.frames[frame_idx].pc += 1;
            }
        }

        self.status
    }

    /// Current execution status.
    #[must_use]
    pub fn status(&self) -> ExecStatus {
        self.status
    }

    /// Current program counter of the active frame, or `None` if no frame is active.
    #[must_use]
    pub fn current_pc(&self) -> Option<u32> {
        self.depth.checked_sub(1).map(|idx| self.frames[idx].pc)
    }

    /// Current function ID of the active frame, or `None` if no frame is active.
    #[must_use]
    pub fn current_func(&self) -> Option<u16> {
        self.depth
            .checked_sub(1)
            .map(|idx| self.frames[idx].func_id)
    }

    /// Active frame register slice for live debugging.
    #[must_use]
    pub fn active_regs(&self) -> Option<&[i32; MAX_REGS]> {
        self.depth.checked_sub(1).map(|idx| &self.frames[idx].regs)
    }

    /// Read-only view of persistent state slots `0..len`.
    #[must_use]
    pub fn state_slice(&self, len: usize) -> &[i32] {
        &self.state[..len.min(MAX_STATE_SLOTS)]
    }

    /// Remaining gas in the current tick.
    #[must_use]
    pub fn gas_remaining(&self) -> u32 {
        self.gas_remaining
    }

    /// Gas consumed so far in the current tick.
    #[must_use]
    pub fn gas_used(&self) -> u32 {
        self.gas_used
    }

    /// Instructions executed so far in the current tick.
    #[must_use]
    pub fn instructions_executed(&self) -> u32 {
        self.instructions_executed
    }

    /// Current dynamic call stack depth.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::{FunctionMeta, Instruction, Opcode, Program, StateArrayDesc, ValType};
    use crate::syscall::{HostApi, SyscallId};
    use crate::verifier::verify;

    struct NoopHost {
        logs: Vec<(i32, i32)>,
    }

    impl HostApi for NoopHost {
        fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32 {
            match id {
                SyscallId::DebugLog => {
                    self.logs.push((args[0], args[1]));
                    0
                }
                _ => 0,
            }
        }
    }

    #[test]
    fn gas_exhaustion_halts_infinite_loop() {
        let prog = Program {
            name: "inf".into(),
            entry_func: 0,
            state_layout: vec![],
            state_init: vec![],
            state_names: vec![],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 2,
                params: vec![],
                ret: None,
                num_regs: 1,
            }],
            code: vec![
                Instruction::ab_imm(Opcode::Nop, 0, 0),
                Instruction::ab_simm(Opcode::Jump, 0, -1),
            ],
            debug_lines: vec![],
        };
        let (vp, _) = verify(prog).expect("verified");
        let mut vm = VmInstance::new(&vp);
        let mut host = NoopHost { logs: vec![] };
        let res = vm.run_tick(&vp, &mut host, 100);
        assert_eq!(res.status, ExecStatus::OutOfGas);
        assert_eq!(res.gas_used, 100);
        assert_eq!(res.instructions_executed, 100);
    }

    #[test]
    fn div_by_zero_and_oob_array_trap_cleanly() {
        let div_zero = Program {
            name: "dz".into(),
            entry_func: 0,
            state_layout: vec![],
            state_init: vec![],
            state_names: vec![],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 4,
                params: vec![],
                ret: None,
                num_regs: 3,
            }],
            code: vec![
                Instruction::ab_imm(Opcode::LoadImmI, 0, 10),
                Instruction::ab_imm(Opcode::LoadImmI, 1, 0),
                Instruction::abc(Opcode::DivI, 2, 0, 1),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
            ],
            debug_lines: vec![],
        };
        let (vp, _) = verify(div_zero).unwrap();
        let mut vm = VmInstance::new(&vp);
        let mut host = NoopHost { logs: vec![] };
        assert_eq!(
            vm.run_tick(&vp, &mut host, 50).status,
            ExecStatus::Trapped(TrapReason::DivByZero)
        );

        let oob_arr = Program {
            name: "oob".into(),
            entry_func: 0,
            state_layout: vec![ValType::Int; 2],
            state_init: vec![0; 2],
            state_names: vec![String::new(); 2],
            state_arrays: vec![StateArrayDesc {
                name: "a".into(),
                base: 0,
                len: 2,
                elem_ty: ValType::Int,
            }],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 3,
                params: vec![],
                ret: None,
                num_regs: 2,
            }],
            code: vec![
                Instruction::ab_imm(Opcode::LoadImmI, 1, 5),
                Instruction::abc(Opcode::LoadStateArr, 0, 0, 1),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
            ],
            debug_lines: vec![],
        };
        let (vp2, _) = verify(oob_arr).unwrap();
        let mut vm2 = VmInstance::new(&vp2);
        assert_eq!(
            vm2.run_tick(&vp2, &mut host, 50).status,
            ExecStatus::Trapped(TrapReason::ArrayOutOfBounds)
        );
    }

    #[test]
    fn fixed_point_arithmetic_and_persistent_state() {
        let prog = Program {
            name: "stateful".into(),
            entry_func: 0,
            state_layout: vec![ValType::Fix],
            state_init: vec![65536], // 1.0
            state_names: vec!["acc".into()],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len: 6,
                params: vec![],
                ret: None,
                num_regs: 4,
            }],
            code: vec![
                Instruction::ab_imm(Opcode::LoadState, 0, 0),
                Instruction::ab_imm(Opcode::LoadImmI, 1, 2),
                Instruction::abc(Opcode::ToFix, 1, 1, 0),
                Instruction::abc(Opcode::MulFix, 0, 0, 1),
                Instruction::ab_imm(Opcode::StoreState, 0, 0),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
            ],
            debug_lines: vec![],
        };
        let (vp, _) = verify(prog).unwrap();
        let mut vm = VmInstance::new(&vp);
        let mut host = NoopHost { logs: vec![] };
        for _ in 0..3 {
            assert_eq!(
                vm.run_tick(&vp, &mut host, 100).status,
                ExecStatus::Completed
            );
        }
        assert_eq!(vm.state_slice(1)[0], 8 * 65536);
    }
}
