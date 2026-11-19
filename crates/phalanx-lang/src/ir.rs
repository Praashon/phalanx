//! Basic-block CFG Intermediate Representation and AST lowering.

use core::fmt::Write;
use std::collections::HashMap;

use phalanx_vm::{SyscallId, SyscallSpec, ValType};

use crate::ast::{AssignOp, BinaryOp, UnaryOp};
use crate::typecheck::{
    CallTarget, LocalId, TypedAssignTarget, TypedExpr, TypedExprKind, TypedFn, TypedProgram,
    TypedStmt,
};

/// Virtual register identifier in IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VReg(pub u32);

/// Basic block index within an [`IrFn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

/// Complete lowered IR module.
#[derive(Debug, Clone)]
pub struct IrProgram {
    /// Persistent state slot types.
    pub state_layout: Vec<ValType>,
    /// Persistent state slot initial values.
    pub state_init: Vec<i32>,
    /// Persistent state slot names.
    pub state_names: Vec<String>,
    /// Persistent state arrays `(name, base, len, elem_ty)`.
    pub state_arrays: Vec<(String, u16, u16, ValType)>,
    /// Entry function index.
    pub entry_func: u16,
    /// Lowered IR functions.
    pub functions: Vec<IrFn>,
}

/// Control-flow graph function in IR form.
#[derive(Debug, Clone)]
pub struct IrFn {
    /// Function name.
    pub name: String,
    /// Parameter virtual registers and types.
    pub params: Vec<(VReg, ValType)>,
    /// Optional return type.
    pub ret: Option<ValType>,
    /// Types of all allocated virtual registers indexed by `VReg::0`.
    pub vreg_types: Vec<ValType>,
    /// Ordered basic blocks (`blocks[0]` is function entry).
    pub blocks: Vec<BasicBlock>,
}

/// Single basic block containing linear instructions followed by a terminator.
#[derive(Debug, Clone)]
pub struct BasicBlock {
    /// Non-branching instructions in execution order.
    pub insts: Vec<IrInst>,
    /// Source byte offset associated with each instruction in `insts`.
    pub spans: Vec<usize>,
    /// Block terminator controlling outgoing edges.
    pub term: Terminator,
    /// Source byte offset for the terminator.
    pub term_span: usize,
}

/// Non-branching IR instruction.
#[derive(Debug, Clone)]
pub enum IrInst {
    /// `dst = imm`
    Const {
        /// Destination virtual register.
        dst: VReg,
        /// Immediate 32-bit bits.
        val: i32,
        /// Static value type.
        ty: ValType,
    },
    /// `dst = src`
    Copy {
        /// Destination virtual register.
        dst: VReg,
        /// Source virtual register.
        src: VReg,
    },
    /// `dst = op src`
    Unary {
        /// Destination virtual register.
        dst: VReg,
        /// Operator.
        op: UnaryOp,
        /// Operand virtual register.
        src: VReg,
        /// Operand/result type.
        ty: ValType,
    },
    /// `dst = lhs op rhs`
    Binary {
        /// Destination virtual register.
        dst: VReg,
        /// Operator (never `And`/`Or` since short-circuiting lowers to CFG branches).
        op: BinaryOp,
        /// Left-hand virtual register.
        lhs: VReg,
        /// Right-hand virtual register.
        rhs: VReg,
        /// Operand type (`Int`, `Fix`, or `Bool`).
        operand_ty: ValType,
        /// Result type.
        result_ty: ValType,
    },
    /// Explicit conversion (`to_fix` or `to_int`).
    Cast {
        /// Destination virtual register.
        dst: VReg,
        /// Source virtual register.
        src: VReg,
        /// Target type (`Fix` or `Int`).
        to: ValType,
    },
    /// `dst = state[slot]`
    LoadState {
        /// Destination virtual register.
        dst: VReg,
        /// State slot index.
        slot: u16,
    },
    /// `state[slot] = src`
    StoreState {
        /// State slot index.
        slot: u16,
        /// Source virtual register.
        src: VReg,
    },
    /// `dst = state_array[arr_id][idx]`
    LoadStateArr {
        /// Destination virtual register.
        dst: VReg,
        /// Array descriptor index.
        arr_id: u8,
        /// Integer index virtual register.
        idx: VReg,
    },
    /// `state_array[arr_id][idx] = src`
    StoreStateArr {
        /// Array descriptor index.
        arr_id: u8,
        /// Integer index virtual register.
        idx: VReg,
        /// Source value virtual register.
        src: VReg,
    },
    /// User function call (`dst` is `Some` iff callee returns a value).
    Call {
        /// Optional destination virtual register.
        dst: Option<VReg>,
        /// Target function index.
        func_id: u16,
        /// Argument virtual registers.
        args: Vec<VReg>,
    },
    /// Host syscall (`dst` is `Some` iff syscall returns a value).
    Syscall {
        /// Optional destination virtual register.
        dst: Option<VReg>,
        /// Host syscall identifier.
        id: SyscallId,
        /// Argument virtual registers.
        args: Vec<VReg>,
    },
}

/// Basic block control-flow terminator.
#[derive(Debug, Clone)]
pub enum Terminator {
    /// Unconditional jump.
    Jump(BlockId),
    /// Conditional branch on boolean virtual register.
    Branch {
        /// Boolean condition register.
        cond: VReg,
        /// Target if `cond != 0`.
        then_bb: BlockId,
        /// Target if `cond == 0`.
        else_bb: BlockId,
    },
    /// Return from function.
    Return(Option<VReg>),
    /// Unterminated placeholder during IR construction (replaced by `Return` or `Trap`).
    Unreachable,
}

impl Terminator {
    /// Returns outgoing successor block IDs.
    #[must_use]
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Self::Jump(b) => vec![*b],
            Self::Branch {
                then_bb, else_bb, ..
            } => {
                if then_bb == else_bb {
                    vec![*then_bb]
                } else {
                    vec![*then_bb, *else_bb]
                }
            }
            Self::Return(_) | Self::Unreachable => vec![],
        }
    }
}

/// Lowers a [`TypedProgram`] into basic-block [`IrProgram`].
#[must_use]
pub fn lower(prog: &TypedProgram) -> IrProgram {
    let functions = prog.functions.iter().map(lower_fn).collect();
    IrProgram {
        state_layout: prog.state_layout.clone(),
        state_init: prog.state_init.clone(),
        state_names: prog.state_names.clone(),
        state_arrays: prog.state_arrays.clone(),
        entry_func: prog.entry_func,
        functions,
    }
}

struct LoopTargets {
    continue_bb: BlockId,
    break_bb: BlockId,
}

struct Lowerer {
    vreg_types: Vec<ValType>,
    local_map: HashMap<LocalId, VReg>,
    blocks: Vec<BasicBlock>,
    cur_bb: BlockId,
    loops: Vec<LoopTargets>,
}

impl Lowerer {
    fn new_vreg(&mut self, ty: ValType) -> VReg {
        let id = VReg(self.vreg_types.len() as u32);
        self.vreg_types.push(ty);
        id
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(BasicBlock {
            insts: Vec::new(),
            spans: Vec::new(),
            term: Terminator::Unreachable,
            term_span: 0,
        });
        id
    }

    fn current_terminated(&self) -> bool {
        !matches!(self.blocks[self.cur_bb.0].term, Terminator::Unreachable)
    }

    fn emit(&mut self, inst: IrInst, span_start: usize) {
        if !self.current_terminated() {
            let bb = &mut self.blocks[self.cur_bb.0];
            bb.insts.push(inst);
            bb.spans.push(span_start);
        }
    }

    fn terminate(&mut self, term: Terminator, span_start: usize) {
        if !self.current_terminated() {
            let bb = &mut self.blocks[self.cur_bb.0];
            bb.term = term;
            bb.term_span = span_start;
        }
    }

    fn lower_stmts(&mut self, stmts: &[TypedStmt]) {
        for stmt in stmts {
            if self.current_terminated() {
                break;
            }
            self.lower_stmt(stmt);
        }
    }

    fn lower_stmt(&mut self, stmt: &TypedStmt) {
        match stmt {
            TypedStmt::Let {
                local,
                ty,
                init,
                span,
            } => {
                let val_reg = self.lower_expr(init).expect("let init has value");
                let dst = self.new_vreg(*ty);
                self.local_map.insert(*local, dst);
                self.emit(IrInst::Copy { dst, src: val_reg }, span.start);
            }
            TypedStmt::Assign {
                target,
                op,
                ty,
                value,
                span,
            } => {
                let rhs = self.lower_expr(value).expect("assign rhs has value");
                match target {
                    TypedAssignTarget::Local(local) => {
                        let dst = self.local_map[local];
                        if *op == AssignOp::Set {
                            self.emit(IrInst::Copy { dst, src: rhs }, span.start);
                        } else {
                            let bin = assign_op_to_binary(*op);
                            let tmp = self.new_vreg(*ty);
                            self.emit(
                                IrInst::Binary {
                                    dst: tmp,
                                    op: bin,
                                    lhs: dst,
                                    rhs,
                                    operand_ty: *ty,
                                    result_ty: *ty,
                                },
                                span.start,
                            );
                            self.emit(IrInst::Copy { dst, src: tmp }, span.start);
                        }
                    }
                    TypedAssignTarget::StateScalar(slot) => {
                        if *op == AssignOp::Set {
                            self.emit(
                                IrInst::StoreState {
                                    slot: *slot,
                                    src: rhs,
                                },
                                span.start,
                            );
                        } else {
                            let cur = self.new_vreg(*ty);
                            self.emit(
                                IrInst::LoadState {
                                    dst: cur,
                                    slot: *slot,
                                },
                                span.start,
                            );
                            let tmp = self.new_vreg(*ty);
                            self.emit(
                                IrInst::Binary {
                                    dst: tmp,
                                    op: assign_op_to_binary(*op),
                                    lhs: cur,
                                    rhs,
                                    operand_ty: *ty,
                                    result_ty: *ty,
                                },
                                span.start,
                            );
                            self.emit(
                                IrInst::StoreState {
                                    slot: *slot,
                                    src: tmp,
                                },
                                span.start,
                            );
                        }
                    }
                    TypedAssignTarget::StateArray { arr_id, index } => {
                        let idx_reg = self.lower_expr(index).expect("index has i32");
                        if *op == AssignOp::Set {
                            self.emit(
                                IrInst::StoreStateArr {
                                    arr_id: *arr_id,
                                    idx: idx_reg,
                                    src: rhs,
                                },
                                span.start,
                            );
                        } else {
                            let cur = self.new_vreg(*ty);
                            self.emit(
                                IrInst::LoadStateArr {
                                    dst: cur,
                                    arr_id: *arr_id,
                                    idx: idx_reg,
                                },
                                span.start,
                            );
                            let tmp = self.new_vreg(*ty);
                            self.emit(
                                IrInst::Binary {
                                    dst: tmp,
                                    op: assign_op_to_binary(*op),
                                    lhs: cur,
                                    rhs,
                                    operand_ty: *ty,
                                    result_ty: *ty,
                                },
                                span.start,
                            );
                            self.emit(
                                IrInst::StoreStateArr {
                                    arr_id: *arr_id,
                                    idx: idx_reg,
                                    src: tmp,
                                },
                                span.start,
                            );
                        }
                    }
                }
            }
            TypedStmt::If {
                cond,
                then_stmts,
                else_stmts,
                span,
            } => {
                let cond_reg = self.lower_expr(cond).expect("if cond has bool");
                let then_bb = self.new_block();
                let else_bb = self.new_block();
                let join_bb = self.new_block();

                self.terminate(
                    Terminator::Branch {
                        cond: cond_reg,
                        then_bb,
                        else_bb,
                    },
                    span.start,
                );

                self.cur_bb = then_bb;
                self.lower_stmts(then_stmts);
                let then_fell = !self.current_terminated();
                if then_fell {
                    self.terminate(Terminator::Jump(join_bb), span.end);
                }

                self.cur_bb = else_bb;
                if let Some(stmts) = else_stmts {
                    self.lower_stmts(stmts);
                }
                let else_fell = !self.current_terminated();
                if else_fell {
                    self.terminate(Terminator::Jump(join_bb), span.end);
                }

                self.cur_bb = join_bb;
            }
            TypedStmt::While { cond, body, span } => {
                let head_bb = self.new_block();
                let body_bb = self.new_block();
                let exit_bb = self.new_block();

                self.terminate(Terminator::Jump(head_bb), span.start);

                self.cur_bb = head_bb;
                let cond_reg = self.lower_expr(cond).expect("while cond has bool");
                self.terminate(
                    Terminator::Branch {
                        cond: cond_reg,
                        then_bb: body_bb,
                        else_bb: exit_bb,
                    },
                    span.start,
                );

                self.loops.push(LoopTargets {
                    continue_bb: head_bb,
                    break_bb: exit_bb,
                });
                self.cur_bb = body_bb;
                self.lower_stmts(body);
                if !self.current_terminated() {
                    self.terminate(Terminator::Jump(head_bb), span.end);
                }
                self.loops.pop();

                self.cur_bb = exit_bb;
            }
            TypedStmt::For {
                var,
                start,
                end,
                body,
                span,
            } => {
                let start_reg = self.lower_expr(start).expect("for start has i32");
                let end_reg = self.lower_expr(end).expect("for end has i32");
                let idx_reg = self.new_vreg(ValType::Int);
                self.local_map.insert(*var, idx_reg);
                self.emit(
                    IrInst::Copy {
                        dst: idx_reg,
                        src: start_reg,
                    },
                    span.start,
                );
                let limit_reg = self.new_vreg(ValType::Int);
                self.emit(
                    IrInst::Copy {
                        dst: limit_reg,
                        src: end_reg,
                    },
                    span.start,
                );

                let head_bb = self.new_block();
                let body_bb = self.new_block();
                let step_bb = self.new_block();
                let exit_bb = self.new_block();

                self.terminate(Terminator::Jump(head_bb), span.start);

                self.cur_bb = head_bb;
                let cmp = self.new_vreg(ValType::Bool);
                self.emit(
                    IrInst::Binary {
                        dst: cmp,
                        op: BinaryOp::Lt,
                        lhs: idx_reg,
                        rhs: limit_reg,
                        operand_ty: ValType::Int,
                        result_ty: ValType::Bool,
                    },
                    span.start,
                );
                self.terminate(
                    Terminator::Branch {
                        cond: cmp,
                        then_bb: body_bb,
                        else_bb: exit_bb,
                    },
                    span.start,
                );

                // `continue` jumps to `step_bb` so the loop counter advances!
                self.loops.push(LoopTargets {
                    continue_bb: step_bb,
                    break_bb: exit_bb,
                });
                self.cur_bb = body_bb;
                self.lower_stmts(body);
                if !self.current_terminated() {
                    self.terminate(Terminator::Jump(step_bb), span.end);
                }
                self.loops.pop();

                self.cur_bb = step_bb;
                let one = self.new_vreg(ValType::Int);
                self.emit(
                    IrInst::Const {
                        dst: one,
                        val: 1,
                        ty: ValType::Int,
                    },
                    span.end,
                );
                let next_val = self.new_vreg(ValType::Int);
                self.emit(
                    IrInst::Binary {
                        dst: next_val,
                        op: BinaryOp::Add,
                        lhs: idx_reg,
                        rhs: one,
                        operand_ty: ValType::Int,
                        result_ty: ValType::Int,
                    },
                    span.end,
                );
                self.emit(
                    IrInst::Copy {
                        dst: idx_reg,
                        src: next_val,
                    },
                    span.end,
                );
                self.terminate(Terminator::Jump(head_bb), span.end);

                self.cur_bb = exit_bb;
            }
            TypedStmt::Break(span) => {
                if let Some(lp) = self.loops.last() {
                    let target = lp.break_bb;
                    self.terminate(Terminator::Jump(target), span.start);
                }
            }
            TypedStmt::Continue(span) => {
                if let Some(lp) = self.loops.last() {
                    let target = lp.continue_bb;
                    self.terminate(Terminator::Jump(target), span.start);
                }
            }
            TypedStmt::Return { value, span } => {
                let ret_reg = value.as_ref().and_then(|e| self.lower_expr(e));
                self.terminate(Terminator::Return(ret_reg), span.start);
            }
            TypedStmt::Expr { expr, .. } => {
                let _ = self.lower_expr(expr);
            }
        }
    }

    fn lower_expr(&mut self, expr: &TypedExpr) -> Option<VReg> {
        let sp = expr.span.start;
        match &expr.kind {
            TypedExprKind::Const(val, ty) => {
                let dst = self.new_vreg(*ty);
                self.emit(
                    IrInst::Const {
                        dst,
                        val: *val,
                        ty: *ty,
                    },
                    sp,
                );
                Some(dst)
            }
            TypedExprKind::Local(local) => Some(self.local_map[local]),
            TypedExprKind::StateScalar(slot) => {
                let ty = expr.ty.expect("state scalar typed");
                let dst = self.new_vreg(ty);
                self.emit(IrInst::LoadState { dst, slot: *slot }, sp);
                Some(dst)
            }
            TypedExprKind::StateArray { arr_id, index } => {
                let idx = self.lower_expr(index).expect("index has i32");
                let ty = expr.ty.expect("state array element typed");
                let dst = self.new_vreg(ty);
                self.emit(
                    IrInst::LoadStateArr {
                        dst,
                        arr_id: *arr_id,
                        idx,
                    },
                    sp,
                );
                Some(dst)
            }
            TypedExprKind::Unary { op, expr: inner } => {
                let src = self.lower_expr(inner).expect("unary operand typed");
                let ty = expr.ty.expect("unary typed");
                let dst = self.new_vreg(ty);
                self.emit(
                    IrInst::Unary {
                        dst,
                        op: *op,
                        src,
                        ty,
                    },
                    sp,
                );
                Some(dst)
            }
            TypedExprKind::Binary { op, lhs, rhs } => {
                if *op == BinaryOp::And || *op == BinaryOp::Or {
                    let lreg = self.lower_expr(lhs).expect("bool lhs");
                    let res = self.new_vreg(ValType::Bool);
                    self.emit(
                        IrInst::Copy {
                            dst: res,
                            src: lreg,
                        },
                        sp,
                    );
                    let rhs_bb = self.new_block();
                    let join_bb = self.new_block();
                    if *op == BinaryOp::And {
                        self.terminate(
                            Terminator::Branch {
                                cond: lreg,
                                then_bb: rhs_bb,
                                else_bb: join_bb,
                            },
                            sp,
                        );
                    } else {
                        self.terminate(
                            Terminator::Branch {
                                cond: lreg,
                                then_bb: join_bb,
                                else_bb: rhs_bb,
                            },
                            sp,
                        );
                    }
                    self.cur_bb = rhs_bb;
                    let rreg = self.lower_expr(rhs).expect("bool rhs");
                    self.emit(
                        IrInst::Copy {
                            dst: res,
                            src: rreg,
                        },
                        sp,
                    );
                    self.terminate(Terminator::Jump(join_bb), sp);
                    self.cur_bb = join_bb;
                    return Some(res);
                }

                let lreg = self.lower_expr(lhs).expect("lhs typed");
                let rreg = self.lower_expr(rhs).expect("rhs typed");
                let operand_ty = lhs.ty.expect("lhs has type");
                let result_ty = expr.ty.expect("binary result typed");
                let dst = self.new_vreg(result_ty);
                self.emit(
                    IrInst::Binary {
                        dst,
                        op: *op,
                        lhs: lreg,
                        rhs: rreg,
                        operand_ty,
                        result_ty,
                    },
                    sp,
                );
                Some(dst)
            }
            TypedExprKind::Call { target, args } => {
                let mut arg_regs = Vec::with_capacity(args.len());
                for a in args {
                    arg_regs.push(self.lower_expr(a).expect("call arg typed"));
                }
                match target {
                    CallTarget::ToFix => {
                        let dst = self.new_vreg(ValType::Fix);
                        self.emit(
                            IrInst::Cast {
                                dst,
                                src: arg_regs[0],
                                to: ValType::Fix,
                            },
                            sp,
                        );
                        Some(dst)
                    }
                    CallTarget::ToInt => {
                        let dst = self.new_vreg(ValType::Int);
                        self.emit(
                            IrInst::Cast {
                                dst,
                                src: arg_regs[0],
                                to: ValType::Int,
                            },
                            sp,
                        );
                        Some(dst)
                    }
                    CallTarget::UserFn(func_id) => {
                        let dst = expr.ty.map(|t| self.new_vreg(t));
                        self.emit(
                            IrInst::Call {
                                dst,
                                func_id: *func_id,
                                args: arg_regs,
                            },
                            sp,
                        );
                        dst
                    }
                    CallTarget::Syscall(id) => {
                        let dst = expr.ty.map(|t| self.new_vreg(t));
                        self.emit(
                            IrInst::Syscall {
                                dst,
                                id: *id,
                                args: arg_regs,
                            },
                            sp,
                        );
                        dst
                    }
                }
            }
        }
    }
}

fn assign_op_to_binary(op: AssignOp) -> BinaryOp {
    match op {
        AssignOp::Add => BinaryOp::Add,
        AssignOp::Sub => BinaryOp::Sub,
        AssignOp::Mul => BinaryOp::Mul,
        AssignOp::Div => BinaryOp::Div,
        AssignOp::Set => unreachable!("AssignOp::Set handled directly"),
    }
}

fn lower_fn(func: &TypedFn) -> IrFn {
    let mut lowerer = Lowerer {
        vreg_types: Vec::new(),
        local_map: HashMap::new(),
        blocks: Vec::new(),
        cur_bb: BlockId(0),
        loops: Vec::new(),
    };
    let entry = lowerer.new_block();
    lowerer.cur_bb = entry;

    let mut params = Vec::with_capacity(func.params.len());
    for &(local_id, ty) in &func.params {
        let v = lowerer.new_vreg(ty);
        lowerer.local_map.insert(local_id, v);
        params.push((v, ty));
    }

    lowerer.lower_stmts(&func.body);

    if !lowerer.current_terminated() {
        lowerer.terminate(Terminator::Return(None), 0);
    }

    // Any unreachable leftover join block gets a safe implicit Return/Unreachable cleanup;
    // opt pass will strip unreachable blocks, and codegen only emits reachable blocks.
    IrFn {
        name: func.name.clone(),
        params,
        ret: func.ret,
        vreg_types: lowerer.vreg_types,
        blocks: lowerer.blocks,
    }
}

/// Pretty-prints an [`IrProgram`] for inspection (`--dump-ir`).
#[must_use]
pub fn dump_ir(prog: &IrProgram) -> String {
    let mut out = String::new();
    for (idx, func) in prog.functions.iter().enumerate() {
        let params = func
            .params
            .iter()
            .map(|(v, ty)| format!("v{}: {ty}", v.0))
            .collect::<Vec<_>>()
            .join(", ");
        let ret = func
            .ret
            .map_or_else(|| "void".to_string(), |t| t.to_string());
        let entry = if usize::from(prog.entry_func) == idx {
            " [entry]"
        } else {
            ""
        };
        let _ = writeln!(out, "fn {}({params}) -> {ret}{entry} {{", func.name);
        for (bb_idx, bb) in func.blocks.iter().enumerate() {
            let _ = writeln!(out, "  bb{bb_idx}:");
            for inst in &bb.insts {
                let _ = writeln!(out, "    {}", fmt_inst(inst));
            }
            let _ = writeln!(out, "    {}", fmt_term(&bb.term));
        }
        let _ = writeln!(out, "}}\n");
    }
    out
}

fn fmt_inst(inst: &IrInst) -> String {
    match inst {
        IrInst::Const { dst, val, ty } => format!("v{} = const {val} ({ty})", dst.0),
        IrInst::Copy { dst, src } => format!("v{} = copy v{}", dst.0, src.0),
        IrInst::Unary { dst, op, src, .. } => format!("v{} = {op:?} v{}", dst.0, src.0),
        IrInst::Binary {
            dst, op, lhs, rhs, ..
        } => format!("v{} = {op:?} v{}, v{}", dst.0, lhs.0, rhs.0),
        IrInst::Cast { dst, src, to } => format!("v{} = cast<{to}> v{}", dst.0, src.0),
        IrInst::LoadState { dst, slot } => format!("v{} = load_state[{slot}]", dst.0),
        IrInst::StoreState { slot, src } => format!("store_state[{slot}] = v{}", src.0),
        IrInst::LoadStateArr { dst, arr_id, idx } => {
            format!("v{} = load_arr[{arr_id}][v{}]", dst.0, idx.0)
        }
        IrInst::StoreStateArr { arr_id, idx, src } => {
            format!("store_arr[{arr_id}][v{}] = v{}", idx.0, src.0)
        }
        IrInst::Call { dst, func_id, args } => {
            let arg_s = args
                .iter()
                .map(|a| format!("v{}", a.0))
                .collect::<Vec<_>>()
                .join(", ");
            match dst {
                Some(d) => format!("v{} = call fn#{func_id}({arg_s})", d.0),
                None => format!("call fn#{func_id}({arg_s})"),
            }
        }
        IrInst::Syscall { dst, id, args } => {
            let name = SyscallSpec::from_id(*id as u8)
                .map(|s| s.name)
                .unwrap_or("?");
            let arg_s = args
                .iter()
                .map(|a| format!("v{}", a.0))
                .collect::<Vec<_>>()
                .join(", ");
            match dst {
                Some(d) => format!("v{} = syscall {name}({arg_s})", d.0),
                None => format!("syscall {name}({arg_s})"),
            }
        }
    }
}

fn fmt_term(term: &Terminator) -> String {
    match term {
        Terminator::Jump(bb) => format!("jmp bb{}", bb.0),
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => format!("br v{}, bb{}, bb{}", cond.0, then_bb.0, else_bb.0),
        Terminator::Return(Some(v)) => format!("ret v{}", v.0),
        Terminator::Return(None) => "ret".to_string(),
        Terminator::Unreachable => "unreachable".to_string(),
    }
}
