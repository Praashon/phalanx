//! Source spans, compiler diagnostics, and rustc-style error rendering.

use core::fmt::Write;

/// Byte-offset span `[start, end)` within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a new byte span.
    #[must_use]
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Merges two spans into the smallest covering span.
    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        let start = if self.start < other.start {
            self.start
        } else {
            other.start
        };
        let end = if self.end > other.end {
            self.end
        } else {
            other.end
        };
        Self { start, end }
    }
}

/// Compiler error or warning diagnostic attached to a source [`Span`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Primary error message.
    pub message: String,
    /// Source span highlighting the offending construct.
    pub span: Span,
    /// Optional suggestion or fix hint.
    pub help: Option<String>,
}

impl Diagnostic {
    /// Creates a diagnostic without a help note.
    #[must_use]
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
            help: None,
        }
    }

    /// Attaches a `help:` hint to this diagnostic.
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

/// Line-offset index for formatting rich multi-line compiler diagnostics.
#[derive(Debug, Clone)]
pub struct SourceMap<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> SourceMap<'a> {
    /// Builds a line index for `source`.
    #[must_use]
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0usize];
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(idx + 1);
            }
        }
        Self {
            source,
            line_starts,
        }
    }

    /// Returns the 1-based `(line, column)` for a byte offset.
    #[must_use]
    pub fn line_col(&self, offset: usize) -> (u32, u32) {
        let clamped = offset.min(self.source.len());
        let line_idx = match self.line_starts.binary_search(&clamped) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line_start = self.line_starts[line_idx];
        let col = self.source[line_start..clamped].chars().count() + 1;
        ((line_idx + 1) as u32, col as u32)
    }

    /// Returns the 1-based line number for a byte offset.
    #[must_use]
    pub fn line_of(&self, offset: usize) -> u32 {
        self.line_col(offset).0
    }

    /// Formats a [`Diagnostic`] with line context and caret underline.
    #[must_use]
    pub fn render_diagnostic(&self, file_name: &str, diag: &Diagnostic) -> String {
        let (line, col) = self.line_col(diag.span.start);
        let line_idx = (line as usize).saturating_sub(1);
        let line_start = self.line_starts.get(line_idx).copied().unwrap_or(0);
        let next_start = self
            .line_starts
            .get(line_idx + 1)
            .copied()
            .unwrap_or(self.source.len());
        let line_text = self.source[line_start..next_start].trim_end_matches(['\r', '\n']);

        let line_num_str = line.to_string();
        let gutter_pad = " ".repeat(line_num_str.len());
        let caret_offset = col.saturating_sub(1) as usize;
        let span_chars = if diag.span.end > diag.span.start {
            let end_clamped = diag.span.end.min(line_start + line_text.len());
            let start_clamped = diag.span.start.min(end_clamped);
            self.source[start_clamped..end_clamped]
                .chars()
                .count()
                .max(1)
        } else {
            1
        };

        let mut out = String::new();
        let _ = writeln!(out, "error: {}", diag.message);
        let _ = writeln!(out, "{gutter_pad}--> {file_name}:{line}:{col}");
        let _ = writeln!(out, "{gutter_pad} |");
        let _ = writeln!(out, "{line_num_str} | {line_text}");
        let _ = writeln!(
            out,
            "{gutter_pad} | {}{}",
            " ".repeat(caret_offset),
            "^".repeat(span_chars)
        );
        if let Some(help) = &diag.help {
            let _ = writeln!(out, "{gutter_pad} = help: {help}");
        }
        out
    }
}
