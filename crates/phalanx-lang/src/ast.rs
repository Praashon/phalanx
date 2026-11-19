//! Abstract Syntax Tree definitions for `.phx` programs.

use phalanx_vm::ValType;

use crate::diagnostic::Span;

/// Top-level parsed `.phx` module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// Ordered list of module declarations.
    pub items: Vec<Item>,
}

/// Source-level type annotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeAnn {
    /// `i32`
    Int,
    /// `fix`
    Fix,
    /// `bool`
    Bool,
}

impl From<TypeAnn> for ValType {
    fn from(value: TypeAnn) -> Self {
        match value {
            TypeAnn::Int => Self::Int,
            TypeAnn::Fix => Self::Fix,
            TypeAnn::Bool => Self::Bool,
        }
    }
}

/// Top-level module declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Compile-time constant declaration (`const NAME: ty = expr;`).
    Const {
        /// Constant name.
        name: String,
        /// Declared type.
        ty: TypeAnn,
        /// Constant initializer expression.
        value: Expr,
        /// Declaration span.
        span: Span,
    },
    /// Persistent scalar state slot (`state name: ty = expr;`).
    StateScalar {
        /// State slot name.
        name: String,
        /// Slot type.
        ty: TypeAnn,
        /// Initial value expression (const-evaluated).
        init: Expr,
        /// Declaration span.
        span: Span,
    },
    /// Persistent fixed-size state array (`state arr: [ty; N];`).
    StateArray {
        /// Array name.
        name: String,
        /// Element type.
        elem_ty: TypeAnn,
        /// Fixed length.
        len: usize,
        /// Declaration span.
        span: Span,
    },
    /// Function definition (`fn name(...) [-> ty] { ... }`).
    Fn(FnDecl),
}

/// Function declaration AST node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnDecl {
    /// Function identifier.
    pub name: String,
    /// Parameter declarations.
    pub params: Vec<Param>,
    /// Optional return type (`None` for void).
    pub ret: Option<TypeAnn>,
    /// Function body block.
    pub body: Block,
    /// Span covering the signature and body.
    pub span: Span,
}

/// Function parameter `(name: ty)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// Parameter name.
    pub name: String,
    /// Parameter type.
    pub ty: TypeAnn,
    /// Source span.
    pub span: Span,
}

/// Sequence of statements inside braces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Contained statements.
    pub stmts: Vec<Stmt>,
    /// Covering span.
    pub span: Span,
}

/// Target of an assignment statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignTarget {
    /// Local variable or scalar `state` variable.
    Var(String),
    /// Persistent `state` array element `name[index]`.
    Index {
        /// Array name.
        name: String,
        /// Element index expression.
        index: Expr,
    },
}

/// Assignment operator kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Set,
    /// `+=`
    Add,
    /// `-=`
    Sub,
    /// `*=`
    Mul,
    /// `/=`
    Div,
}

/// `else` branch attached to an `if` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElseBranch {
    /// `else { ... }`
    Block(Block),
    /// `else if ...`
    If(Box<Stmt>),
}

/// Executable statement AST node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// Local variable binding (`let [mut] x [: ty] = expr;`).
    Let {
        /// Variable identifier.
        name: String,
        /// Whether `mut` was specified.
        mutable: bool,
        /// Optional explicit type annotation.
        ty: Option<TypeAnn>,
        /// Initializer expression.
        init: Expr,
        /// Statement span.
        span: Span,
    },
    /// Assignment or compound assignment (`target op= value;`).
    Assign {
        /// Destination lvalue.
        target: AssignTarget,
        /// Assignment operator.
        op: AssignOp,
        /// Assigned value expression.
        value: Expr,
        /// Statement span.
        span: Span,
    },
    /// Conditional branch (`if cond { ... } [else ...]`).
    If {
        /// Boolean condition.
        cond: Expr,
        /// Then block.
        then_block: Block,
        /// Optional else branch.
        else_branch: Option<ElseBranch>,
        /// Statement span.
        span: Span,
    },
    /// Pre-test loop (`while cond { ... }`).
    While {
        /// Loop condition.
        cond: Expr,
        /// Loop body.
        body: Block,
        /// Statement span.
        span: Span,
    },
    /// Counted integer loop (`for var in start..end { ... }`).
    For {
        /// Loop induction variable name.
        var: String,
        /// Inclusive start bound (`i32`).
        start: Expr,
        /// Exclusive end bound (`i32`).
        end: Expr,
        /// Loop body.
        body: Block,
        /// Statement span.
        span: Span,
    },
    /// Loop `break;`.
    Break(Span),
    /// Loop `continue;`.
    Continue(Span),
    /// Function `return [expr];`.
    Return {
        /// Optional returned value.
        value: Option<Expr>,
        /// Statement span.
        span: Span,
    },
    /// Expression evaluated for side effects (`expr;`).
    Expr {
        /// Inner expression.
        expr: Expr,
        /// Statement span.
        span: Span,
    },
}

/// Unary prefix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// Arithmetic negation `-x` (`i32` or `fix`).
    Neg,
    /// Logical negation `!x` (`bool`).
    Not,
    /// Bitwise complement `~x` (`i32`).
    BitNot,
}

/// Binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `&`
    BitAnd,
    /// `|`
    BitOr,
    /// `^`
    BitXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&` (short-circuiting)
    And,
    /// `||` (short-circuiting)
    Or,
}

/// Expression AST node with source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expr {
    /// Expression variant.
    pub kind: ExprKind,
    /// Source span.
    pub span: Span,
}

/// Variants of [`Expr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprKind {
    /// Signed 64-bit parsed integer literal.
    IntLit(i64),
    /// Signed 64-bit parsed Q16.16 raw fixed-point literal.
    FixLit(i64),
    /// Boolean literal (`true` / `false`).
    BoolLit(bool),
    /// Variable or constant reference.
    Ident(String),
    /// Persistent state array indexing `name[index]`.
    Index {
        /// Array name.
        name: String,
        /// Index expression.
        index: Box<Expr>,
    },
    /// Unary prefix operation.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand.
        expr: Box<Expr>,
    },
    /// Binary infix operation.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left-hand operand.
        lhs: Box<Expr>,
        /// Right-hand operand.
        rhs: Box<Expr>,
    },
    /// Function, conversion builtin, or host syscall call.
    Call {
        /// Callee identifier.
        name: String,
        /// Argument expressions.
        args: Vec<Expr>,
    },
}
