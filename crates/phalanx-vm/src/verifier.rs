//! eBPF-inspired static bytecode safety verifier using abstract interpretation.

use core::fmt;
use std::collections::VecDeque;

use crate::isa::{
    MAX_CALL_DEPTH, MAX_CONSTANTS, MAX_FUNCTIONS, MAX_INSTRUCTIONS, MAX_REGS, MAX_STATE_ARRAYS,
    MAX_STATE_SLOTS, Opcode, Program, ValType,
};
use crate::syscall::SyscallSpec;

/// Opaque wrapper proving a [`Program`] passed all static verification checks.
///
/// The inner [`Program`] can only be constructed by [`verify`], establishing a
/// compile-time typestate security boundary for the VM execution engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProgram {
    inner: Program,
}

impl VerifiedProgram {
    /// Returns an immutable reference to the verified program.
    #[must_use]
    pub fn program(&self) -> &Program {
        &self.inner
    }
}

/// Summary report emitted after successful static verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    /// Program name.
    pub program_name: String,
    /// Deterministic FNV-1a bytecode fingerprint.
    pub fingerprint: u64,
    /// Number of functions in the program.
    pub functions: usize,
    /// Total number of 32-bit instructions.
    pub instructions: usize,
    /// Number of reachable basic blocks across all functions.
    pub basic_blocks: usize,
    /// Number of backward branch edges (loops) detected.
    pub back_edges: usize,
    /// Proven worst-case call depth starting from the entry function.
    pub max_call_depth: usize,
    /// Persistent scalar state slot count.
    pub state_slots: usize,
    /// Persistent state array count.
    pub state_arrays: usize,
}

/// Static verification failure diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// A structural table size violates VM limits.
    LimitExceeded(&'static str),
    /// Entry function index is invalid or its signature is not `fn tick()`.
    InvalidEntryFunction,
    /// Function metadata bounds are invalid or overlap another function.
    InvalidFunctionBounds {
        /// Function name.
        name: String,
    },
    /// State array bounds or slot types are inconsistent.
    InvalidStateArray {
        /// Array name.
        name: String,
    },
    /// Unrecognized instruction opcode.
    InvalidOpcode {
        /// Bytecode PC.
        pc: usize,
        /// Raw opcode byte.
        opcode: u8,
    },
    /// Register index >= `MAX_REGS`.
    RegisterOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// Out-of-range register index.
        reg: u8,
    },
    /// Constant pool index out of bounds.
    ConstOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// Constant index.
        index: u16,
    },
    /// State slot index out of bounds.
    StateOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// State slot index.
        slot: u16,
    },
    /// State array index out of bounds.
    StateArrayOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// State array index.
        arr_id: u8,
    },
    /// Unknown syscall ID.
    InvalidSyscall {
        /// Bytecode PC.
        pc: usize,
        /// Syscall ID.
        id: u8,
    },
    /// Target function index out of bounds.
    InvalidCallTarget {
        /// Bytecode PC.
        pc: usize,
        /// Function index.
        func_id: u8,
    },
    /// Reading a register that may be uninitialized or disagrees across CFG join edges.
    UninitializedRegister {
        /// Bytecode PC.
        pc: usize,
        /// Register index.
        reg: u8,
    },
    /// Register holds a value of the wrong static type for the instruction.
    TypeMismatch {
        /// Bytecode PC.
        pc: usize,
        /// Register index.
        reg: u8,
        /// Expected static type.
        expected: ValType,
        /// Observed static type.
        found: ValType,
    },
    /// Jump or branch target escapes the enclosing function body.
    JumpOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// Computed target PC.
        target: isize,
    },
    /// Control flow reaches the end of a function without `ret` or `trap`.
    FallOffEnd {
        /// Function index.
        func_id: usize,
        /// Bytecode PC.
        pc: usize,
    },
    /// `ret` presence or value type disagrees with the function signature.
    ReturnTypeMismatch {
        /// Bytecode PC.
        pc: usize,
        /// Function name.
        func: String,
    },
    /// Call or syscall argument window exceeds `MAX_REGS`.
    ArgWindowOutOfBounds {
        /// Bytecode PC.
        pc: usize,
        /// Starting register.
        arg_base: u8,
        /// Parameter count.
        arity: usize,
    },
    /// Recursive or mutually recursive call cycle detected.
    RecursionCycle {
        /// Function involved in the cycle.
        func: String,
    },
    /// Static worst-case call stack depth exceeds `MAX_CALL_DEPTH`.
    MaxCallDepthExceeded {
        /// Proven depth.
        depth: usize,
    },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded(what) => write!(f, "program limit exceeded: {what}"),
            Self::InvalidEntryFunction => {
                f.write_str("entry function must exist and have signature `fn tick()`")
            }
            Self::InvalidFunctionBounds { name } => {
                write!(f, "function `{name}` has invalid or overlapping code range")
            }
            Self::InvalidStateArray { name } => {
                write!(
                    f,
                    "state array `{name}` has invalid slots or overlapping layout"
                )
            }
            Self::InvalidOpcode { pc, opcode } => {
                write!(f, "pc {pc}: invalid opcode 0x{opcode:02x}")
            }
            Self::RegisterOutOfBounds { pc, reg } => {
                write!(f, "pc {pc}: register r{reg} out of bounds (max {MAX_REGS})")
            }
            Self::ConstOutOfBounds { pc, index } => {
                write!(f, "pc {pc}: constant index {index} out of bounds")
            }
            Self::StateOutOfBounds { pc, slot } => {
                write!(f, "pc {pc}: state slot {slot} out of bounds")
            }
            Self::StateArrayOutOfBounds { pc, arr_id } => {
                write!(f, "pc {pc}: state array {arr_id} out of bounds")
            }
            Self::InvalidSyscall { pc, id } => write!(f, "pc {pc}: invalid syscall id {id}"),
            Self::InvalidCallTarget { pc, func_id } => {
                write!(f, "pc {pc}: invalid call target function {func_id}")
            }
            Self::UninitializedRegister { pc, reg } => {
                write!(
                    f,
                    "pc {pc}: read of uninitialized or conflicting register r{reg}"
                )
            }
            Self::TypeMismatch {
                pc,
                reg,
                expected,
                found,
            } => write!(
                f,
                "pc {pc}: register r{reg} type mismatch (expected {expected}, found {found})"
            ),
            Self::JumpOutOfBounds { pc, target } => {
                write!(
                    f,
                    "pc {pc}: branch target {target} escapes enclosing function"
                )
            }
            Self::FallOffEnd { func_id, pc } => {
                write!(
                    f,
                    "function #{func_id} falls off end at pc {pc} without ret/trap"
                )
            }
            Self::ReturnTypeMismatch { pc, func } => {
                write!(f, "pc {pc}: return value mismatches signature of `{func}`")
            }
            Self::ArgWindowOutOfBounds {
                pc,
                arg_base,
                arity,
            } => write!(
                f,
                "pc {pc}: argument window r{arg_base}..r{} exceeds MAX_REGS",
                usize::from(*arg_base) + arity
            ),
            Self::RecursionCycle { func } => {
                write!(f, "unbounded recursion cycle detected involving `{func}`")
            }
            Self::MaxCallDepthExceeded { depth } => {
                write!(
                    f,
                    "call stack depth {depth} exceeds MAX_CALL_DEPTH ({MAX_CALL_DEPTH})"
                )
            }
        }
    }
}

impl std::error::Error for VerifyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegState {
    Uninit,
    Typed(ValType),
}

/// Statically verifies a bytecode [`Program`] and produces a [`VerifiedProgram`].
pub fn verify(prog: Program) -> Result<(VerifiedProgram, VerificationReport), VerifyError> {
    check_structure(&prog)?;

    let mut call_edges = vec![Vec::<usize>::new(); prog.functions.len()];
    let mut total_blocks = 0usize;
    let mut total_back_edges = 0usize;

    for (func_idx, edges) in call_edges.iter_mut().enumerate() {
        let stats = verify_function(&prog, func_idx, edges)?;
        total_blocks += stats.basic_blocks;
        total_back_edges += stats.back_edges;
    }

    let max_call_depth = check_call_graph(&prog, &call_edges)?;
    let fingerprint = prog.fingerprint();

    let report = VerificationReport {
        program_name: prog.name.clone(),
        fingerprint,
        functions: prog.functions.len(),
        instructions: prog.code.len(),
        basic_blocks: total_blocks,
        back_edges: total_back_edges,
        max_call_depth,
        state_slots: prog.state_layout.len(),
        state_arrays: prog.state_arrays.len(),
    };

    Ok((VerifiedProgram { inner: prog }, report))
}

fn check_structure(prog: &Program) -> Result<(), VerifyError> {
    if prog.state_layout.len() > MAX_STATE_SLOTS || prog.state_init.len() != prog.state_layout.len()
    {
        return Err(VerifyError::LimitExceeded("state_slots"));
    }
    if prog.state_arrays.len() > MAX_STATE_ARRAYS {
        return Err(VerifyError::LimitExceeded("state_arrays"));
    }
    if prog.constants.len() > MAX_CONSTANTS {
        return Err(VerifyError::LimitExceeded("constants"));
    }
    if prog.functions.is_empty() || prog.functions.len() > MAX_FUNCTIONS {
        return Err(VerifyError::LimitExceeded("functions"));
    }
    if prog.code.len() > MAX_INSTRUCTIONS {
        return Err(VerifyError::LimitExceeded("instructions"));
    }

    let entry_idx = usize::from(prog.entry_func);
    let Some(entry_fn) = prog.functions.get(entry_idx) else {
        return Err(VerifyError::InvalidEntryFunction);
    };
    if !entry_fn.params.is_empty() || entry_fn.ret.is_some() {
        return Err(VerifyError::InvalidEntryFunction);
    }

    let mut slot_claimed_by_array = vec![false; prog.state_layout.len()];
    for arr in &prog.state_arrays {
        if arr.len == 0 {
            return Err(VerifyError::InvalidStateArray {
                name: arr.name.clone(),
            });
        }
        let base = usize::from(arr.base);
        let len = usize::from(arr.len);
        let Some(end) = base.checked_add(len) else {
            return Err(VerifyError::InvalidStateArray {
                name: arr.name.clone(),
            });
        };
        if end > prog.state_layout.len() {
            return Err(VerifyError::InvalidStateArray {
                name: arr.name.clone(),
            });
        }
        for claimed in &mut slot_claimed_by_array[base..end] {
            if *claimed {
                return Err(VerifyError::InvalidStateArray {
                    name: arr.name.clone(),
                });
            }
            *claimed = true;
        }
        for &layout_ty in &prog.state_layout[base..end] {
            if layout_ty != arr.elem_ty {
                return Err(VerifyError::InvalidStateArray {
                    name: arr.name.clone(),
                });
            }
        }
    }

    let mut ranges: Vec<(usize, usize)> = Vec::with_capacity(prog.functions.len());
    for func in &prog.functions {
        if func.len == 0 || func.params.len() > MAX_REGS {
            return Err(VerifyError::InvalidFunctionBounds {
                name: func.name.clone(),
            });
        }
        let start = func.entry_pc as usize;
        let Some(end) = start.checked_add(func.len as usize) else {
            return Err(VerifyError::InvalidFunctionBounds {
                name: func.name.clone(),
            });
        };
        if end > prog.code.len() {
            return Err(VerifyError::InvalidFunctionBounds {
                name: func.name.clone(),
            });
        }
        for &(other_start, other_end) in &ranges {
            if start < other_end && other_start < end {
                return Err(VerifyError::InvalidFunctionBounds {
                    name: func.name.clone(),
                });
            }
        }
        ranges.push((start, end));
    }

    Ok(())
}

struct FuncStats {
    basic_blocks: usize,
    back_edges: usize,
}

fn verify_function(
    prog: &Program,
    func_idx: usize,
    callee_edges: &mut Vec<usize>,
) -> Result<FuncStats, VerifyError> {
    let func = &prog.functions[func_idx];
    let start = func.entry_pc as usize;
    let len = func.len as usize;

    let mut states: Vec<Option<[RegState; MAX_REGS]>> = vec![None; len];
    let mut leaders = vec![false; len];
    leaders[0] = true;
    let mut back_edges = 0usize;
    let mut seen_back_edge = vec![false; len];

    let mut entry_regs = [RegState::Uninit; MAX_REGS];
    for (i, &ty) in func.params.iter().enumerate() {
        entry_regs[i] = RegState::Typed(ty);
    }

    let mut worklist = VecDeque::new();
    states[0] = Some(entry_regs);
    worklist.push_back(0usize);

    while let Some(local_pc) = worklist.pop_front() {
        let mut regs = states[local_pc].expect("worklist entry always has state");
        let abs_pc = start + local_pc;
        let insn = prog.code[abs_pc];
        let op = insn.opcode().ok_or(VerifyError::InvalidOpcode {
            pc: abs_pc,
            opcode: insn.raw_opcode(),
        })?;

        let a = insn.a();
        let b = insn.b();
        let c = insn.c();

        let mut fallthrough = true;

        match op {
            Opcode::Nop => {}
            Opcode::Trap => {
                fallthrough = false;
            }
            Opcode::LoadImmI => {
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Int);
            }
            Opcode::LoadImmBool => {
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Bool);
            }
            Opcode::LoadConst => {
                check_reg(abs_pc, a)?;
                let idx = insn.imm16();
                let entry =
                    prog.constants
                        .get(usize::from(idx))
                        .ok_or(VerifyError::ConstOutOfBounds {
                            pc: abs_pc,
                            index: idx,
                        })?;
                regs[usize::from(a)] = RegState::Typed(entry.ty);
            }
            Opcode::Move => {
                let ty = read_any(&regs, abs_pc, b)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ty);
            }
            Opcode::AddI
            | Opcode::SubI
            | Opcode::MulI
            | Opcode::DivI
            | Opcode::ModI
            | Opcode::BitAnd
            | Opcode::BitOr
            | Opcode::BitXor
            | Opcode::Shl
            | Opcode::Shr => {
                expect_reg(&regs, abs_pc, b, ValType::Int)?;
                expect_reg(&regs, abs_pc, c, ValType::Int)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Int);
            }
            Opcode::NegI | Opcode::BitNot => {
                expect_reg(&regs, abs_pc, b, ValType::Int)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Int);
            }
            Opcode::AddFix | Opcode::SubFix | Opcode::MulFix | Opcode::DivFix => {
                expect_reg(&regs, abs_pc, b, ValType::Fix)?;
                expect_reg(&regs, abs_pc, c, ValType::Fix)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Fix);
            }
            Opcode::NegFix => {
                expect_reg(&regs, abs_pc, b, ValType::Fix)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Fix);
            }
            Opcode::EqI | Opcode::NeI | Opcode::LtI | Opcode::LeI | Opcode::GtI | Opcode::GeI => {
                expect_reg(&regs, abs_pc, b, ValType::Int)?;
                expect_reg(&regs, abs_pc, c, ValType::Int)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Bool);
            }
            Opcode::EqFix
            | Opcode::NeFix
            | Opcode::LtFix
            | Opcode::LeFix
            | Opcode::GtFix
            | Opcode::GeFix => {
                expect_reg(&regs, abs_pc, b, ValType::Fix)?;
                expect_reg(&regs, abs_pc, c, ValType::Fix)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Bool);
            }
            Opcode::EqBool | Opcode::NeBool => {
                expect_reg(&regs, abs_pc, b, ValType::Bool)?;
                expect_reg(&regs, abs_pc, c, ValType::Bool)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Bool);
            }
            Opcode::NotBool => {
                expect_reg(&regs, abs_pc, b, ValType::Bool)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Bool);
            }
            Opcode::ToFix => {
                expect_reg(&regs, abs_pc, b, ValType::Int)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Fix);
            }
            Opcode::ToInt => {
                expect_reg(&regs, abs_pc, b, ValType::Fix)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(ValType::Int);
            }
            Opcode::LoadState => {
                check_reg(abs_pc, a)?;
                let slot = insn.imm16();
                let ty = prog
                    .state_layout
                    .get(usize::from(slot))
                    .copied()
                    .ok_or(VerifyError::StateOutOfBounds { pc: abs_pc, slot })?;
                regs[usize::from(a)] = RegState::Typed(ty);
            }
            Opcode::StoreState => {
                let slot = insn.imm16();
                let ty = prog
                    .state_layout
                    .get(usize::from(slot))
                    .copied()
                    .ok_or(VerifyError::StateOutOfBounds { pc: abs_pc, slot })?;
                expect_reg(&regs, abs_pc, a, ty)?;
            }
            Opcode::LoadStateArr => {
                let arr = prog.state_arrays.get(usize::from(b)).ok_or(
                    VerifyError::StateArrayOutOfBounds {
                        pc: abs_pc,
                        arr_id: b,
                    },
                )?;
                expect_reg(&regs, abs_pc, c, ValType::Int)?;
                check_reg(abs_pc, a)?;
                regs[usize::from(a)] = RegState::Typed(arr.elem_ty);
            }
            Opcode::StoreStateArr => {
                let arr = prog.state_arrays.get(usize::from(b)).ok_or(
                    VerifyError::StateArrayOutOfBounds {
                        pc: abs_pc,
                        arr_id: b,
                    },
                )?;
                expect_reg(&regs, abs_pc, c, ValType::Int)?;
                expect_reg(&regs, abs_pc, a, arr.elem_ty)?;
            }
            Opcode::Jump => {
                let target = resolve_jump(local_pc, len, abs_pc, insn.imm16_i16())?;
                leaders[target] = true;
                if target <= local_pc && !seen_back_edge[local_pc] {
                    seen_back_edge[local_pc] = true;
                    back_edges += 1;
                }
                merge_state(&mut states, &mut worklist, target, &regs);
                fallthrough = false;
            }
            Opcode::JumpIfFalse | Opcode::JumpIfTrue => {
                expect_reg(&regs, abs_pc, a, ValType::Bool)?;
                let target = resolve_jump(local_pc, len, abs_pc, insn.imm16_i16())?;
                leaders[target] = true;
                if target <= local_pc && !seen_back_edge[local_pc] {
                    seen_back_edge[local_pc] = true;
                    back_edges += 1;
                }
                if local_pc + 1 >= len {
                    return Err(VerifyError::FallOffEnd {
                        func_id: func_idx,
                        pc: abs_pc,
                    });
                }
                leaders[local_pc + 1] = true;
                merge_state(&mut states, &mut worklist, target, &regs);
            }
            Opcode::Call => {
                let callee =
                    prog.functions
                        .get(usize::from(b))
                        .ok_or(VerifyError::InvalidCallTarget {
                            pc: abs_pc,
                            func_id: b,
                        })?;
                let arity = callee.params.len();
                if usize::from(c) + arity > MAX_REGS {
                    return Err(VerifyError::ArgWindowOutOfBounds {
                        pc: abs_pc,
                        arg_base: c,
                        arity,
                    });
                }
                for (i, &param_ty) in callee.params.iter().enumerate() {
                    expect_reg(&regs, abs_pc, c + i as u8, param_ty)?;
                }
                check_reg(abs_pc, a)?;
                if let Some(ret_ty) = callee.ret {
                    regs[usize::from(a)] = RegState::Typed(ret_ty);
                }
                let callee_idx = usize::from(b);
                if !callee_edges.contains(&callee_idx) {
                    callee_edges.push(callee_idx);
                }
            }
            Opcode::Ret => {
                let has_val = b != 0;
                match (func.ret, has_val) {
                    (None, false) => {}
                    (Some(expected), true) => {
                        expect_reg(&regs, abs_pc, a, expected)?;
                    }
                    _ => {
                        return Err(VerifyError::ReturnTypeMismatch {
                            pc: abs_pc,
                            func: func.name.clone(),
                        });
                    }
                }
                fallthrough = false;
            }
            Opcode::Syscall => {
                let spec = SyscallSpec::from_id(b)
                    .ok_or(VerifyError::InvalidSyscall { pc: abs_pc, id: b })?;
                let arity = spec.params.len();
                if usize::from(c) + arity > MAX_REGS {
                    return Err(VerifyError::ArgWindowOutOfBounds {
                        pc: abs_pc,
                        arg_base: c,
                        arity,
                    });
                }
                for (i, &param_ty) in spec.params.iter().enumerate() {
                    expect_reg(&regs, abs_pc, c + i as u8, param_ty)?;
                }
                check_reg(abs_pc, a)?;
                if let Some(ret_ty) = spec.ret {
                    regs[usize::from(a)] = RegState::Typed(ret_ty);
                }
            }
        }

        if fallthrough {
            let next_pc = local_pc + 1;
            if next_pc >= len {
                return Err(VerifyError::FallOffEnd {
                    func_id: func_idx,
                    pc: abs_pc,
                });
            }
            merge_state(&mut states, &mut worklist, next_pc, &regs);
        }
    }

    let basic_blocks = states
        .iter()
        .enumerate()
        .filter(|&(pc, st)| st.is_some() && leaders[pc])
        .count();

    Ok(FuncStats {
        basic_blocks,
        back_edges,
    })
}

fn resolve_jump(
    local_pc: usize,
    len: usize,
    abs_pc: usize,
    offset: i16,
) -> Result<usize, VerifyError> {
    let target = (local_pc as isize) + isize::from(offset);
    if target < 0 || (target as usize) >= len {
        return Err(VerifyError::JumpOutOfBounds {
            pc: abs_pc,
            target: (abs_pc as isize) + isize::from(offset),
        });
    }
    Ok(target as usize)
}

fn merge_state(
    states: &mut [Option<[RegState; MAX_REGS]>],
    worklist: &mut VecDeque<usize>,
    target: usize,
    incoming: &[RegState; MAX_REGS],
) {
    match &mut states[target] {
        None => {
            states[target] = Some(*incoming);
            worklist.push_back(target);
        }
        Some(existing) => {
            let mut changed = false;
            for r in 0..MAX_REGS {
                if existing[r] != RegState::Uninit && existing[r] != incoming[r] {
                    existing[r] = RegState::Uninit;
                    changed = true;
                }
            }
            if changed {
                worklist.push_back(target);
            }
        }
    }
}

fn check_reg(pc: usize, reg: u8) -> Result<(), VerifyError> {
    if usize::from(reg) >= MAX_REGS {
        return Err(VerifyError::RegisterOutOfBounds { pc, reg });
    }
    Ok(())
}

fn read_any(regs: &[RegState; MAX_REGS], pc: usize, reg: u8) -> Result<ValType, VerifyError> {
    check_reg(pc, reg)?;
    match regs[usize::from(reg)] {
        RegState::Uninit => Err(VerifyError::UninitializedRegister { pc, reg }),
        RegState::Typed(ty) => Ok(ty),
    }
}

fn expect_reg(
    regs: &[RegState; MAX_REGS],
    pc: usize,
    reg: u8,
    expected: ValType,
) -> Result<(), VerifyError> {
    let found = read_any(regs, pc, reg)?;
    if found != expected {
        return Err(VerifyError::TypeMismatch {
            pc,
            reg,
            expected,
            found,
        });
    }
    Ok(())
}

fn check_call_graph(prog: &Program, edges: &[Vec<usize>]) -> Result<usize, VerifyError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        Unvisited,
        Visiting,
        Visited,
    }

    fn dfs(
        node: usize,
        prog: &Program,
        edges: &[Vec<usize>],
        color: &mut [Color],
        depth_memo: &mut [usize],
    ) -> Result<usize, VerifyError> {
        match color[node] {
            Color::Visiting => {
                return Err(VerifyError::RecursionCycle {
                    func: prog.functions[node].name.clone(),
                });
            }
            Color::Visited => return Ok(depth_memo[node]),
            Color::Unvisited => {}
        }

        color[node] = Color::Visiting;
        let mut max_child = 0usize;
        for &callee in &edges[node] {
            let child_depth = dfs(callee, prog, edges, color, depth_memo)?;
            max_child = max_child.max(child_depth);
        }
        color[node] = Color::Visited;
        let depth = 1 + max_child;
        depth_memo[node] = depth;
        Ok(depth)
    }

    let n = prog.functions.len();
    let mut color = vec![Color::Unvisited; n];
    let mut depth_memo = vec![0usize; n];

    for func_idx in 0..n {
        let d = dfs(func_idx, prog, edges, &mut color, &mut depth_memo)?;
        if d > MAX_CALL_DEPTH {
            return Err(VerifyError::MaxCallDepthExceeded { depth: d });
        }
    }

    Ok(depth_memo[usize::from(prog.entry_func)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::{FunctionMeta, Instruction, Opcode, StateArrayDesc};

    fn minimal_prog(code: Vec<Instruction>) -> Program {
        let len = code.len() as u32;
        Program {
            name: "test".into(),
            entry_func: 0,
            state_layout: vec![],
            state_init: vec![],
            state_names: vec![],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![FunctionMeta {
                name: "tick".into(),
                entry_pc: 0,
                len,
                params: vec![],
                ret: None,
                num_regs: 4,
            }],
            code,
            debug_lines: vec![],
        }
    }

    #[test]
    fn rejects_jump_out_of_bounds() {
        let prog = minimal_prog(vec![
            Instruction::ab_simm(Opcode::Jump, 0, 5),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ]);
        assert!(matches!(
            verify(prog),
            Err(VerifyError::JumpOutOfBounds { .. })
        ));
    }

    #[test]
    fn rejects_fall_off_end() {
        let prog = minimal_prog(vec![Instruction::ab_imm(Opcode::LoadImmI, 0, 1)]);
        assert!(matches!(verify(prog), Err(VerifyError::FallOffEnd { .. })));
    }

    #[test]
    fn rejects_uninit_register_read() {
        let prog = minimal_prog(vec![
            Instruction::abc(Opcode::AddI, 0, 1, 2),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ]);
        assert!(matches!(
            verify(prog),
            Err(VerifyError::UninitializedRegister { .. })
        ));
    }

    #[test]
    fn rejects_type_confusion_across_join() {
        // r0 = true; if r0 { r1 = 10 (Int) } else { r1 = true (Bool) }; r2 = r1 + r1
        // Place Int on the else-target (pushed first in FIFO worklist? No: fallthrough is
        // checked if we jump over the Int block to Bool block, or verify either rejection).
        let prog = minimal_prog(vec![
            Instruction::ab_imm(Opcode::LoadImmBool, 0, 1), // 0
            Instruction::ab_simm(Opcode::JumpIfTrue, 0, 3), // 1 -> 4 (Int checked first)
            Instruction::ab_imm(Opcode::LoadImmBool, 1, 1), // 2
            Instruction::ab_simm(Opcode::Jump, 0, 2),       // 3 -> 5 (demotes r1 to Uninit!)
            Instruction::ab_imm(Opcode::LoadImmI, 1, 10),   // 4
            Instruction::abc(Opcode::AddI, 2, 1, 1),        // 5
            Instruction::abc(Opcode::Ret, 0, 0, 0),         // 6
        ]);
        assert!(matches!(
            verify(prog),
            Err(VerifyError::UninitializedRegister { pc: 5, reg: 1 })
        ));
    }

    #[test]
    fn rejects_fix_int_mismatch() {
        let prog = minimal_prog(vec![
            Instruction::ab_imm(Opcode::LoadImmI, 0, 5),
            Instruction::abc(Opcode::AddFix, 1, 0, 0),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ]);
        assert!(matches!(
            verify(prog),
            Err(VerifyError::TypeMismatch {
                expected: ValType::Fix,
                found: ValType::Int,
                ..
            })
        ));
    }

    #[test]
    fn rejects_recursion_cycle() {
        let prog = Program {
            name: "cycle".into(),
            entry_func: 0,
            state_layout: vec![],
            state_init: vec![],
            state_names: vec![],
            state_arrays: vec![],
            constants: vec![],
            functions: vec![
                FunctionMeta {
                    name: "tick".into(),
                    entry_pc: 0,
                    len: 2,
                    params: vec![],
                    ret: None,
                    num_regs: 1,
                },
                FunctionMeta {
                    name: "recur".into(),
                    entry_pc: 2,
                    len: 2,
                    params: vec![],
                    ret: None,
                    num_regs: 1,
                },
            ],
            code: vec![
                Instruction::abc(Opcode::Call, 0, 1, 0),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
                Instruction::abc(Opcode::Call, 0, 1, 0),
                Instruction::abc(Opcode::Ret, 0, 0, 0),
            ],
            debug_lines: vec![],
        };
        assert!(matches!(
            verify(prog),
            Err(VerifyError::RecursionCycle { .. })
        ));
    }

    #[test]
    fn rejects_invalid_syscall_and_array_oob() {
        let bad_sys = minimal_prog(vec![
            Instruction::abc(Opcode::Syscall, 0, 250, 0),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ]);
        assert!(matches!(
            verify(bad_sys),
            Err(VerifyError::InvalidSyscall { .. })
        ));

        let mut bad_arr = minimal_prog(vec![
            Instruction::ab_imm(Opcode::LoadImmI, 1, 0),
            Instruction::abc(Opcode::LoadStateArr, 0, 0, 1),
            Instruction::abc(Opcode::Ret, 0, 0, 0),
        ]);
        assert!(matches!(
            verify(bad_arr.clone()),
            Err(VerifyError::StateArrayOutOfBounds { .. })
        ));

        bad_arr.state_layout = vec![ValType::Int; 2];
        bad_arr.state_init = vec![0; 2];
        bad_arr.state_arrays = vec![StateArrayDesc {
            name: "arr".into(),
            base: 0,
            len: 2,
            elem_ty: ValType::Int,
        }];
        assert!(verify(bad_arr).is_ok());
    }
}
