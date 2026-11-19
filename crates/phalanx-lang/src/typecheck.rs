//! Static type checker, constant evaluator, and state/function resolver for `.phx`.

use std::collections::HashMap;

use phalanx_vm::{
    MAX_FUNCTIONS, MAX_STATE_ARRAYS, MAX_STATE_SLOTS, SyscallId, SyscallSpec, ValType,
};

use crate::ast::{
    AssignOp, AssignTarget, BinaryOp, Block, ElseBranch, Expr, ExprKind, Item, Module, Stmt,
    UnaryOp,
};
use crate::diagnostic::{Diagnostic, Span};

/// Unique local variable slot within a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalId(pub u32);

/// Type-checked program ready for IR lowering.
#[derive(Debug, Clone)]
pub struct TypedProgram {
    /// Persistent scalar state slot types.
    pub state_layout: Vec<ValType>,
    /// Persistent scalar state slot initial values.
    pub state_init: Vec<i32>,
    /// Human-readable names of state slots.
    pub state_names: Vec<String>,
    /// Persistent state array descriptors (`name, base, len, elem_ty`).
    pub state_arrays: Vec<(String, u16, u16, ValType)>,
    /// Entry function index (`tick`).
    pub entry_func: u16,
    /// Type-checked functions.
    pub functions: Vec<TypedFn>,
}

/// Type-checked function body and signature.
#[derive(Debug, Clone)]
pub struct TypedFn {
    /// Function identifier.
    pub name: String,
    /// Parameter bindings `(LocalId, ValType)`.
    pub params: Vec<(LocalId, ValType)>,
    /// Optional return type.
    pub ret: Option<ValType>,
    /// Total distinct `LocalId`s allocated in this function.
    pub num_locals: u32,
    /// Checked function body statements.
    pub body: Vec<TypedStmt>,
}

/// Checked assignment destination.
#[derive(Debug, Clone)]
pub enum TypedAssignTarget {
    /// Function-local variable.
    Local(LocalId),
    /// Scalar persistent state slot.
    StateScalar(u16),
    /// Indexed persistent state array.
    StateArray {
        /// Descriptor index in `state_arrays`.
        arr_id: u8,
        /// Checked integer index expression.
        index: TypedExpr,
    },
}

/// Checked statement node.
#[derive(Debug, Clone)]
pub enum TypedStmt {
    /// Local variable initialization.
    Let {
        /// Target local ID.
        local: LocalId,
        /// Value type.
        ty: ValType,
        /// Initializer expression.
        init: TypedExpr,
        /// Source span.
        span: Span,
    },
    /// Assignment or compound assignment.
    Assign {
        /// Destination lvalue.
        target: TypedAssignTarget,
        /// Assignment operator.
        op: AssignOp,
        /// Target element/variable type.
        ty: ValType,
        /// Assigned RHS expression.
        value: TypedExpr,
        /// Source span.
        span: Span,
    },
    /// Conditional branch.
    If {
        /// Boolean condition.
        cond: TypedExpr,
        /// Then statements.
        then_stmts: Vec<TypedStmt>,
        /// Optional else statements.
        else_stmts: Option<Vec<TypedStmt>>,
        /// Source span.
        span: Span,
    },
    /// While loop.
    While {
        /// Boolean loop condition.
        cond: TypedExpr,
        /// Loop body statements.
        body: Vec<TypedStmt>,
        /// Source span.
        span: Span,
    },
    /// Counted integer range loop `for var in start..end`.
    For {
        /// Loop variable local ID (`i32`).
        var: LocalId,
        /// Start expression (`i32`).
        start: TypedExpr,
        /// Exclusive end expression (`i32`).
        end: TypedExpr,
        /// Loop body statements.
        body: Vec<TypedStmt>,
        /// Source span.
        span: Span,
    },
    /// Loop break.
    Break(Span),
    /// Loop continue.
    Continue(Span),
    /// Function return.
    Return {
        /// Optional returned expression.
        value: Option<TypedExpr>,
        /// Source span.
        span: Span,
    },
    /// Expression evaluated for side effects.
    Expr {
        /// Inner expression.
        expr: TypedExpr,
        /// Source span.
        span: Span,
    },
}

/// Resolved callee target for function/syscall invocations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget {
    /// Builtin `to_fix(i32) -> fix`.
    ToFix,
    /// Builtin `to_int(fix) -> i32`.
    ToInt,
    /// User-defined `.phx` function.
    UserFn(u16),
    /// Host arena or math syscall.
    Syscall(SyscallId),
}

/// Checked expression with resolved [`ValType`] (or `None` for void expressions).
#[derive(Debug, Clone)]
pub struct TypedExpr {
    /// Typed expression kind.
    pub kind: TypedExprKind,
    /// Optional expression result type (`None` for void calls).
    pub ty: Option<ValType>,
    /// Source span.
    pub span: Span,
}

/// Variants of [`TypedExpr`].
#[derive(Debug, Clone)]
pub enum TypedExprKind {
    /// Immediate 32-bit constant (`i32`, `fix`, or `bool`).
    Const(i32, ValType),
    /// Local variable load.
    Local(LocalId),
    /// Scalar persistent state slot load.
    StateScalar(u16),
    /// Indexed persistent state array load.
    StateArray {
        /// State array index.
        arr_id: u8,
        /// Integer index expression.
        index: Box<TypedExpr>,
    },
    /// Unary prefix operation.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand expression.
        expr: Box<TypedExpr>,
    },
    /// Binary operation.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left-hand expression.
        lhs: Box<TypedExpr>,
        /// Right-hand expression.
        rhs: Box<TypedExpr>,
    },
    /// Call expression.
    Call {
        /// Resolved target.
        target: CallTarget,
        /// Argument expressions.
        args: Vec<TypedExpr>,
    },
}

#[derive(Debug, Clone)]
struct FnSig {
    id: u16,
    params: Vec<ValType>,
    ret: Option<ValType>,
}

#[derive(Debug, Clone, Copy)]
struct LocalBinding {
    id: LocalId,
    ty: ValType,
    mutable: bool,
}

#[derive(Debug, Clone)]
enum StateRef {
    Scalar { slot: u16, ty: ValType },
    Array { arr_id: u8, elem_ty: ValType },
}

/// Performs static type checking and name/constant resolution on a parsed [`Module`].
pub fn check(module: &Module) -> Result<TypedProgram, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut consts: HashMap<String, (i32, ValType)> = HashMap::new();
    // Built-in fixed-point math constants (Q16.16)
    consts.insert("PI".into(), (205_887, ValType::Fix));
    consts.insert("TAU".into(), (411_775, ValType::Fix));
    consts.insert("HALF_PI".into(), (102_944, ValType::Fix));

    let mut state_layout = Vec::new();
    let mut state_init = Vec::new();
    let mut state_names = Vec::new();
    let mut state_arrays = Vec::new();
    let mut state_map: HashMap<String, StateRef> = HashMap::new();
    let mut fn_sigs: HashMap<String, FnSig> = HashMap::new();
    let mut fn_decls = Vec::new();

    for item in &module.items {
        match item {
            Item::Const {
                name,
                ty,
                value,
                span,
            } => {
                if consts.contains_key(name) || state_map.contains_key(name) {
                    errors.push(Diagnostic::new(
                        format!("duplicate global constant `{name}`"),
                        *span,
                    ));
                    continue;
                }
                let expected = ValType::from(*ty);
                match eval_const(value, &consts) {
                    Ok((val, got)) if got == expected => {
                        consts.insert(name.clone(), (val, got));
                    }
                    Ok((_, got)) => {
                        errors.push(Diagnostic::new(
                            format!("constant `{name}` declared as `{expected}`, but initialized with `{got}`"),
                            value.span,
                        ));
                    }
                    Err(diag) => errors.push(diag),
                }
            }
            Item::StateScalar {
                name,
                ty,
                init,
                span,
            } => {
                if consts.contains_key(name) || state_map.contains_key(name) {
                    errors.push(Diagnostic::new(
                        format!("duplicate state declaration `{name}`"),
                        *span,
                    ));
                    continue;
                }
                if state_layout.len() >= MAX_STATE_SLOTS {
                    errors.push(Diagnostic::new(
                        format!("state slots exceed VM maximum ({MAX_STATE_SLOTS})"),
                        *span,
                    ));
                    continue;
                }
                let expected = ValType::from(*ty);
                match eval_const(init, &consts) {
                    Ok((val, got)) if got == expected => {
                        let slot = state_layout.len() as u16;
                        state_layout.push(expected);
                        state_init.push(val);
                        state_names.push(name.clone());
                        state_map.insert(name.clone(), StateRef::Scalar { slot, ty: expected });
                    }
                    Ok((_, got)) => {
                        errors.push(Diagnostic::new(
                            format!("state `{name}` declared as `{expected}`, but initialized with `{got}`"),
                            init.span,
                        ));
                    }
                    Err(diag) => errors.push(diag),
                }
            }
            Item::StateArray {
                name,
                elem_ty,
                len,
                span,
            } => {
                if consts.contains_key(name) || state_map.contains_key(name) {
                    errors.push(Diagnostic::new(
                        format!("duplicate state declaration `{name}`"),
                        *span,
                    ));
                    continue;
                }
                if state_arrays.len() >= MAX_STATE_ARRAYS {
                    errors.push(Diagnostic::new(
                        format!("state arrays exceed VM maximum ({MAX_STATE_ARRAYS})"),
                        *span,
                    ));
                    continue;
                }
                if state_layout.len() + *len > MAX_STATE_SLOTS {
                    errors.push(Diagnostic::new(
                        format!("state slots exceed VM maximum ({MAX_STATE_SLOTS})"),
                        *span,
                    ));
                    continue;
                }
                let base = state_layout.len() as u16;
                let vt = ValType::from(*elem_ty);
                for idx in 0..*len {
                    state_layout.push(vt);
                    state_init.push(0);
                    state_names.push(format!("{name}[{idx}]"));
                }
                let arr_id = state_arrays.len() as u8;
                state_arrays.push((name.clone(), base, *len as u16, vt));
                state_map.insert(
                    name.clone(),
                    StateRef::Array {
                        arr_id,
                        elem_ty: vt,
                    },
                );
            }
            Item::Fn(decl) => {
                if is_reserved_fn(&decl.name) {
                    errors.push(Diagnostic::new(
                        format!(
                            "function name `{}` collides with a built-in syscall",
                            decl.name
                        ),
                        decl.span,
                    ));
                    continue;
                }
                if fn_sigs.contains_key(&decl.name) {
                    errors.push(Diagnostic::new(
                        format!("duplicate function definition `{}`", decl.name),
                        decl.span,
                    ));
                    continue;
                }
                if fn_decls.len() >= MAX_FUNCTIONS {
                    errors.push(Diagnostic::new(
                        format!("function count exceeds VM maximum ({MAX_FUNCTIONS})"),
                        decl.span,
                    ));
                    continue;
                }
                let id = fn_decls.len() as u16;
                let params = decl.params.iter().map(|p| ValType::from(p.ty)).collect();
                let ret = decl.ret.map(ValType::from);
                fn_sigs.insert(decl.name.clone(), FnSig { id, params, ret });
                fn_decls.push(decl);
            }
        }
    }

    let entry_func = match fn_sigs.get("tick") {
        Some(sig) if sig.params.is_empty() && sig.ret.is_none() => sig.id,
        Some(_) => {
            errors.push(Diagnostic::new(
                "entrypoint `fn tick()` must take no parameters and return no value",
                Span::new(0, 0),
            ));
            0
        }
        None => {
            errors.push(
                Diagnostic::new(
                    "missing required entrypoint `fn tick() { ... }`",
                    Span::new(0, 0),
                )
                .with_help("define `fn tick()` as the per-tick bot controller"),
            );
            0
        }
    };

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut typed_fns = Vec::with_capacity(fn_decls.len());
    for decl in fn_decls {
        let mut ctx = FnChecker {
            consts: &consts,
            state_map: &state_map,
            fn_sigs: &fn_sigs,
            scopes: vec![HashMap::new()],
            next_local: 0,
            expected_ret: decl.ret.map(ValType::from),
            loop_depth: 0,
            errors: &mut errors,
        };
        let mut typed_params = Vec::with_capacity(decl.params.len());
        for p in &decl.params {
            let ty = ValType::from(p.ty);
            if ctx.scopes[0].contains_key(&p.name) {
                ctx.errors.push(Diagnostic::new(
                    format!("duplicate parameter `{}`", p.name),
                    p.span,
                ));
                continue;
            }
            let id = ctx.alloc_local();
            ctx.scopes[0].insert(
                p.name.clone(),
                LocalBinding {
                    id,
                    ty,
                    mutable: false,
                },
            );
            typed_params.push((id, ty));
        }

        let body = ctx.check_block(&decl.body);
        if let Some(ret_ty) = decl.ret
            && !block_always_returns(&body)
        {
            ctx.errors.push(Diagnostic::new(
                format!(
                    "function `{}` returns `{}` but not all control-flow paths return a value",
                    decl.name,
                    ValType::from(ret_ty)
                ),
                decl.span,
            ));
        }

        typed_fns.push(TypedFn {
            name: decl.name.clone(),
            params: typed_params,
            ret: decl.ret.map(ValType::from),
            num_locals: ctx.next_local,
            body,
        });
    }

    if errors.is_empty() {
        Ok(TypedProgram {
            state_layout,
            state_init,
            state_names,
            state_arrays,
            entry_func,
            functions: typed_fns,
        })
    } else {
        Err(errors)
    }
}

fn is_reserved_fn(name: &str) -> bool {
    name == "to_fix" || name == "to_int" || SyscallSpec::from_name(name).is_some()
}

fn eval_const(
    expr: &Expr,
    consts: &HashMap<String, (i32, ValType)>,
) -> Result<(i32, ValType), Diagnostic> {
    match &expr.kind {
        ExprKind::IntLit(v) => i32::try_from(*v)
            .map(|x| (x, ValType::Int))
            .map_err(|_| Diagnostic::new("integer literal overflows `i32`", expr.span)),
        ExprKind::FixLit(v) => i32::try_from(*v)
            .map(|x| (x, ValType::Fix))
            .map_err(|_| Diagnostic::new("fixed-point literal overflows Q16.16 `fix`", expr.span)),
        ExprKind::BoolLit(b) => Ok((i32::from(*b), ValType::Bool)),
        ExprKind::Ident(name) => consts.get(name).copied().ok_or_else(|| {
            Diagnostic::new(
                format!("unknown constant `{name}` in constant expression"),
                expr.span,
            )
        }),
        ExprKind::Unary { op, expr: inner } => {
            if *op == UnaryOp::Neg
                && let ExprKind::IntLit(v) = inner.kind
                && let Ok(neg) = i32::try_from(-v)
            {
                return Ok((neg, ValType::Int));
            }
            if *op == UnaryOp::Neg
                && let ExprKind::FixLit(v) = inner.kind
                && let Ok(neg) = i32::try_from(-v)
            {
                return Ok((neg, ValType::Fix));
            }
            let (val, ty) = eval_const(inner, consts)?;
            match (op, ty) {
                (UnaryOp::Neg, ValType::Int) => Ok((val.wrapping_neg(), ValType::Int)),
                (UnaryOp::Neg, ValType::Fix) => Ok((val.saturating_neg(), ValType::Fix)),
                (UnaryOp::Not, ValType::Bool) => Ok((i32::from(val == 0), ValType::Bool)),
                (UnaryOp::BitNot, ValType::Int) => Ok((!val, ValType::Int)),
                _ => Err(Diagnostic::new(
                    format!("unary operator not supported on constant `{ty}`"),
                    expr.span,
                )),
            }
        }
        ExprKind::Binary { op, lhs, rhs } => {
            let (lv, lt) = eval_const(lhs, consts)?;
            let (rv, rt) = eval_const(rhs, consts)?;
            if lt != rt {
                return Err(Diagnostic::new(
                    format!("constant binary op type mismatch: `{lt}` vs `{rt}`"),
                    expr.span,
                ));
            }
            match (op, lt) {
                (BinaryOp::Add, ValType::Int) => Ok((lv.wrapping_add(rv), ValType::Int)),
                (BinaryOp::Sub, ValType::Int) => Ok((lv.wrapping_sub(rv), ValType::Int)),
                (BinaryOp::Mul, ValType::Int) => Ok((lv.wrapping_mul(rv), ValType::Int)),
                (BinaryOp::Div, ValType::Int) => {
                    if rv == 0 {
                        Err(Diagnostic::new(
                            "division by zero in constant expression",
                            expr.span,
                        ))
                    } else {
                        Ok((lv.wrapping_div(rv), ValType::Int))
                    }
                }
                (BinaryOp::Mod, ValType::Int) => {
                    if rv == 0 {
                        Err(Diagnostic::new(
                            "modulo by zero in constant expression",
                            expr.span,
                        ))
                    } else {
                        Ok((lv.wrapping_rem(rv), ValType::Int))
                    }
                }
                (BinaryOp::Add, ValType::Fix) => Ok((lv.saturating_add(rv), ValType::Fix)),
                (BinaryOp::Sub, ValType::Fix) => Ok((lv.saturating_sub(rv), ValType::Fix)),
                (BinaryOp::Mul, ValType::Fix) => {
                    let prod = (i64::from(lv) * i64::from(rv)) >> 16;
                    Ok((
                        prod.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                        ValType::Fix,
                    ))
                }
                (BinaryOp::Div, ValType::Fix) => {
                    if rv == 0 {
                        Err(Diagnostic::new(
                            "division by zero in constant expression",
                            expr.span,
                        ))
                    } else {
                        let quot = (i64::from(lv) << 16) / i64::from(rv);
                        Ok((
                            quot.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                            ValType::Fix,
                        ))
                    }
                }
                (BinaryOp::BitAnd, ValType::Int) => Ok((lv & rv, ValType::Int)),
                (BinaryOp::BitOr, ValType::Int) => Ok((lv | rv, ValType::Int)),
                (BinaryOp::BitXor, ValType::Int) => Ok((lv ^ rv, ValType::Int)),
                (BinaryOp::Shl, ValType::Int) => {
                    Ok((lv.wrapping_shl((rv as u32) & 31), ValType::Int))
                }
                (BinaryOp::Shr, ValType::Int) => {
                    Ok((lv.wrapping_shr((rv as u32) & 31), ValType::Int))
                }
                (BinaryOp::Eq, _) => Ok((i32::from(lv == rv), ValType::Bool)),
                (BinaryOp::Ne, _) => Ok((i32::from(lv != rv), ValType::Bool)),
                (BinaryOp::Lt, ValType::Int | ValType::Fix) => {
                    Ok((i32::from(lv < rv), ValType::Bool))
                }
                (BinaryOp::Le, ValType::Int | ValType::Fix) => {
                    Ok((i32::from(lv <= rv), ValType::Bool))
                }
                (BinaryOp::Gt, ValType::Int | ValType::Fix) => {
                    Ok((i32::from(lv > rv), ValType::Bool))
                }
                (BinaryOp::Ge, ValType::Int | ValType::Fix) => {
                    Ok((i32::from(lv >= rv), ValType::Bool))
                }
                (BinaryOp::And, ValType::Bool) => {
                    Ok((i32::from(lv != 0 && rv != 0), ValType::Bool))
                }
                (BinaryOp::Or, ValType::Bool) => Ok((i32::from(lv != 0 || rv != 0), ValType::Bool)),
                _ => Err(Diagnostic::new(
                    format!("operator unsupported on constant type `{lt}`"),
                    expr.span,
                )),
            }
        }
        ExprKind::Call { name, args } if name == "to_fix" && args.len() == 1 => {
            let (v, ty) = eval_const(&args[0], consts)?;
            if ty != ValType::Int {
                return Err(Diagnostic::new("`to_fix` requires `i32`", args[0].span));
            }
            let shifted =
                (i64::from(v) << 16).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            Ok((shifted, ValType::Fix))
        }
        ExprKind::Call { name, args } if name == "to_int" && args.len() == 1 => {
            let (v, ty) = eval_const(&args[0], consts)?;
            if ty != ValType::Fix {
                return Err(Diagnostic::new("`to_int` requires `fix`", args[0].span));
            }
            Ok((v >> 16, ValType::Int))
        }
        _ => Err(Diagnostic::new(
            "expression cannot be evaluated at compile time",
            expr.span,
        )),
    }
}

struct FnChecker<'a> {
    consts: &'a HashMap<String, (i32, ValType)>,
    state_map: &'a HashMap<String, StateRef>,
    fn_sigs: &'a HashMap<String, FnSig>,
    scopes: Vec<HashMap<String, LocalBinding>>,
    next_local: u32,
    expected_ret: Option<ValType>,
    loop_depth: usize,
    errors: &'a mut Vec<Diagnostic>,
}

impl<'a> FnChecker<'a> {
    fn alloc_local(&mut self) -> LocalId {
        let id = LocalId(self.next_local);
        self.next_local += 1;
        id
    }

    fn lookup_local(&self, name: &str) -> Option<LocalBinding> {
        for scope in self.scopes.iter().rev() {
            if let Some(&b) = scope.get(name) {
                return Some(b);
            }
        }
        None
    }

    fn check_block(&mut self, block: &Block) -> Vec<TypedStmt> {
        self.scopes.push(HashMap::new());
        let mut out = Vec::with_capacity(block.stmts.len());
        for stmt in &block.stmts {
            if let Some(ts) = self.check_stmt(stmt) {
                out.push(ts);
            }
        }
        self.scopes.pop();
        out
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> Option<TypedStmt> {
        match stmt {
            Stmt::Let {
                name,
                mutable,
                ty,
                init,
                span,
            } => {
                let checked_init = self.check_expr(init);
                let Some(init_ty) = checked_init.ty else {
                    self.errors.push(Diagnostic::new(
                        "cannot bind `void` expression to a local variable",
                        init.span,
                    ));
                    return None;
                };
                if let Some(ann) = ty {
                    let expected = ValType::from(*ann);
                    if expected != init_ty {
                        self.errors.push(
                            Diagnostic::new(
                                format!(
                                    "mismatched types in `let {name}`: expected `{expected}`, found `{init_ty}`"
                                ),
                                init.span,
                            )
                            .with_help(type_conversion_hint(expected, init_ty)),
                        );
                        return None;
                    }
                }
                let local = self.alloc_local();
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(
                        name.clone(),
                        LocalBinding {
                            id: local,
                            ty: init_ty,
                            mutable: *mutable,
                        },
                    );
                }
                Some(TypedStmt::Let {
                    local,
                    ty: init_ty,
                    init: checked_init,
                    span: *span,
                })
            }
            Stmt::Assign {
                target,
                op,
                value,
                span,
            } => {
                let checked_val = self.check_expr(value);
                let Some(val_ty) = checked_val.ty else {
                    self.errors.push(Diagnostic::new(
                        "cannot assign `void` expression",
                        value.span,
                    ));
                    return None;
                };

                let (typed_target, target_ty) = match target {
                    AssignTarget::Var(name) => {
                        if let Some(local) = self.lookup_local(name) {
                            if !local.mutable {
                                self.errors.push(
                                    Diagnostic::new(
                                        format!(
                                            "cannot assign twice to immutable variable `{name}`"
                                        ),
                                        *span,
                                    )
                                    .with_help(format!("consider declaring `let mut {name}`")),
                                );
                                return None;
                            }
                            (TypedAssignTarget::Local(local.id), local.ty)
                        } else if let Some(StateRef::Scalar { slot, ty }) = self.state_map.get(name)
                        {
                            (TypedAssignTarget::StateScalar(*slot), *ty)
                        } else if self.consts.contains_key(name) {
                            self.errors.push(Diagnostic::new(
                                format!("cannot assign to constant `{name}`"),
                                *span,
                            ));
                            return None;
                        } else {
                            self.errors.push(Diagnostic::new(
                                format!("cannot find variable `{name}` in this scope"),
                                *span,
                            ));
                            return None;
                        }
                    }
                    AssignTarget::Index { name, index } => {
                        let checked_idx = self.check_expr(index);
                        if checked_idx.ty != Some(ValType::Int) {
                            self.errors.push(Diagnostic::new(
                                "array index must have type `i32`",
                                index.span,
                            ));
                            return None;
                        }
                        match self.state_map.get(name) {
                            Some(StateRef::Array { arr_id, elem_ty }) => (
                                TypedAssignTarget::StateArray {
                                    arr_id: *arr_id,
                                    index: checked_idx,
                                },
                                *elem_ty,
                            ),
                            _ => {
                                self.errors.push(Diagnostic::new(
                                    format!("unknown state array `{name}`"),
                                    *span,
                                ));
                                return None;
                            }
                        }
                    }
                };

                if target_ty != val_ty {
                    self.errors.push(
                        Diagnostic::new(
                            format!(
                                "assignment type mismatch: expected `{target_ty}`, found `{val_ty}`"
                            ),
                            value.span,
                        )
                        .with_help(type_conversion_hint(target_ty, val_ty)),
                    );
                    return None;
                }
                if *op != AssignOp::Set && !matches!(target_ty, ValType::Int | ValType::Fix) {
                    self.errors.push(Diagnostic::new(
                        format!("compound assignment cannot be applied to `{target_ty}`"),
                        *span,
                    ));
                    return None;
                }

                Some(TypedStmt::Assign {
                    target: typed_target,
                    op: *op,
                    ty: target_ty,
                    value: checked_val,
                    span: *span,
                })
            }
            Stmt::If {
                cond,
                then_block,
                else_branch,
                span,
            } => {
                let checked_cond = self.check_expr(cond);
                if checked_cond.ty != Some(ValType::Bool) {
                    self.errors.push(Diagnostic::new(
                        "`if` condition must have type `bool`",
                        cond.span,
                    ));
                }
                let then_stmts = self.check_block(then_block);
                let else_stmts = else_branch.as_ref().map(|eb| match eb {
                    ElseBranch::Block(blk) => self.check_block(blk),
                    ElseBranch::If(nested) => self.check_stmt(nested).into_iter().collect(),
                });
                Some(TypedStmt::If {
                    cond: checked_cond,
                    then_stmts,
                    else_stmts,
                    span: *span,
                })
            }
            Stmt::While { cond, body, span } => {
                let checked_cond = self.check_expr(cond);
                if checked_cond.ty != Some(ValType::Bool) {
                    self.errors.push(Diagnostic::new(
                        "`while` condition must have type `bool`",
                        cond.span,
                    ));
                }
                self.loop_depth += 1;
                let checked_body = self.check_block(body);
                self.loop_depth -= 1;
                Some(TypedStmt::While {
                    cond: checked_cond,
                    body: checked_body,
                    span: *span,
                })
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                span,
            } => {
                let checked_start = self.check_expr(start);
                if checked_start.ty != Some(ValType::Int) {
                    self.errors.push(Diagnostic::new(
                        "`for` range start must have type `i32`",
                        start.span,
                    ));
                }
                let checked_end = self.check_expr(end);
                if checked_end.ty != Some(ValType::Int) {
                    self.errors.push(Diagnostic::new(
                        "`for` range end must have type `i32`",
                        end.span,
                    ));
                }
                let loop_var = self.alloc_local();
                self.scopes.push(HashMap::new());
                if let Some(scope) = self.scopes.last_mut() {
                    scope.insert(
                        var.clone(),
                        LocalBinding {
                            id: loop_var,
                            ty: ValType::Int,
                            mutable: false,
                        },
                    );
                }
                self.loop_depth += 1;
                let checked_body = self.check_block(body);
                self.loop_depth -= 1;
                self.scopes.pop();
                Some(TypedStmt::For {
                    var: loop_var,
                    start: checked_start,
                    end: checked_end,
                    body: checked_body,
                    span: *span,
                })
            }
            Stmt::Break(span) => {
                if self.loop_depth == 0 {
                    self.errors.push(Diagnostic::new(
                        "`break` cannot be used outside of a loop",
                        *span,
                    ));
                    return None;
                }
                Some(TypedStmt::Break(*span))
            }
            Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    self.errors.push(Diagnostic::new(
                        "`continue` cannot be used outside of a loop",
                        *span,
                    ));
                    return None;
                }
                Some(TypedStmt::Continue(*span))
            }
            Stmt::Return { value, span } => {
                let checked = value.as_ref().map(|e| self.check_expr(e));
                match (self.expected_ret, checked.as_ref().and_then(|e| e.ty)) {
                    (None, None) if value.is_none() => {}
                    (Some(exp), Some(got)) if exp == got => {}
                    (None, _) => {
                        self.errors.push(Diagnostic::new(
                            "void function cannot return a value",
                            *span,
                        ));
                    }
                    (Some(exp), got) => {
                        let got_desc = got.map_or_else(|| "void".to_string(), |t| t.to_string());
                        self.errors.push(Diagnostic::new(
                            format!("return type mismatch: expected `{exp}`, found `{got_desc}`"),
                            *span,
                        ));
                    }
                }
                Some(TypedStmt::Return {
                    value: checked,
                    span: *span,
                })
            }
            Stmt::Expr { expr, span } => {
                let checked = self.check_expr(expr);
                Some(TypedStmt::Expr {
                    expr: checked,
                    span: *span,
                })
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr) -> TypedExpr {
        let dummy = || TypedExpr {
            kind: TypedExprKind::Const(0, ValType::Int),
            ty: None,
            span: expr.span,
        };

        match &expr.kind {
            ExprKind::IntLit(v) => match i32::try_from(*v) {
                Ok(n) => TypedExpr {
                    kind: TypedExprKind::Const(n, ValType::Int),
                    ty: Some(ValType::Int),
                    span: expr.span,
                },
                Err(_) => {
                    self.errors.push(Diagnostic::new(
                        "integer literal overflows `i32`",
                        expr.span,
                    ));
                    dummy()
                }
            },
            ExprKind::FixLit(v) => match i32::try_from(*v) {
                Ok(n) => TypedExpr {
                    kind: TypedExprKind::Const(n, ValType::Fix),
                    ty: Some(ValType::Fix),
                    span: expr.span,
                },
                Err(_) => {
                    self.errors.push(Diagnostic::new(
                        "fixed-point literal overflows Q16.16 `fix`",
                        expr.span,
                    ));
                    dummy()
                }
            },
            ExprKind::BoolLit(b) => TypedExpr {
                kind: TypedExprKind::Const(i32::from(*b), ValType::Bool),
                ty: Some(ValType::Bool),
                span: expr.span,
            },
            ExprKind::Ident(name) => {
                if let Some(local) = self.lookup_local(name) {
                    TypedExpr {
                        kind: TypedExprKind::Local(local.id),
                        ty: Some(local.ty),
                        span: expr.span,
                    }
                } else if let Some(StateRef::Scalar { slot, ty }) = self.state_map.get(name) {
                    TypedExpr {
                        kind: TypedExprKind::StateScalar(*slot),
                        ty: Some(*ty),
                        span: expr.span,
                    }
                } else if let Some(&(val, ty)) = self.consts.get(name) {
                    TypedExpr {
                        kind: TypedExprKind::Const(val, ty),
                        ty: Some(ty),
                        span: expr.span,
                    }
                } else {
                    self.errors.push(Diagnostic::new(
                        format!("cannot find value `{name}` in this scope"),
                        expr.span,
                    ));
                    dummy()
                }
            }
            ExprKind::Index { name, index } => {
                let checked_idx = self.check_expr(index);
                if checked_idx.ty != Some(ValType::Int) {
                    self.errors.push(Diagnostic::new(
                        "array index must have type `i32`",
                        index.span,
                    ));
                }
                match self.state_map.get(name) {
                    Some(StateRef::Array { arr_id, elem_ty }) => TypedExpr {
                        kind: TypedExprKind::StateArray {
                            arr_id: *arr_id,
                            index: Box::new(checked_idx),
                        },
                        ty: Some(*elem_ty),
                        span: expr.span,
                    },
                    _ => {
                        self.errors.push(Diagnostic::new(
                            format!("unknown state array `{name}`"),
                            expr.span,
                        ));
                        dummy()
                    }
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                if *op == UnaryOp::Neg
                    && let ExprKind::IntLit(v) = inner.kind
                    && let Ok(neg) = i32::try_from(-v)
                {
                    return TypedExpr {
                        kind: TypedExprKind::Const(neg, ValType::Int),
                        ty: Some(ValType::Int),
                        span: expr.span,
                    };
                }
                if *op == UnaryOp::Neg
                    && let ExprKind::FixLit(v) = inner.kind
                    && let Ok(neg) = i32::try_from(-v)
                {
                    return TypedExpr {
                        kind: TypedExprKind::Const(neg, ValType::Fix),
                        ty: Some(ValType::Fix),
                        span: expr.span,
                    };
                }
                let checked = self.check_expr(inner);
                let Some(inner_ty) = checked.ty else {
                    self.errors.push(Diagnostic::new(
                        "cannot apply unary operator to `void`",
                        expr.span,
                    ));
                    return dummy();
                };
                let ok = match (op, inner_ty) {
                    (UnaryOp::Neg, ValType::Int | ValType::Fix) => true,
                    (UnaryOp::Not, ValType::Bool) => true,
                    (UnaryOp::BitNot, ValType::Int) => true,
                    _ => {
                        self.errors.push(Diagnostic::new(
                            format!("unary operator cannot be applied to `{inner_ty}`"),
                            expr.span,
                        ));
                        false
                    }
                };
                TypedExpr {
                    kind: TypedExprKind::Unary {
                        op: *op,
                        expr: Box::new(checked),
                    },
                    ty: if ok { Some(inner_ty) } else { None },
                    span: expr.span,
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let checked_lhs = self.check_expr(lhs);
                let checked_rhs = self.check_expr(rhs);
                let (Some(lt), Some(rt)) = (checked_lhs.ty, checked_rhs.ty) else {
                    self.errors.push(Diagnostic::new(
                        "cannot use `void` expression as binary operand",
                        expr.span,
                    ));
                    return dummy();
                };
                if lt != rt {
                    self.errors.push(
                        Diagnostic::new(
                            format!("binary operand type mismatch: `{lt}` vs `{rt}`"),
                            expr.span,
                        )
                        .with_help(type_conversion_hint(lt, rt)),
                    );
                    return dummy();
                }
                let out_ty = match (op, lt) {
                    (
                        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div,
                        ValType::Int | ValType::Fix,
                    ) => Some(lt),
                    (
                        BinaryOp::Mod
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                        | BinaryOp::Shl
                        | BinaryOp::Shr,
                        ValType::Int,
                    ) => Some(ValType::Int),
                    (BinaryOp::Eq | BinaryOp::Ne, _) => Some(ValType::Bool),
                    (
                        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge,
                        ValType::Int | ValType::Fix,
                    ) => Some(ValType::Bool),
                    (BinaryOp::And | BinaryOp::Or, ValType::Bool) => Some(ValType::Bool),
                    _ => {
                        self.errors.push(Diagnostic::new(
                            format!("operator cannot be applied to type `{lt}`"),
                            expr.span,
                        ));
                        None
                    }
                };
                TypedExpr {
                    kind: TypedExprKind::Binary {
                        op: *op,
                        lhs: Box::new(checked_lhs),
                        rhs: Box::new(checked_rhs),
                    },
                    ty: out_ty,
                    span: expr.span,
                }
            }
            ExprKind::Call { name, args } => {
                let checked_args: Vec<TypedExpr> =
                    args.iter().map(|a| self.check_expr(a)).collect();
                if name == "to_fix" {
                    return self.check_conversion(
                        CallTarget::ToFix,
                        "to_fix",
                        ValType::Int,
                        ValType::Fix,
                        checked_args,
                        expr.span,
                    );
                }
                if name == "to_int" {
                    return self.check_conversion(
                        CallTarget::ToInt,
                        "to_int",
                        ValType::Fix,
                        ValType::Int,
                        checked_args,
                        expr.span,
                    );
                }
                if let Some(sig) = self.fn_sigs.get(name) {
                    self.check_call_args(name, &sig.params, &checked_args, expr.span);
                    return TypedExpr {
                        kind: TypedExprKind::Call {
                            target: CallTarget::UserFn(sig.id),
                            args: checked_args,
                        },
                        ty: sig.ret,
                        span: expr.span,
                    };
                }
                if let Some(spec) = SyscallSpec::from_name(name) {
                    self.check_call_args(name, spec.params, &checked_args, expr.span);
                    return TypedExpr {
                        kind: TypedExprKind::Call {
                            target: CallTarget::Syscall(spec.id),
                            args: checked_args,
                        },
                        ty: spec.ret,
                        span: expr.span,
                    };
                }
                self.errors.push(Diagnostic::new(
                    format!("cannot find function `{name}` in this scope"),
                    expr.span,
                ));
                dummy()
            }
        }
    }

    fn check_conversion(
        &mut self,
        target: CallTarget,
        name: &str,
        expected_in: ValType,
        out_ty: ValType,
        args: Vec<TypedExpr>,
        span: Span,
    ) -> TypedExpr {
        if args.len() != 1 {
            self.errors.push(Diagnostic::new(
                format!("`{name}` expects 1 argument, found {}", args.len()),
                span,
            ));
            return TypedExpr {
                kind: TypedExprKind::Const(0, out_ty),
                ty: Some(out_ty),
                span,
            };
        }
        if args[0].ty != Some(expected_in) {
            self.errors.push(Diagnostic::new(
                format!("`{name}` expects argument of type `{expected_in}`"),
                args[0].span,
            ));
        }
        TypedExpr {
            kind: TypedExprKind::Call { target, args },
            ty: Some(out_ty),
            span,
        }
    }

    fn check_call_args(
        &mut self,
        name: &str,
        expected: &[ValType],
        args: &[TypedExpr],
        span: Span,
    ) {
        if expected.len() != args.len() {
            self.errors.push(Diagnostic::new(
                format!(
                    "`{name}` expects {} argument(s), found {}",
                    expected.len(),
                    args.len()
                ),
                span,
            ));
            return;
        }
        for (i, (&exp, arg)) in expected.iter().zip(args.iter()).enumerate() {
            if arg.ty != Some(exp) {
                let found = arg.ty.map_or_else(|| "void".to_string(), |t| t.to_string());
                self.errors.push(
                    Diagnostic::new(
                        format!(
                            "argument #{} to `{name}` has wrong type: expected `{exp}`, found `{found}`",
                            i + 1
                        ),
                        arg.span,
                    )
                    .with_help(
                        arg.ty
                            .map_or_else(String::new, |g| type_conversion_hint(exp, g)),
                    ),
                );
            }
        }
    }
}

fn type_conversion_hint(expected: ValType, found: ValType) -> String {
    match (expected, found) {
        (ValType::Fix, ValType::Int) => {
            "convert explicit integer to fixed-point with `to_fix(...)`".into()
        }
        (ValType::Int, ValType::Fix) => {
            "convert explicit fixed-point to integer with `to_int(...)`".into()
        }
        _ => format!("expected `{expected}`, found `{found}`"),
    }
}

fn block_always_returns(stmts: &[TypedStmt]) -> bool {
    for stmt in stmts {
        match stmt {
            TypedStmt::Return { .. } => return true,
            TypedStmt::If {
                then_stmts,
                else_stmts: Some(else_stmts),
                ..
            } if block_always_returns(then_stmts) && block_always_returns(else_stmts) => {
                return true;
            }
            _ => {}
        }
    }
    false
}
