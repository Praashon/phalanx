use std::path::Path;

use phalanx_lang::{CompileOutput, OptLevel, SourceMap, compile};
use phalanx_vm::{VerifiedProgram, decode_program, verify};

/// Built-in showcase champion bots embedded in the CLI binary.
pub const BUILTIN_BOTS: &[(&str, &str)] = &[
    ("kiter", include_str!("../../../bots/kiter.phx")),
    ("berserker", include_str!("../../../bots/berserker.phx")),
    ("sentry", include_str!("../../../bots/sentry.phx")),
    ("vanguard", include_str!("../../../bots/vanguard.phx")),
    ("ghost", include_str!("../../../bots/ghost.phx")),
];

/// Example bot demonstrating deterministic gas-ceiling containment on an infinite loop.
pub const ROGUE_LOOP_SRC: &str = include_str!("../../../bots/examples/rogue_loop.phx");

/// Returns all default built-in champion bot names.
#[must_use]
pub fn default_bot_specs() -> Vec<String> {
    BUILTIN_BOTS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect()
}

/// Resolves a bot specification (built-in name, `.phx` file, or `.phxb` bytecode) into a verified program.
pub fn load_bot(
    spec: &str,
    opt_level: OptLevel,
) -> Result<(String, VerifiedProgram, Option<CompileOutput>), String> {
    if let Some(&(name, src)) = BUILTIN_BOTS
        .iter()
        .find(|&&(n, _)| n.eq_ignore_ascii_case(spec))
    {
        let out =
            compile_with_rendered_errors(name, &format!("builtin:{name}.phx"), src, opt_level)?;
        let vp = out.verified.clone();
        return Ok((name.to_string(), vp, Some(out)));
    }

    let path = Path::new(spec);
    let display_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(spec)
        .to_string();

    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("phxb"))
    {
        let bytes = std::fs::read(path).map_err(|e| format!("failed to read `{spec}`: {e}"))?;
        let prog = decode_program(&bytes).map_err(|e| format!("failed to decode `{spec}`: {e}"))?;
        let name = if prog.name.is_empty() {
            display_name
        } else {
            prog.name.clone()
        };
        let (vp, _) = verify(prog).map_err(|e| format!("verification failed for `{spec}`: {e}"))?;
        return Ok((name, vp, None));
    }

    let src =
        std::fs::read_to_string(path).map_err(|e| format!("failed to read bot `{spec}`: {e}"))?;
    let out = compile_with_rendered_errors(&display_name, spec, &src, opt_level)?;
    let vp = out.verified.clone();
    Ok((display_name, vp, Some(out)))
}

fn compile_with_rendered_errors(
    name: &str,
    file_label: &str,
    src: &str,
    opt_level: OptLevel,
) -> Result<CompileOutput, String> {
    compile(name, src, opt_level).map_err(|diags| {
        let sm = SourceMap::new(src);
        diags
            .iter()
            .map(|d| sm.render_diagnostic(file_label, d))
            .collect::<Vec<_>>()
            .join("\n")
    })
}
