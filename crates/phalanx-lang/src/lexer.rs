//! Hand-written lexer for the `.phx` bot programming language.

use crate::diagnostic::{Diagnostic, Span};

/// Token categories produced by [`lex`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TokenKind {
    // Keywords
    Const,
    State,
    Fn,
    Let,
    Mut,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    True,
    False,
    TyI32,
    TyFix,
    TyBool,

    // Identifiers & literals
    Ident(String),
    IntLit(i64),
    FixLit(i64),

    // Operators & punctuation
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    EqEq,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,
    PipePipe,
    Bang,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    DotDot,
    Arrow,
    Colon,
    Semi,
    Comma,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
}

/// Lexical token paired with its source [`Span`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Token variant.
    pub kind: TokenKind,
    /// Source byte range.
    pub span: Span,
}

/// Tokenizes `.phx` source code into a vector of [`Token`]s.
pub fn lex(source: &str) -> Result<Vec<Token>, Vec<Diagnostic>> {
    let bytes = source.as_bytes();
    let mut pos = 0usize;
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    while pos < bytes.len() {
        let b = bytes[pos];
        if b.is_ascii_whitespace() {
            pos += 1;
            continue;
        }

        // Line comments: //
        if b == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'/' {
            pos += 2;
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }

        // Block comments: /* ... */
        if b == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'*' {
            let start = pos;
            pos += 2;
            let mut closed = false;
            while pos + 1 < bytes.len() {
                if bytes[pos] == b'*' && bytes[pos + 1] == b'/' {
                    pos += 2;
                    closed = true;
                    break;
                }
                pos += 1;
            }
            if !closed {
                errors.push(Diagnostic::new(
                    "unterminated block comment",
                    Span::new(start, bytes.len()),
                ));
                break;
            }
            continue;
        }

        // Numbers: hex, integer, or Q16.16 fixed-point decimal
        if b.is_ascii_digit() {
            let start = pos;
            match lex_number(source, &mut pos) {
                Ok(kind) => tokens.push(Token {
                    kind,
                    span: Span::new(start, pos),
                }),
                Err(msg) => errors.push(Diagnostic::new(msg, Span::new(start, pos))),
            }
            continue;
        }

        // Identifiers & keywords
        if is_ident_start(b) {
            let start = pos;
            pos += 1;
            while pos < bytes.len() && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            let text = &source[start..pos];
            let kind = keyword_or_ident(text);
            tokens.push(Token {
                kind,
                span: Span::new(start, pos),
            });
            continue;
        }

        // Two-character and single-character punctuation/operators
        let start = pos;
        let next = bytes.get(pos + 1).copied();
        let (kind, len) = match (b, next) {
            (b'<', Some(b'<')) => (TokenKind::Shl, 2),
            (b'>', Some(b'>')) => (TokenKind::Shr, 2),
            (b'=', Some(b'=')) => (TokenKind::EqEq, 2),
            (b'!', Some(b'=')) => (TokenKind::BangEq, 2),
            (b'<', Some(b'=')) => (TokenKind::LtEq, 2),
            (b'>', Some(b'=')) => (TokenKind::GtEq, 2),
            (b'&', Some(b'&')) => (TokenKind::AmpAmp, 2),
            (b'|', Some(b'|')) => (TokenKind::PipePipe, 2),
            (b'+', Some(b'=')) => (TokenKind::PlusEq, 2),
            (b'-', Some(b'=')) => (TokenKind::MinusEq, 2),
            (b'*', Some(b'=')) => (TokenKind::StarEq, 2),
            (b'/', Some(b'=')) => (TokenKind::SlashEq, 2),
            (b'.', Some(b'.')) => (TokenKind::DotDot, 2),
            (b'-', Some(b'>')) => (TokenKind::Arrow, 2),
            (b'+', _) => (TokenKind::Plus, 1),
            (b'-', _) => (TokenKind::Minus, 1),
            (b'*', _) => (TokenKind::Star, 1),
            (b'/', _) => (TokenKind::Slash, 1),
            (b'%', _) => (TokenKind::Percent, 1),
            (b'&', _) => (TokenKind::Amp, 1),
            (b'|', _) => (TokenKind::Pipe, 1),
            (b'^', _) => (TokenKind::Caret, 1),
            (b'~', _) => (TokenKind::Tilde, 1),
            (b'<', _) => (TokenKind::Lt, 1),
            (b'>', _) => (TokenKind::Gt, 1),
            (b'!', _) => (TokenKind::Bang, 1),
            (b'=', _) => (TokenKind::Eq, 1),
            (b':', _) => (TokenKind::Colon, 1),
            (b';', _) => (TokenKind::Semi, 1),
            (b',', _) => (TokenKind::Comma, 1),
            (b'(', _) => (TokenKind::LParen, 1),
            (b')', _) => (TokenKind::RParen, 1),
            (b'{', _) => (TokenKind::LBrace, 1),
            (b'}', _) => (TokenKind::RBrace, 1),
            (b'[', _) => (TokenKind::LBracket, 1),
            (b']', _) => (TokenKind::RBracket, 1),
            _ => {
                let ch = source[pos..].chars().next().unwrap_or('?');
                pos += ch.len_utf8();
                errors.push(Diagnostic::new(
                    format!("unexpected character `{ch}`"),
                    Span::new(start, pos),
                ));
                continue;
            }
        };
        pos += len;
        tokens.push(Token {
            kind,
            span: Span::new(start, pos),
        });
    }

    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn keyword_or_ident(text: &str) -> TokenKind {
    match text {
        "const" => TokenKind::Const,
        "state" => TokenKind::State,
        "fn" => TokenKind::Fn,
        "let" => TokenKind::Let,
        "mut" => TokenKind::Mut,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "for" => TokenKind::For,
        "in" => TokenKind::In,
        "break" => TokenKind::Break,
        "continue" => TokenKind::Continue,
        "return" => TokenKind::Return,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "i32" => TokenKind::TyI32,
        "fix" => TokenKind::TyFix,
        "bool" => TokenKind::TyBool,
        _ => TokenKind::Ident(text.to_owned()),
    }
}

fn lex_number(source: &str, pos: &mut usize) -> Result<TokenKind, String> {
    let bytes = source.as_bytes();
    let start = *pos;

    // Hexadecimal integer: 0x...
    if bytes[*pos] == b'0'
        && *pos + 1 < bytes.len()
        && (bytes[*pos + 1] == b'x' || bytes[*pos + 1] == b'X')
    {
        *pos += 2;
        let hex_start = *pos;
        let mut val: u64 = 0;
        let mut digits = 0usize;
        while *pos < bytes.len() && (bytes[*pos].is_ascii_hexdigit() || bytes[*pos] == b'_') {
            let ch = bytes[*pos];
            *pos += 1;
            if ch == b'_' {
                continue;
            }
            digits += 1;
            let d = match ch {
                b'0'..=b'9' => u64::from(ch - b'0'),
                b'a'..=b'f' => u64::from(ch - b'a' + 10),
                _ => u64::from(ch - b'A' + 10),
            };
            val = val
                .checked_mul(16)
                .and_then(|v| v.checked_add(d))
                .ok_or_else(|| "hex literal overflows 64-bit integer".to_string())?;
        }
        if digits == 0 || *pos == hex_start {
            return Err("expected hexadecimal digits after `0x`".to_string());
        }
        let signed = i64::try_from(val)
            .map_err(|_| "hex literal exceeds signed 64-bit range".to_string())?;
        return Ok(TokenKind::IntLit(signed));
    }

    while *pos < bytes.len() && (bytes[*pos].is_ascii_digit() || bytes[*pos] == b'_') {
        *pos += 1;
    }

    // Check for decimal fixed-point literal (ignore range operator `..`)
    if *pos < bytes.len()
        && bytes[*pos] == b'.'
        && bytes.get(*pos + 1).is_some_and(u8::is_ascii_digit)
    {
        let int_str: String = source[start..*pos].chars().filter(|&c| c != '_').collect();
        let int_part: i64 = int_str
            .parse()
            .map_err(|_| "integer part of fixed-point literal overflows".to_string())?;
        *pos += 1; // consume '.'
        let frac_start = *pos;
        while *pos < bytes.len() && (bytes[*pos].is_ascii_digit() || bytes[*pos] == b'_') {
            *pos += 1;
        }
        let frac_clean: String = source[frac_start..*pos]
            .chars()
            .filter(|&c| c != '_')
            .collect();
        if frac_clean.is_empty() {
            return Err("expected fractional digits after decimal point".to_string());
        }

        // Pure-integer deterministic decimal-to-Q16.16 conversion (up to 9 digits).
        let take_len = frac_clean.len().min(9);
        let frac_slice = &frac_clean[..take_len];
        let frac_num: u128 = frac_slice
            .parse()
            .map_err(|_| "fractional part invalid".to_string())?;
        let denom = 10u128.pow(take_len as u32);
        let frac_q16 = ((frac_num * 65536) + (denom / 2)) / denom;
        let total = int_part
            .checked_mul(65536)
            .and_then(|v| v.checked_add(frac_q16 as i64))
            .ok_or_else(|| "fixed-point literal overflows Q16.16 range".to_string())?;
        return Ok(TokenKind::FixLit(total));
    }

    let clean: String = source[start..*pos].chars().filter(|&c| c != '_').collect();
    let val: i64 = clean
        .parse()
        .map_err(|_| "integer literal overflows 64-bit range".to_string())?;
    Ok(TokenKind::IntLit(val))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_fixed_point_deterministically_without_floats() {
        let toks = lex("let x: fix = 1.5 + 3.14159; for i in 0..10 {}").unwrap();
        assert!(matches!(toks[5].kind, TokenKind::FixLit(98304)));
        assert!(matches!(toks[7].kind, TokenKind::FixLit(205887)));
        assert!(toks.iter().any(|t| t.kind == TokenKind::DotDot));
    }
}
