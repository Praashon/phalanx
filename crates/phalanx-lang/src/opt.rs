//! IR optimization passes: constant folding, algebraic identities, copy propagation,
//! unreachable block pruning, and dead-code elimination.

use std::collections::{HashMap, VecDeque};

use phalanx_vm::ValType;

use crate::ast::{BinaryOp, UnaryOp};
use crate::ir::{BasicBlock, BlockId, IrFn, IrInst, IrProgram, Terminator, VReg};

/// Optimization level selected by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    /// Lower IR directly without optimizations.
    None,
    /// Run all fixed-point scalar and CFG optimization passes.
    #[default]
    Standard,
}

/// Metrics collected by the IR optimizer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OptStats {
    /// Constant expressions folded at compile time.
    pub consts_folded: usize,
    /// Algebraic identities simplified (`x + 0`, `x * 1`, etc.).
    pub algebraic_simplifications: usize,
    /// Redundant `Copy` assignments eliminated.
    pub copies_propagated: usize,
    /// Unused pure instructions removed.
    pub dead_insts_removed: usize,
    /// Unreachable basic blocks pruned.
    pub dead_blocks_removed: usize,
}

/// Runs configured optimization passes over `prog` and returns summary metrics.
pub fn optimize(prog: &mut IrProgram, level: OptLevel) -> OptStats {
    let mut stats = OptStats::default();
    for func in &mut prog.functions {
        if level == OptLevel::Standard {
            optimize_fn(func, &mut stats);
        } else {
            prune_unreachable_blocks(func, &mut OptStats::default());
        }
    }
    stats
}

fn optimize_fn(func: &mut IrFn, stats: &mut OptStats) {
    for _ in 0..8 {
        let before = *stats;
        prune_unreachable_blocks(func, stats);
        fold_and_simplify(func, stats);
        propagate_copies(func, stats);
        dce(func, stats);
        prune_unreachable_blocks(func, stats);
        if *stats == before {
            break;
        }
    }
}

fn compute_def_counts(func: &IrFn) -> Vec<u32> {
    let mut counts = vec![0u32; func.vreg_types.len()];
    for &(param, _) in &func.params {
        counts[param.0 as usize] += 1;
    }
    for bb in &func.blocks {
        for inst in &bb.insts {
            if let Some(d) = inst_dst(inst) {
                counts[d.0 as usize] += 1;
            }
        }
    }
    counts
}

fn fold_and_simplify(func: &mut IrFn, stats: &mut OptStats) {
    let def_counts = compute_def_counts(func);
    let mut known_consts: HashMap<VReg, (i32, ValType)> = HashMap::new();
    for bb in &func.blocks {
        for inst in &bb.insts {
            if let IrInst::Const { dst, val, ty } = inst
                && def_counts[dst.0 as usize] == 1
            {
                known_consts.insert(*dst, (*val, *ty));
            }
        }
    }

    for bb in &mut func.blocks {
        for inst in &mut bb.insts {
            match inst.clone() {
                IrInst::Copy { dst, src } => {
                    if let Some(&(val, ty)) = known_consts.get(&src) {
                        *inst = IrInst::Const { dst, val, ty };
                        if def_counts[dst.0 as usize] == 1 {
                            known_consts.insert(dst, (val, ty));
                        }
                        stats.consts_folded += 1;
                    }
                }
                IrInst::Unary { dst, op, src, ty } => {
                    if let Some(&(val, _)) = known_consts.get(&src)
                        && let Some(folded) = eval_unary(op, val, ty)
                    {
                        *inst = IrInst::Const {
                            dst,
                            val: folded,
                            ty,
                        };
                        if def_counts[dst.0 as usize] == 1 {
                            known_consts.insert(dst, (folded, ty));
                        }
                        stats.consts_folded += 1;
                    }
                }
                IrInst::Cast { dst, src, to } => {
                    if let Some(&(val, _)) = known_consts.get(&src) {
                        let folded = match to {
                            ValType::Fix => (i64::from(val) << 16)
                                .clamp(i64::from(i32::MIN), i64::from(i32::MAX))
                                as i32,
                            ValType::Int => val >> 16,
                            ValType::Bool => val,
                        };
                        *inst = IrInst::Const {
                            dst,
                            val: folded,
                            ty: to,
                        };
                        if def_counts[dst.0 as usize] == 1 {
                            known_consts.insert(dst, (folded, to));
                        }
                        stats.consts_folded += 1;
                    }
                }
                IrInst::Binary {
                    dst,
                    op,
                    lhs,
                    rhs,
                    operand_ty,
                    result_ty,
                } => {
                    let lc = known_consts.get(&lhs).copied();
                    let rc = known_consts.get(&rhs).copied();
                    if let (Some((lv, _)), Some((rv, _))) = (lc, rc)
                        && let Some(folded) = eval_binary(op, lv, rv, operand_ty)
                    {
                        *inst = IrInst::Const {
                            dst,
                            val: folded,
                            ty: result_ty,
                        };
                        if def_counts[dst.0 as usize] == 1 {
                            known_consts.insert(dst, (folded, result_ty));
                        }
                        stats.consts_folded += 1;
                        continue;
                    }

                    // Algebraic identities
                    if let Some(simplified) = simplify_algebra(AlgebraCtx {
                        dst,
                        op,
                        lhs,
                        rhs,
                        lc: lc.map(|x| x.0),
                        rc: rc.map(|x| x.0),
                        operand_ty,
                        result_ty,
                    }) {
                        *inst = simplified;
                        stats.algebraic_simplifications += 1;
                    }
                }
                _ => {}
            }
        }

        if let Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } = bb.term
        {
            if then_bb == else_bb {
                bb.term = Terminator::Jump(then_bb);
                stats.algebraic_simplifications += 1;
            } else if let Some(&(val, _)) = known_consts.get(&cond) {
                bb.term = Terminator::Jump(if val != 0 { then_bb } else { else_bb });
                stats.consts_folded += 1;
            }
        }
    }
}

struct AlgebraCtx {
    dst: VReg,
    op: BinaryOp,
    lhs: VReg,
    rhs: VReg,
    lc: Option<i32>,
    rc: Option<i32>,
    operand_ty: ValType,
    result_ty: ValType,
}

fn simplify_algebra(ctx: AlgebraCtx) -> Option<IrInst> {
    let AlgebraCtx {
        dst,
        op,
        lhs,
        rhs,
        lc,
        rc,
        operand_ty,
        result_ty,
    } = ctx;
    let one = match operand_ty {
        ValType::Int => 1,
        ValType::Fix => 65536,
        ValType::Bool => 1,
    };
    match op {
        BinaryOp::Add => {
            if rc == Some(0) {
                return Some(IrInst::Copy { dst, src: lhs });
            }
            if lc == Some(0) {
                return Some(IrInst::Copy { dst, src: rhs });
            }
        }
        BinaryOp::Sub => {
            if rc == Some(0) {
                return Some(IrInst::Copy { dst, src: lhs });
            }
            if lhs == rhs && operand_ty == ValType::Int {
                return Some(IrInst::Const {
                    dst,
                    val: 0,
                    ty: result_ty,
                });
            }
        }
        BinaryOp::Mul => {
            if rc == Some(one) {
                return Some(IrInst::Copy { dst, src: lhs });
            }
            if lc == Some(one) {
                return Some(IrInst::Copy { dst, src: rhs });
            }
            if rc == Some(0) || lc == Some(0) {
                return Some(IrInst::Const {
                    dst,
                    val: 0,
                    ty: result_ty,
                });
            }
        }
        BinaryOp::Div => {
            if rc == Some(one) {
                return Some(IrInst::Copy { dst, src: lhs });
            }
        }
        BinaryOp::BitAnd => {
            if rc == Some(0) || lc == Some(0) {
                return Some(IrInst::Const {
                    dst,
                    val: 0,
                    ty: result_ty,
                });
            }
        }
        BinaryOp::BitOr | BinaryOp::BitXor | BinaryOp::Shl | BinaryOp::Shr if rc == Some(0) => {
            return Some(IrInst::Copy { dst, src: lhs });
        }
        _ => {}
    }
    None
}

fn propagate_copies(func: &mut IrFn, stats: &mut OptStats) {
    let def_counts = compute_def_counts(func);
    let mut copy_map: HashMap<VReg, VReg> = HashMap::new();
    for bb in &func.blocks {
        for inst in &bb.insts {
            if let IrInst::Copy { dst, src } = inst
                && dst != src
                && def_counts[dst.0 as usize] == 1
                && def_counts[src.0 as usize] == 1
            {
                copy_map.insert(*dst, *src);
            }
        }
    }
    if copy_map.is_empty() {
        return;
    }

    let resolve = |mut v: VReg| -> VReg {
        let mut steps = 0;
        while let Some(&next) = copy_map.get(&v) {
            if next == v || steps > 32 {
                break;
            }
            v = next;
            steps += 1;
        }
        v
    };

    for bb in &mut func.blocks {
        for inst in &mut bb.insts {
            visit_inst_uses_mut(inst, &mut |r| {
                let resolved = resolve(*r);
                if resolved != *r {
                    *r = resolved;
                    stats.copies_propagated += 1;
                }
            });
        }
        visit_term_uses_mut(&mut bb.term, &mut |r| {
            let resolved = resolve(*r);
            if resolved != *r {
                *r = resolved;
                stats.copies_propagated += 1;
            }
        });
    }
}

fn dce(func: &mut IrFn, stats: &mut OptStats) {
    let mut use_counts = vec![0u32; func.vreg_types.len()];
    for bb in &func.blocks {
        for inst in &bb.insts {
            visit_inst_uses(inst, &mut |r| {
                use_counts[r.0 as usize] += 1;
            });
        }
        visit_term_uses(&bb.term, &mut |r| {
            use_counts[r.0 as usize] += 1;
        });
    }

    for bb in &mut func.blocks {
        let mut idx = 0usize;
        while idx < bb.insts.len() {
            let remove = is_pure(&bb.insts[idx])
                && inst_dst(&bb.insts[idx]).is_some_and(|d| use_counts[d.0 as usize] == 0);
            if remove {
                bb.insts.remove(idx);
                bb.spans.remove(idx);
                stats.dead_insts_removed += 1;
            } else {
                idx += 1;
            }
        }
    }
}

fn prune_unreachable_blocks(func: &mut IrFn, stats: &mut OptStats) {
    if func.blocks.is_empty() {
        return;
    }
    let mut reachable = vec![false; func.blocks.len()];
    let mut queue = VecDeque::new();
    reachable[0] = true;
    queue.push_back(0usize);

    while let Some(b) = queue.pop_front() {
        for succ in func.blocks[b].term.successors() {
            if succ.0 < func.blocks.len() && !reachable[succ.0] {
                reachable[succ.0] = true;
                queue.push_back(succ.0);
            }
        }
    }

    let unreachable_count = reachable.iter().filter(|&&r| !r).count();
    if unreachable_count == 0 {
        return;
    }
    stats.dead_blocks_removed += unreachable_count;

    let mut remap = vec![BlockId(0); func.blocks.len()];
    let mut new_blocks: Vec<BasicBlock> = Vec::with_capacity(func.blocks.len() - unreachable_count);
    for (old_idx, bb) in func.blocks.drain(..).enumerate() {
        if reachable[old_idx] {
            remap[old_idx] = BlockId(new_blocks.len());
            new_blocks.push(bb);
        }
    }

    for bb in &mut new_blocks {
        match &mut bb.term {
            Terminator::Jump(t) => *t = remap[t.0],
            Terminator::Branch {
                then_bb, else_bb, ..
            } => {
                *then_bb = remap[then_bb.0];
                *else_bb = remap[else_bb.0];
            }
            Terminator::Return(_) | Terminator::Unreachable => {}
        }
    }
    func.blocks = new_blocks;
}

fn is_pure(inst: &IrInst) -> bool {
    match inst {
        IrInst::Const { .. }
        | IrInst::Copy { .. }
        | IrInst::Unary { .. }
        | IrInst::Cast { .. }
        | IrInst::LoadState { .. } => true,
        IrInst::Binary { op, .. } => !matches!(op, BinaryOp::Div | BinaryOp::Mod),
        IrInst::StoreState { .. }
        | IrInst::LoadStateArr { .. }
        | IrInst::StoreStateArr { .. }
        | IrInst::Call { .. }
        | IrInst::Syscall { .. } => false,
    }
}

pub(crate) fn inst_dst(inst: &IrInst) -> Option<VReg> {
    match inst {
        IrInst::Const { dst, .. }
        | IrInst::Copy { dst, .. }
        | IrInst::Unary { dst, .. }
        | IrInst::Binary { dst, .. }
        | IrInst::Cast { dst, .. }
        | IrInst::LoadState { dst, .. }
        | IrInst::LoadStateArr { dst, .. } => Some(*dst),
        IrInst::Call { dst, .. } | IrInst::Syscall { dst, .. } => *dst,
        IrInst::StoreState { .. } | IrInst::StoreStateArr { .. } => None,
    }
}

pub(crate) fn visit_inst_uses(inst: &IrInst, f: &mut impl FnMut(VReg)) {
    match inst {
        IrInst::Const { .. } | IrInst::LoadState { .. } => {}
        IrInst::Copy { src, .. }
        | IrInst::Unary { src, .. }
        | IrInst::Cast { src, .. }
        | IrInst::StoreState { src, .. } => f(*src),
        IrInst::Binary { lhs, rhs, .. } => {
            f(*lhs);
            f(*rhs);
        }
        IrInst::LoadStateArr { idx, .. } => f(*idx),
        IrInst::StoreStateArr { idx, src, .. } => {
            f(*idx);
            f(*src);
        }
        IrInst::Call { args, .. } | IrInst::Syscall { args, .. } => {
            for &a in args {
                f(a);
            }
        }
    }
}

fn visit_inst_uses_mut(inst: &mut IrInst, f: &mut impl FnMut(&mut VReg)) {
    match inst {
        IrInst::Const { .. } | IrInst::LoadState { .. } => {}
        IrInst::Copy { src, .. }
        | IrInst::Unary { src, .. }
        | IrInst::Cast { src, .. }
        | IrInst::StoreState { src, .. } => f(src),
        IrInst::Binary { lhs, rhs, .. } => {
            f(lhs);
            f(rhs);
        }
        IrInst::LoadStateArr { idx, .. } => f(idx),
        IrInst::StoreStateArr { idx, src, .. } => {
            f(idx);
            f(src);
        }
        IrInst::Call { args, .. } | IrInst::Syscall { args, .. } => {
            for a in args {
                f(a);
            }
        }
    }
}

pub(crate) fn visit_term_uses(term: &Terminator, f: &mut impl FnMut(VReg)) {
    match term {
        Terminator::Branch { cond, .. } => f(*cond),
        Terminator::Return(Some(v)) => f(*v),
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => {}
    }
}

fn visit_term_uses_mut(term: &mut Terminator, f: &mut impl FnMut(&mut VReg)) {
    match term {
        Terminator::Branch { cond, .. } => f(cond),
        Terminator::Return(Some(v)) => f(v),
        Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => {}
    }
}

fn eval_unary(op: UnaryOp, val: i32, ty: ValType) -> Option<i32> {
    match (op, ty) {
        (UnaryOp::Neg, ValType::Int) => Some(val.wrapping_neg()),
        (UnaryOp::Neg, ValType::Fix) => Some(val.saturating_neg()),
        (UnaryOp::Not, ValType::Bool) => Some(i32::from(val == 0)),
        (UnaryOp::BitNot, ValType::Int) => Some(!val),
        _ => None,
    }
}

fn eval_binary(op: BinaryOp, lv: i32, rv: i32, ty: ValType) -> Option<i32> {
    match (op, ty) {
        (BinaryOp::Add, ValType::Int) => Some(lv.wrapping_add(rv)),
        (BinaryOp::Sub, ValType::Int) => Some(lv.wrapping_sub(rv)),
        (BinaryOp::Mul, ValType::Int) => Some(lv.wrapping_mul(rv)),
        (BinaryOp::Div, ValType::Int) => (rv != 0).then(|| lv.wrapping_div(rv)),
        (BinaryOp::Mod, ValType::Int) => (rv != 0).then(|| lv.wrapping_rem(rv)),
        (BinaryOp::Add, ValType::Fix) => Some(lv.saturating_add(rv)),
        (BinaryOp::Sub, ValType::Fix) => Some(lv.saturating_sub(rv)),
        (BinaryOp::Mul, ValType::Fix) => {
            let p = (i64::from(lv) * i64::from(rv)) >> 16;
            Some(p.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
        }
        (BinaryOp::Div, ValType::Fix) => (rv != 0).then(|| {
            let q = (i64::from(lv) << 16) / i64::from(rv);
            q.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        }),
        (BinaryOp::BitAnd, ValType::Int) => Some(lv & rv),
        (BinaryOp::BitOr, ValType::Int) => Some(lv | rv),
        (BinaryOp::BitXor, ValType::Int) => Some(lv ^ rv),
        (BinaryOp::Shl, ValType::Int) => Some(lv.wrapping_shl((rv as u32) & 31)),
        (BinaryOp::Shr, ValType::Int) => Some(lv.wrapping_shr((rv as u32) & 31)),
        (BinaryOp::Eq, _) => Some(i32::from(lv == rv)),
        (BinaryOp::Ne, _) => Some(i32::from(lv != rv)),
        (BinaryOp::Lt, ValType::Int | ValType::Fix) => Some(i32::from(lv < rv)),
        (BinaryOp::Le, ValType::Int | ValType::Fix) => Some(i32::from(lv <= rv)),
        (BinaryOp::Gt, ValType::Int | ValType::Fix) => Some(i32::from(lv > rv)),
        (BinaryOp::Ge, ValType::Int | ValType::Fix) => Some(i32::from(lv >= rv)),
        _ => None,
    }
}
