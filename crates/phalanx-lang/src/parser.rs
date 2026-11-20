//! Recursive-descent and Pratt expression parser for `.phx`.

use crate::ast::{
    AssignOp, AssignTarget, BinaryOp, Block, ElseBranch, Expr, ExprKind, FnDecl, Item, Module,
    Param, Stmt, TypeAnn, UnaryOp,
};
use crate::diagnostic::{Diagnostic, Span};
use crate::lexer::{Token, TokenKind};

/// Parses a slice of lexical [`Token`]s into a [`Module`] AST.
pub fn parse(tokens: &[Token], source_len: usize) -> Result<Module, Vec<Diagnostic>> {
    let mut parser = Parser {
        tokens,
        pos: 0,
        source_len,
    };
    let mut items = Vec::new();
    let mut errors = Vec::new();

    while !parser.at_eof() {
        match parser.parse_item() {
            Ok(item) => items.push(item),
            Err(err) => {
                errors.push(err);
                parser.synchronize_item();
            }
        }
    }

    if errors.is_empty() {
        Ok(Module { items })
    } else {
        Err(errors)
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    source_len: usize,
}

impl<'a> Parser<'a> {
    fn at_eof(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<&TokenKind> {
        self.peek().map(|t| &t.kind)
    }

    fn eof_span(&self) -> Span {
        Span::new(self.source_len, self.source_len)
    }

    fn cur_span(&self) -> Span {
        self.peek().map_or_else(|| self.eof_span(), |t| t.span)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(tok)
    }

    fn check(&self, expected: &TokenKind) -> bool {
        self.peek_kind() == Some(expected)
    }

    fn eat(&mut self, expected: &TokenKind) -> bool {
        if self.check(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &TokenKind, msg: &str) -> Result<Span, Diagnostic> {
        if let Some(tok) = self.peek() {
            if &tok.kind == expected {
                let span = tok.span;
                self.pos += 1;
                return Ok(span);
            }
            return Err(Diagnostic::new(msg, tok.span));
        }
        Err(Diagnostic::new(msg, self.eof_span()))
    }

    fn expect_ident(&mut self, msg: &str) -> Result<(String, Span), Diagnostic> {
        if let Some(tok) = self.peek() {
            if let TokenKind::Ident(name) = &tok.kind {
                let name = name.clone();
                let span = tok.span;
                self.pos += 1;
                return Ok((name, span));
            }
            return Err(Diagnostic::new(msg, tok.span));
        }
        Err(Diagnostic::new(msg, self.eof_span()))
    }

    fn parse_type(&mut self) -> Result<(TypeAnn, Span), Diagnostic> {
        let Some(tok) = self.advance().cloned() else {
            return Err(Diagnostic::new(
                "expected type (`i32`, `fix`, or `bool`)",
                self.eof_span(),
            ));
        };
        let ty = match tok.kind {
            TokenKind::TyI32 => TypeAnn::Int,
            TokenKind::TyFix => TypeAnn::Fix,
            TokenKind::TyBool => TypeAnn::Bool,
            _ => {
                return Err(Diagnostic::new(
                    "expected type (`i32`, `fix`, or `bool`)",
                    tok.span,
                ));
            }
        };
        Ok((ty, tok.span))
    }

    fn synchronize_item(&mut self) {
        while let Some(k) = self.peek_kind() {
            if matches!(k, TokenKind::Const | TokenKind::State | TokenKind::Fn) {
                break;
            }
            self.pos += 1;
        }
    }

    fn parse_item(&mut self) -> Result<Item, Diagnostic> {
        let start = self.cur_span();
        match self.peek_kind() {
            Some(TokenKind::Const) => {
                self.pos += 1;
                let (name, _) = self.expect_ident("expected constant name")?;
                self.expect(
                    &TokenKind::Colon,
                    "expected `:` and type annotation on `const`",
                )?;
                let (ty, _) = self.parse_type()?;
                self.expect(&TokenKind::Eq, "expected `=` in `const` declaration")?;
                let value = self.parse_expr(0)?;
                let end = self.expect(&TokenKind::Semi, "expected `;` after `const` value")?;
                Ok(Item::Const {
                    name,
                    ty,
                    value,
                    span: start.merge(end),
                })
            }
            Some(TokenKind::State) => {
                self.pos += 1;
                let (name, _) = self.expect_ident("expected state variable name")?;
                self.expect(&TokenKind::Colon, "expected `:` after state name")?;
                if self.eat(&TokenKind::LBracket) {
                    let (elem_ty, _) = self.parse_type()?;
                    self.expect(
                        &TokenKind::Semi,
                        "expected `;` in state array type `[T; N]`",
                    )?;
                    let len_tok = self.advance().cloned().ok_or_else(|| {
                        Diagnostic::new("expected positive integer array length", self.eof_span())
                    })?;
                    let len = match len_tok.kind {
                        TokenKind::IntLit(n) if n > 0 && n <= 64 => n as usize,
                        _ => {
                            return Err(Diagnostic::new(
                                "state array length must be a positive integer <= 64",
                                len_tok.span,
                            ));
                        }
                    };
                    self.expect(
                        &TokenKind::RBracket,
                        "expected `]` after state array length",
                    )?;
                    let end = self.expect(&TokenKind::Semi, "expected `;` after state array")?;
                    Ok(Item::StateArray {
                        name,
                        elem_ty,
                        len,
                        span: start.merge(end),
                    })
                } else {
                    let (ty, _) = self.parse_type()?;
                    self.expect(&TokenKind::Eq, "expected `=` and initial value for `state`")?;
                    let init = self.parse_expr(0)?;
                    let end =
                        self.expect(&TokenKind::Semi, "expected `;` after state declaration")?;
                    Ok(Item::StateScalar {
                        name,
                        ty,
                        init,
                        span: start.merge(end),
                    })
                }
            }
            Some(TokenKind::Fn) => {
                self.pos += 1;
                let (name, _) = self.expect_ident("expected function name")?;
                self.expect(&TokenKind::LParen, "expected `(` after function name")?;
                let mut params = Vec::new();
                if !self.check(&TokenKind::RParen) {
                    loop {
                        let p_start = self.cur_span();
                        let (pname, _) = self.expect_ident("expected parameter name")?;
                        self.expect(&TokenKind::Colon, "expected `:` after parameter name")?;
                        let (pty, p_end) = self.parse_type()?;
                        params.push(Param {
                            name: pname,
                            ty: pty,
                            span: p_start.merge(p_end),
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::RParen, "expected `)` after parameter list")?;
                let ret = if self.eat(&TokenKind::Arrow) {
                    Some(self.parse_type()?.0)
                } else {
                    None
                };
                let body = self.parse_block()?;
                let span = start.merge(body.span);
                Ok(Item::Fn(FnDecl {
                    name,
                    params,
                    ret,
                    body,
                    span,
                }))
            }
            _ => Err(Diagnostic::new(
                "expected top-level `const`, `state`, or `fn` declaration",
                start,
            )),
        }
    }

    fn parse_block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.expect(&TokenKind::LBrace, "expected `{` to start block")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            stmts.push(self.parse_stmt()?);
        }
        let end = self.expect(&TokenKind::RBrace, "expected `}` to close block")?;
        Ok(Block {
            stmts,
            span: start.merge(end),
        })
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.cur_span();
        match self.peek_kind() {
            Some(TokenKind::Let) => {
                self.pos += 1;
                let mutable = self.eat(&TokenKind::Mut);
                let (name, _) = self.expect_ident("expected variable name after `let`")?;
                let ty = if self.eat(&TokenKind::Colon) {
                    Some(self.parse_type()?.0)
                } else {
                    None
                };
                self.expect(&TokenKind::Eq, "expected `=` in `let` binding")?;
                let init = self.parse_expr(0)?;
                let end = self.expect(&TokenKind::Semi, "expected `;` after `let` binding")?;
                Ok(Stmt::Let {
                    name,
                    mutable,
                    ty,
                    init,
                    span: start.merge(end),
                })
            }
            Some(TokenKind::If) => self.parse_if_stmt(),
            Some(TokenKind::While) => {
                self.pos += 1;
                let cond = self.parse_expr(0)?;
                let body = self.parse_block()?;
                let span = start.merge(body.span);
                Ok(Stmt::While { cond, body, span })
            }
            Some(TokenKind::For) => {
                self.pos += 1;
                let (var, _) = self.expect_ident("expected loop variable name after `for`")?;
                self.expect(&TokenKind::In, "expected `in` after `for` variable")?;
                let range_start = self.parse_expr(0)?;
                self.expect(&TokenKind::DotDot, "expected `..` in `for` range")?;
                let range_end = self.parse_expr(0)?;
                let body = self.parse_block()?;
                let span = start.merge(body.span);
                Ok(Stmt::For {
                    var,
                    start: range_start,
                    end: range_end,
                    body,
                    span,
                })
            }
            Some(TokenKind::Break) => {
                self.pos += 1;
                let end = self.expect(&TokenKind::Semi, "expected `;` after `break`")?;
                Ok(Stmt::Break(start.merge(end)))
            }
            Some(TokenKind::Continue) => {
                self.pos += 1;
                let end = self.expect(&TokenKind::Semi, "expected `;` after `continue`")?;
                Ok(Stmt::Continue(start.merge(end)))
            }
            Some(TokenKind::Return) => {
                self.pos += 1;
                let value = if self.check(&TokenKind::Semi) {
                    None
                } else {
                    Some(self.parse_expr(0)?)
                };
                let end = self.expect(&TokenKind::Semi, "expected `;` after `return`")?;
                Ok(Stmt::Return {
                    value,
                    span: start.merge(end),
                })
            }
            _ => self.parse_assign_or_expr_stmt(),
        }
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.expect(&TokenKind::If, "expected `if`")?;
        let cond = self.parse_expr(0)?;
        let then_block = self.parse_block()?;
        let mut end = then_block.span;
        let else_branch = if self.eat(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                let nested = self.parse_if_stmt()?;
                if let Stmt::If { span, .. } = &nested {
                    end = *span;
                }
                Some(ElseBranch::If(Box::new(nested)))
            } else {
                let blk = self.parse_block()?;
                end = blk.span;
                Some(ElseBranch::Block(blk))
            }
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then_block,
            else_branch,
            span: start.merge(end),
        })
    }

    fn parse_assign_or_expr_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let start = self.cur_span();
        let lhs = self.parse_expr(0)?;
        if let Some(op) = self.try_parse_assign_op() {
            let target = match lhs.kind {
                ExprKind::Ident(name) => AssignTarget::Var(name),
                ExprKind::Index { name, index } => AssignTarget::Index {
                    name,
                    index: *index,
                },
                _ => {
                    return Err(Diagnostic::new(
                        "invalid assignment target; expected variable or state array element",
                        lhs.span,
                    ));
                }
            };
            let value = self.parse_expr(0)?;
            let end = self.expect(&TokenKind::Semi, "expected `;` after assignment")?;
            return Ok(Stmt::Assign {
                target,
                op,
                value,
                span: start.merge(end),
            });
        }
        let end = self.expect(&TokenKind::Semi, "expected `;` after expression")?;
        Ok(Stmt::Expr {
            expr: lhs,
            span: start.merge(end),
        })
    }

    fn try_parse_assign_op(&mut self) -> Option<AssignOp> {
        let op = match self.peek_kind()? {
            TokenKind::Eq => AssignOp::Set,
            TokenKind::PlusEq => AssignOp::Add,
            TokenKind::MinusEq => AssignOp::Sub,
            TokenKind::StarEq => AssignOp::Mul,
            TokenKind::SlashEq => AssignOp::Div,
            _ => return None,
        };
        self.pos += 1;
        Some(op)
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, Diagnostic> {
        let mut lhs = self.parse_prefix()?;

        while let Some((op, l_bp, r_bp)) = self.peek_infix_bp() {
            if l_bp < min_bp {
                break;
            }
            self.pos += 1;
            let rhs = self.parse_expr(r_bp)?;
            let span = lhs.span.merge(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }

        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, Diagnostic> {
        let Some(tok) = self.advance().cloned() else {
            return Err(Diagnostic::new("expected expression", self.eof_span()));
        };
        match tok.kind {
            TokenKind::IntLit(v) => Ok(Expr {
                kind: ExprKind::IntLit(v),
                span: tok.span,
            }),
            TokenKind::FixLit(v) => Ok(Expr {
                kind: ExprKind::FixLit(v),
                span: tok.span,
            }),
            TokenKind::True => Ok(Expr {
                kind: ExprKind::BoolLit(true),
                span: tok.span,
            }),
            TokenKind::False => Ok(Expr {
                kind: ExprKind::BoolLit(false),
                span: tok.span,
            }),
            TokenKind::Minus => {
                let expr = self.parse_expr(11)?;
                let span = tok.span.merge(expr.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::Bang => {
                let expr = self.parse_expr(11)?;
                let span = tok.span.merge(expr.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::Not,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::Tilde => {
                let expr = self.parse_expr(11)?;
                let span = tok.span.merge(expr.span);
                Ok(Expr {
                    kind: ExprKind::Unary {
                        op: UnaryOp::BitNot,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::LParen => {
                let mut inner = self.parse_expr(0)?;
                let close = self.expect(&TokenKind::RParen, "expected `)` after expression")?;
                inner.span = tok.span.merge(close);
                Ok(inner)
            }
            TokenKind::Ident(name) => {
                if self.eat(&TokenKind::LParen) {
                    let mut args = Vec::new();
                    if !self.check(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    let close =
                        self.expect(&TokenKind::RParen, "expected `)` after call arguments")?;
                    Ok(Expr {
                        kind: ExprKind::Call { name, args },
                        span: tok.span.merge(close),
                    })
                } else if self.eat(&TokenKind::LBracket) {
                    let index = self.parse_expr(0)?;
                    let close =
                        self.expect(&TokenKind::RBracket, "expected `]` after array index")?;
                    Ok(Expr {
                        kind: ExprKind::Index {
                            name,
                            index: Box::new(index),
                        },
                        span: tok.span.merge(close),
                    })
                } else {
                    Ok(Expr {
                        kind: ExprKind::Ident(name),
                        span: tok.span,
                    })
                }
            }
            _ => Err(Diagnostic::new(
                "unexpected token while parsing expression",
                tok.span,
            )),
        }
    }

    fn peek_infix_bp(&self) -> Option<(BinaryOp, u8, u8)> {
        let op = match self.peek_kind()? {
            TokenKind::PipePipe => (BinaryOp::Or, 1, 2),
            TokenKind::AmpAmp => (BinaryOp::And, 2, 3),
            TokenKind::Pipe => (BinaryOp::BitOr, 3, 4),
            TokenKind::Caret => (BinaryOp::BitXor, 4, 5),
            TokenKind::Amp => (BinaryOp::BitAnd, 5, 6),
            TokenKind::EqEq => (BinaryOp::Eq, 6, 7),
            TokenKind::BangEq => (BinaryOp::Ne, 6, 7),
            TokenKind::Lt => (BinaryOp::Lt, 7, 8),
            TokenKind::LtEq => (BinaryOp::Le, 7, 8),
            TokenKind::Gt => (BinaryOp::Gt, 7, 8),
            TokenKind::GtEq => (BinaryOp::Ge, 7, 8),
            TokenKind::Shl => (BinaryOp::Shl, 8, 9),
            TokenKind::Shr => (BinaryOp::Shr, 8, 9),
            TokenKind::Plus => (BinaryOp::Add, 9, 10),
            TokenKind::Minus => (BinaryOp::Sub, 9, 10),
            TokenKind::Star => (BinaryOp::Mul, 10, 11),
            TokenKind::Slash => (BinaryOp::Div, 10, 11),
            TokenKind::Percent => (BinaryOp::Mod, 10, 11),
            _ => return None,
        };
        Some(op)
    }
}
