#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Phalanx `.phx` Compiler (`phalanx-lang`).
//!
//! Compiles typed `.phx` autonomous bot source files through lexing, Pratt
//! parsing, strict static type checking, constant evaluation, basic-block IR
//! optimization, loop-aware linear-scan register allocation, and automatic
//! verification via [`phalanx_vm::verify`].

pub mod ast;
pub mod codegen;
pub mod diagnostic;
pub mod ir;
pub mod lexer;
pub mod opt;
pub mod parser;
pub mod typecheck;

use phalanx_vm::{VerificationReport, VerifiedProgram};

pub use diagnostic::{Diagnostic, SourceMap, Span};
pub use opt::{OptLevel, OptStats};

/// Complete artifact returned by [`compile`].
#[derive(Debug, Clone)]
pub struct CompileOutput {
    /// Statically verified bytecode program ready for execution or encoding.
    pub verified: VerifiedProgram,
    /// Bytecode verification report.
    pub report: VerificationReport,
    /// Optimizer statistics collected during compilation.
    pub opt_stats: OptStats,
    /// Human-readable IR dump after optimization passes.
    pub ir_dump: String,
}

/// Compiles a `.phx` source file into a verified Phalanx bytecode program.
pub fn compile(
    name: &str,
    source: &str,
    opt_level: OptLevel,
) -> Result<CompileOutput, Vec<Diagnostic>> {
    let source_map = SourceMap::new(source);
    let tokens = lexer::lex(source)?;
    let module = parser::parse(&tokens, source.len())?;
    let typed = typecheck::check(&module)?;
    let mut ir_prog = ir::lower(&typed);
    let opt_stats = opt::optimize(&mut ir_prog, opt_level);
    let ir_dump = ir::dump_ir(&ir_prog);
    let raw_program = codegen::emit(name, &ir_prog, &source_map);

    match phalanx_vm::verify(raw_program) {
        Ok((verified, report)) => Ok(CompileOutput {
            verified,
            report,
            opt_stats,
            ir_dump,
        }),
        Err(err) => Err(vec![Diagnostic::new(
            format!("internal compiler error: bytecode verifier rejected emitted code: {err}"),
            Span::new(0, 0),
        )]),
    }
}
