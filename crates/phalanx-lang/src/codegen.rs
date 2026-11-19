//! Dataflow liveness, linear-scan register allocation, and bytecode emission.

use std::collections::{HashMap, HashSet};

use phalanx_vm::{
    ConstEntry, DebugLine, FunctionMeta, Instruction, MAX_REGS, Opcode, Program, StateArrayDesc,
    ValType,
};

use crate::ast::{BinaryOp, UnaryOp};
use crate::diagnostic::SourceMap;
use crate::ir::{BlockId, IrFn, IrInst, IrProgram, Terminator, VReg};
use crate::opt::{inst_dst, visit_inst_uses, visit_term_uses};

/// Emits a [`Program`] from an optimized [`IrProgram`].
#[must_use]
pub fn emit(name: &str, ir: &IrProgram, source_map: &SourceMap<'_>) -> Program {
    let mut constants = Vec::new();
    let mut const_dedup: HashMap<(i32, ValType), u16> = HashMap::new();
    let mut functions = Vec::with_capacity(ir.functions.len());
    let mut code = Vec::new();
    let mut debug_lines = Vec::new();

    for func in &ir.functions {
        let entry_pc = code.len() as u32;
        let num_regs = emit_function(
            func,
            source_map,
            &mut constants,
            &mut const_dedup,
            &mut code,
            &mut debug_lines,
        );
        let len = (code.len() as u32) - entry_pc;
        functions.push(FunctionMeta {
            name: func.name.clone(),
            entry_pc,
            len,
            params: func.params.iter().map(|&(_, ty)| ty).collect(),
            ret: func.ret,
            num_regs,
        });
    }

    let state_arrays = ir
        .state_arrays
        .iter()
        .map(|(n, base, len, elem_ty)| StateArrayDesc {
            name: n.clone(),
            base: *base,
            len: *len,
            elem_ty: *elem_ty,
        })
        .collect();

    Program {
        name: name.to_owned(),
        entry_func: ir.entry_func,
        state_layout: ir.state_layout.clone(),
        state_init: ir.state_init.clone(),
        state_names: ir.state_names.clone(),
        state_arrays,
        constants,
        functions,
        code,
        debug_lines,
    }
}

fn compute_rpo(func: &IrFn) -> Vec<BlockId> {
    let mut visited = vec![false; func.blocks.len()];
    let mut postorder = Vec::with_capacity(func.blocks.len());

    fn dfs(bb: BlockId, func: &IrFn, visited: &mut [bool], postorder: &mut Vec<BlockId>) {
        if visited[bb.0] {
            return;
        }
        visited[bb.0] = true;
        for succ in func.blocks[bb.0].term.successors() {
            dfs(succ, func, visited, postorder);
        }
        postorder.push(bb);
    }

    if !func.blocks.is_empty() {
        dfs(BlockId(0), func, &mut visited, &mut postorder);
    }
    postorder.reverse();
    postorder
}

#[derive(Debug, Clone, Copy)]
struct Interval {
    vreg: VReg,
    start: usize,
    end: usize,
}

fn allocate_registers(func: &IrFn, rpo: &[BlockId]) -> (Vec<u8>, u8) {
    let num_vregs = func.vreg_types.len();
    let max_call_arity = func
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .map(|inst| match inst {
            IrInst::Call { args, .. } | IrInst::Syscall { args, .. } => args.len(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);

    let arg_window = max_call_arity.min(MAX_REGS);
    let alloc_limit = MAX_REGS.saturating_sub(arg_window).max(1);

    // 1. Backward iterative dataflow liveness across basic blocks.
    let mut live_in: Vec<HashSet<VReg>> = vec![HashSet::new(); func.blocks.len()];
    let mut live_out: Vec<HashSet<VReg>> = vec![HashSet::new(); func.blocks.len()];

    loop {
        let mut changed = false;
        for &bb_id in rpo.iter().rev() {
            let bb = &func.blocks[bb_id.0];
            let mut out = HashSet::new();
            for succ in bb.term.successors() {
                out.extend(live_in[succ.0].iter().copied());
            }

            let mut cur = out.clone();
            visit_term_uses(&bb.term, &mut |v| {
                cur.insert(v);
            });
            for inst in bb.insts.iter().rev() {
                if let Some(d) = inst_dst(inst) {
                    cur.remove(&d);
                }
                visit_inst_uses(inst, &mut |v| {
                    cur.insert(v);
                });
            }

            if out != live_out[bb_id.0] || cur != live_in[bb_id.0] {
                live_out[bb_id.0] = out;
                live_in[bb_id.0] = cur;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // 2. Assign linear instruction positions in RPO block order.
    let mut pos = 1usize; // 0 reserved for function parameter arrival
    let mut min_pos = vec![usize::MAX; num_vregs];
    let mut max_pos = vec![0usize; num_vregs];
    let mut record = |v: VReg, p: usize| {
        let idx = v.0 as usize;
        min_pos[idx] = min_pos[idx].min(p);
        max_pos[idx] = max_pos[idx].max(p);
    };

    for &(pv, _) in &func.params {
        record(pv, 0);
    }

    for &bb_id in rpo {
        let bb = &func.blocks[bb_id.0];
        let bb_start = pos;
        for &v in &live_in[bb_id.0] {
            record(v, bb_start);
        }
        for inst in &bb.insts {
            visit_inst_uses(inst, &mut |v| record(v, pos));
            if let Some(d) = inst_dst(inst) {
                record(d, pos);
                // Ensure unused pure def still reserves one tick so it doesn't clobber live regs.
                record(d, pos + 1);
            }
            pos += 1;
        }
        let term_pos = pos;
        visit_term_uses(&bb.term, &mut |v| record(v, term_pos));
        for &v in &live_out[bb_id.0] {
            record(v, term_pos + 1);
        }
        pos += 1;
    }

    let mut reg_map = vec![0u8; num_vregs];
    let mut active: Vec<(VReg, usize, u8)> = Vec::new();
    let mut free_regs: Vec<u8> = (0..alloc_limit as u8).rev().collect();

    // Pre-pin entry parameter vregs to r0..rN because VM passes params in r0..rN.
    for (param_idx, &(pv, _)) in func.params.iter().enumerate() {
        let preg = param_idx as u8;
        reg_map[pv.0 as usize] = preg;
        free_regs.retain(|&r| r != preg);
        active.push((pv, max_pos[pv.0 as usize], preg));
    }

    let mut intervals: Vec<Interval> = (0..num_vregs)
        .filter(|&idx| {
            min_pos[idx] != usize::MAX && !func.params.iter().any(|&(pv, _)| pv.0 as usize == idx)
        })
        .map(|idx| Interval {
            vreg: VReg(idx as u32),
            start: min_pos[idx],
            end: max_pos[idx],
        })
        .collect();
    intervals.sort_by_key(|iv| (iv.start, iv.vreg.0));

    let mut peak_reg = func.params.len() as u8;
    for iv in intervals {
        // Expire any interval whose last use is <= iv.start (half-open at use site).
        let mut i = 0;
        while i < active.len() {
            if active[i].1 <= iv.start {
                let (_, _, freed) = active.swap_remove(i);
                free_regs.push(freed);
            } else {
                i += 1;
            }
        }
        free_regs.sort_by(|a, b| b.cmp(a));

        let assigned = if let Some(r) = free_regs.pop() {
            r
        } else {
            // Fallback: allocate from upper registers if below MAX_REGS
            let used_max = active.iter().map(|x| x.2).max().unwrap_or(0);
            (used_max + 1).min((MAX_REGS - 1) as u8)
        };
        reg_map[iv.vreg.0 as usize] = assigned;
        peak_reg = peak_reg.max(assigned + 1);
        active.push((iv.vreg, iv.end, assigned));
    }

    let total_regs = peak_reg.max((alloc_limit + arg_window).min(MAX_REGS) as u8);
    (reg_map, total_regs)
}

enum PatchKind {
    Jump,
    JumpIfFalse(u8),
    JumpIfTrue(u8),
}

struct PendingBranch {
    code_idx: usize,
    target_bb: BlockId,
    kind: PatchKind,
}

#[allow(clippy::too_many_arguments)]
fn emit_function(
    func: &IrFn,
    source_map: &SourceMap<'_>,
    constants: &mut Vec<ConstEntry>,
    const_dedup: &mut HashMap<(i32, ValType), u16>,
    code: &mut Vec<Instruction>,
    debug_lines: &mut Vec<DebugLine>,
) -> u8 {
    let rpo = compute_rpo(func);
    let (reg_map, num_regs) = allocate_registers(func, &rpo);
    let max_call_arity = func
        .blocks
        .iter()
        .flat_map(|b| b.insts.iter())
        .map(|inst| match inst {
            IrInst::Call { args, .. } | IrInst::Syscall { args, .. } => args.len(),
            _ => 0,
        })
        .max()
        .unwrap_or(0);
    let arg_base = MAX_REGS.saturating_sub(max_call_arity) as u8;

    let mut bb_offsets = vec![0usize; func.blocks.len()];
    let mut patches = Vec::new();
    let mut last_line: Option<u32> = None;

    let mut push_insn = |insn: Instruction, span_start: usize, code: &mut Vec<Instruction>| {
        let pc = code.len() as u32;
        if span_start > 0 {
            let line = source_map.line_of(span_start);
            if last_line != Some(line) {
                last_line = Some(line);
                debug_lines.push(DebugLine { pc, line });
            }
        }
        code.push(insn);
    };

    for (rpo_idx, &bb_id) in rpo.iter().enumerate() {
        bb_offsets[bb_id.0] = code.len();
        let bb = &func.blocks[bb_id.0];
        let next_bb = rpo.get(rpo_idx + 1).copied();

        for (i, inst) in bb.insts.iter().enumerate() {
            let sp = bb.spans.get(i).copied().unwrap_or(0);
            match inst {
                IrInst::Const { dst, val, ty } => {
                    let rd = reg_map[dst.0 as usize];
                    match ty {
                        ValType::Bool => {
                            push_insn(
                                Instruction::ab_imm(Opcode::LoadImmBool, rd, u16::from(*val != 0)),
                                sp,
                                code,
                            );
                        }
                        ValType::Int if i16::try_from(*val).is_ok() => {
                            push_insn(
                                Instruction::ab_simm(Opcode::LoadImmI, rd, *val as i16),
                                sp,
                                code,
                            );
                        }
                        _ => {
                            let idx = intern_const(constants, const_dedup, *val, *ty);
                            push_insn(Instruction::ab_imm(Opcode::LoadConst, rd, idx), sp, code);
                        }
                    }
                }
                IrInst::Copy { dst, src } => {
                    let rd = reg_map[dst.0 as usize];
                    let rs = reg_map[src.0 as usize];
                    if rd != rs {
                        push_insn(Instruction::abc(Opcode::Move, rd, rs, 0), sp, code);
                    }
                }
                IrInst::Unary { dst, op, src, ty } => {
                    let rd = reg_map[dst.0 as usize];
                    let rs = reg_map[src.0 as usize];
                    let opcode = match (op, ty) {
                        (UnaryOp::Neg, ValType::Int) => Opcode::NegI,
                        (UnaryOp::Neg, ValType::Fix) => Opcode::NegFix,
                        (UnaryOp::Not, ValType::Bool) => Opcode::NotBool,
                        (UnaryOp::BitNot, ValType::Int) => Opcode::BitNot,
                        _ => unreachable!("verified unary op"),
                    };
                    push_insn(Instruction::abc(opcode, rd, rs, 0), sp, code);
                }
                IrInst::Binary {
                    dst,
                    op,
                    lhs,
                    rhs,
                    operand_ty,
                    ..
                } => {
                    let rd = reg_map[dst.0 as usize];
                    let rl = reg_map[lhs.0 as usize];
                    let rr = reg_map[rhs.0 as usize];
                    let opcode = match (op, operand_ty) {
                        (BinaryOp::Add, ValType::Int) => Opcode::AddI,
                        (BinaryOp::Sub, ValType::Int) => Opcode::SubI,
                        (BinaryOp::Mul, ValType::Int) => Opcode::MulI,
                        (BinaryOp::Div, ValType::Int) => Opcode::DivI,
                        (BinaryOp::Mod, ValType::Int) => Opcode::ModI,
                        (BinaryOp::Add, ValType::Fix) => Opcode::AddFix,
                        (BinaryOp::Sub, ValType::Fix) => Opcode::SubFix,
                        (BinaryOp::Mul, ValType::Fix) => Opcode::MulFix,
                        (BinaryOp::Div, ValType::Fix) => Opcode::DivFix,
                        (BinaryOp::BitAnd, ValType::Int) => Opcode::BitAnd,
                        (BinaryOp::BitOr, ValType::Int) => Opcode::BitOr,
                        (BinaryOp::BitXor, ValType::Int) => Opcode::BitXor,
                        (BinaryOp::Shl, ValType::Int) => Opcode::Shl,
                        (BinaryOp::Shr, ValType::Int) => Opcode::Shr,
                        (BinaryOp::Eq, ValType::Int) => Opcode::EqI,
                        (BinaryOp::Ne, ValType::Int) => Opcode::NeI,
                        (BinaryOp::Lt, ValType::Int) => Opcode::LtI,
                        (BinaryOp::Le, ValType::Int) => Opcode::LeI,
                        (BinaryOp::Gt, ValType::Int) => Opcode::GtI,
                        (BinaryOp::Ge, ValType::Int) => Opcode::GeI,
                        (BinaryOp::Eq, ValType::Fix) => Opcode::EqFix,
                        (BinaryOp::Ne, ValType::Fix) => Opcode::NeFix,
                        (BinaryOp::Lt, ValType::Fix) => Opcode::LtFix,
                        (BinaryOp::Le, ValType::Fix) => Opcode::LeFix,
                        (BinaryOp::Gt, ValType::Fix) => Opcode::GtFix,
                        (BinaryOp::Ge, ValType::Fix) => Opcode::GeFix,
                        (BinaryOp::Eq, ValType::Bool) => Opcode::EqBool,
                        (BinaryOp::Ne, ValType::Bool) => Opcode::NeBool,
                        _ => unreachable!("invalid binary op in codegen"),
                    };
                    push_insn(Instruction::abc(opcode, rd, rl, rr), sp, code);
                }
                IrInst::Cast { dst, src, to } => {
                    let rd = reg_map[dst.0 as usize];
                    let rs = reg_map[src.0 as usize];
                    let opcode = match to {
                        ValType::Fix => Opcode::ToFix,
                        ValType::Int => Opcode::ToInt,
                        ValType::Bool => unreachable!(),
                    };
                    push_insn(Instruction::abc(opcode, rd, rs, 0), sp, code);
                }
                IrInst::LoadState { dst, slot } => {
                    let rd = reg_map[dst.0 as usize];
                    push_insn(Instruction::ab_imm(Opcode::LoadState, rd, *slot), sp, code);
                }
                IrInst::StoreState { slot, src } => {
                    let rs = reg_map[src.0 as usize];
                    push_insn(Instruction::ab_imm(Opcode::StoreState, rs, *slot), sp, code);
                }
                IrInst::LoadStateArr { dst, arr_id, idx } => {
                    let rd = reg_map[dst.0 as usize];
                    let ri = reg_map[idx.0 as usize];
                    push_insn(
                        Instruction::abc(Opcode::LoadStateArr, rd, *arr_id, ri),
                        sp,
                        code,
                    );
                }
                IrInst::StoreStateArr { arr_id, idx, src } => {
                    let rs = reg_map[src.0 as usize];
                    let ri = reg_map[idx.0 as usize];
                    push_insn(
                        Instruction::abc(Opcode::StoreStateArr, rs, *arr_id, ri),
                        sp,
                        code,
                    );
                }
                IrInst::Call { dst, func_id, args } => {
                    for (idx, a) in args.iter().enumerate() {
                        let ra = reg_map[a.0 as usize];
                        let staging = arg_base + idx as u8;
                        if ra != staging {
                            push_insn(Instruction::abc(Opcode::Move, staging, ra, 0), sp, code);
                        }
                    }
                    let rd = dst.map_or(0, |d| reg_map[d.0 as usize]);
                    let base = if args.is_empty() { 0 } else { arg_base };
                    push_insn(
                        Instruction::abc(Opcode::Call, rd, *func_id as u8, base),
                        sp,
                        code,
                    );
                }
                IrInst::Syscall { dst, id, args } => {
                    for (idx, a) in args.iter().enumerate() {
                        let ra = reg_map[a.0 as usize];
                        let staging = arg_base + idx as u8;
                        if ra != staging {
                            push_insn(Instruction::abc(Opcode::Move, staging, ra, 0), sp, code);
                        }
                    }
                    let rd = dst.map_or(0, |d| reg_map[d.0 as usize]);
                    let base = if args.is_empty() { 0 } else { arg_base };
                    push_insn(
                        Instruction::abc(Opcode::Syscall, rd, *id as u8, base),
                        sp,
                        code,
                    );
                }
            }
        }

        let tsp = bb.term_span;
        match bb.term {
            Terminator::Jump(target) => {
                if next_bb != Some(target) {
                    patches.push(PendingBranch {
                        code_idx: code.len(),
                        target_bb: target,
                        kind: PatchKind::Jump,
                    });
                    push_insn(Instruction::ab_simm(Opcode::Jump, 0, 0), tsp, code);
                }
            }
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                let rc = reg_map[cond.0 as usize];
                if next_bb == Some(then_bb) {
                    patches.push(PendingBranch {
                        code_idx: code.len(),
                        target_bb: else_bb,
                        kind: PatchKind::JumpIfFalse(rc),
                    });
                    push_insn(Instruction::ab_simm(Opcode::JumpIfFalse, rc, 0), tsp, code);
                } else {
                    patches.push(PendingBranch {
                        code_idx: code.len(),
                        target_bb: then_bb,
                        kind: PatchKind::JumpIfTrue(rc),
                    });
                    push_insn(Instruction::ab_simm(Opcode::JumpIfTrue, rc, 0), tsp, code);
                    if next_bb != Some(else_bb) {
                        patches.push(PendingBranch {
                            code_idx: code.len(),
                            target_bb: else_bb,
                            kind: PatchKind::Jump,
                        });
                        push_insn(Instruction::ab_simm(Opcode::Jump, 0, 0), tsp, code);
                    }
                }
            }
            Terminator::Return(Some(v)) => {
                let rv = reg_map[v.0 as usize];
                push_insn(Instruction::abc(Opcode::Ret, rv, 1, 0), tsp, code);
            }
            Terminator::Return(None) => {
                push_insn(Instruction::abc(Opcode::Ret, 0, 0, 0), tsp, code);
            }
            Terminator::Unreachable => {
                push_insn(Instruction::abc(Opcode::Trap, 0, 0, 0), tsp, code);
            }
        }
    }

    for patch in patches {
        let target_pc = bb_offsets[patch.target_bb.0] as isize;
        let cur_pc = patch.code_idx as isize;
        let rel = (target_pc - cur_pc) as i16;
        code[patch.code_idx] = match patch.kind {
            PatchKind::Jump => Instruction::ab_simm(Opcode::Jump, 0, rel),
            PatchKind::JumpIfFalse(r) => Instruction::ab_simm(Opcode::JumpIfFalse, r, rel),
            PatchKind::JumpIfTrue(r) => Instruction::ab_simm(Opcode::JumpIfTrue, r, rel),
        };
    }

    num_regs
}

fn intern_const(
    constants: &mut Vec<ConstEntry>,
    dedup: &mut HashMap<(i32, ValType), u16>,
    value: i32,
    ty: ValType,
) -> u16 {
    if let Some(&idx) = dedup.get(&(value, ty)) {
        return idx;
    }
    let idx = constants.len() as u16;
    constants.push(ConstEntry { value, ty });
    dedup.insert((value, ty), idx);
    idx
}
