use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use phalanx_cli::{bots, server, tui};
use phalanx_lang::OptLevel;
use phalanx_sim::{
    ArenaConfig, decode_replay, encode_replay, record_match, simulate_replay_frames, verify_replay,
};
use phalanx_tourney::{BotEntry, TournamentConfig, TournamentFormat, run_tournament};
use phalanx_vm::disasm::disassemble;
use phalanx_vm::{decode_program, encode_program, verify};

#[derive(Parser)]
#[command(
    name = "phalanx",
    version,
    about = "Phalanx: Sandboxed Autonomous Bot Battle Arena, Bytecode VM & Deterministic Physics Engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Launch the interactive Phalanx Web Studio in your browser (default when run with no arguments).
    Serve {
        /// Bind host address.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Listen port.
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Watch a live autonomous bot match in the terminal TUI or headless mode.
    Watch {
        /// Bot specs (built-in names: kiter, berserker, sentry, vanguard, ghost, or .phx/.phxb paths).
        bots: Vec<String>,
        /// Deterministic match seed.
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Maximum match ticks.
        #[arg(long, default_value_t = 1200)]
        ticks: u32,
        /// Run without TUI and print final match telemetry to stdout.
        #[arg(long)]
        headless: bool,
        /// Save self-contained deterministic `.phxr` replay file.
        #[arg(long)]
        save_replay: Option<PathBuf>,
        /// Use ASCII dot markers instead of Braille in the TUI arena canvas.
        #[arg(long)]
        ascii: bool,
    },
    /// Compile a `.phx` bot, verify its bytecode safety, and optionally inspect IR/disassembly.
    Compile {
        /// Path to `.phx` source file (or built-in bot name).
        file: String,
        /// Output `.phxb` binary path.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Print optimized basic-block IR.
        #[arg(long)]
        dump_ir: bool,
        /// Print annotated VM assembly disassembly.
        #[arg(long)]
        disasm: bool,
        /// Disable IR optimization passes (`OptLevel::None`).
        #[arg(long)]
        no_opt: bool,
        /// Print detailed static verification report.
        #[arg(long)]
        verify_report: bool,
    },
    /// Decode and statically verify a compiled `.phxb` bytecode file.
    Verify {
        /// Path to `.phxb` binary file.
        file: PathBuf,
    },
    /// Play back or cryptographically verify a `.phxr` match replay.
    Replay {
        /// Path to `.phxr` replay file.
        file: PathBuf,
        /// Re-simulate every tick and verify every 64-bit state hash.
        #[arg(long)]
        verify: bool,
        /// Print summary without launching interactive TUI.
        #[arg(long)]
        headless: bool,
        /// Use ASCII dot markers in TUI mode.
        #[arg(long)]
        ascii: bool,
    },
    /// Run a parallel work-stealing Elo tournament across multiple CPU threads.
    Tourney {
        /// Bot specs (defaults to all 5 built-in champion bots).
        bots: Vec<String>,
        /// Number of rounds per pairing (1v1) or royale rounds.
        #[arg(long, default_value_t = 5)]
        rounds: u32,
        /// Worker threads (defaults to CPU count).
        #[arg(long)]
        threads: Option<usize>,
        /// Tournament format.
        #[arg(long, value_enum, default_value_t = TourneyFormatArg::RoundRobin)]
        format: TourneyFormatArg,
        /// Base RNG seed.
        #[arg(long, default_value_t = 1337)]
        seed: u64,
        /// Cross-verify every match twice across worker threads for bit-exact determinism.
        #[arg(long)]
        audit_determinism: bool,
        /// Write machine-readable JSON tournament report to file.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TourneyFormatArg {
    RoundRobin,
    Royale,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Serve {
        host: "127.0.0.1".to_string(),
        port: 8080,
    });
    match command {
        Command::Serve { host, port } => server::start_server(&host, port),
        Command::Watch {
            bots,
            seed,
            ticks,
            headless,
            save_replay,
            ascii,
        } => {
            let specs = if bots.is_empty() {
                bots::default_bot_specs()
            } else {
                bots
            };
            let mut entries = Vec::with_capacity(specs.len());
            for s in &specs {
                let (name, vp, _) = bots::load_bot(s, OptLevel::Standard)?;
                entries.push((name, vp));
            }
            let config = ArenaConfig {
                max_ticks: ticks,
                ..ArenaConfig::default()
            };

            if headless || save_replay.is_some() {
                let (summary, replay, _) = record_match(entries.clone(), seed, config.clone())
                    .map_err(|e| e.to_string())?;
                if let Some(path) = save_replay {
                    let bytes = encode_replay(&replay);
                    if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| format!("failed to create directory: {e}"))?;
                    }
                    std::fs::write(&path, bytes)
                        .map_err(|e| format!("failed to write replay `{}`: {e}", path.display()))?;
                    println!("Saved replay to {}", path.display());
                }
                if headless {
                    print_match_summary(&summary);
                    return Ok(());
                }
            }

            tui::run_live_tui(entries, seed, config, ascii).map_err(|e| format!("TUI error: {e}"))
        }
        Command::Compile {
            file,
            out,
            dump_ir,
            disasm,
            no_opt,
            verify_report,
        } => {
            let opt = if no_opt {
                OptLevel::None
            } else {
                OptLevel::Standard
            };
            let (name, vp, compile_out) = bots::load_bot(&file, opt)?;
            if let Some(co) = &compile_out {
                if dump_ir {
                    println!("=== OPTIMIZED IR ({name}) ===\n{}", co.ir_dump);
                }
                if disasm {
                    println!(
                        "=== DISASSEMBLY ({name}) ===\n{}",
                        disassemble(vp.program())
                    );
                }
                if verify_report || (!dump_ir && !disasm && out.is_none()) {
                    println!(
                        "Compiled & verified `{}`: {} funcs, {} insns, {} basic blocks, {} back-edges, max depth {}, fingerprint 0x{:016x}",
                        co.report.program_name,
                        co.report.functions,
                        co.report.instructions,
                        co.report.basic_blocks,
                        co.report.back_edges,
                        co.report.max_call_depth,
                        co.report.fingerprint
                    );
                    println!(
                        "Optimizer: {} consts folded, {} algebraic simplifications, {} copies propagated, {} dead insts, {} dead blocks removed",
                        co.opt_stats.consts_folded,
                        co.opt_stats.algebraic_simplifications,
                        co.opt_stats.copies_propagated,
                        co.opt_stats.dead_insts_removed,
                        co.opt_stats.dead_blocks_removed
                    );
                }
            } else if disasm {
                println!("{}", disassemble(vp.program()));
            }

            if let Some(out_path) = out {
                let bytes = encode_program(vp.program()).map_err(|e| e.to_string())?;
                std::fs::write(&out_path, &bytes)
                    .map_err(|e| format!("failed to write `{}`: {e}", out_path.display()))?;
                println!(
                    "Wrote {} ({} bytes, CRC32C protected)",
                    out_path.display(),
                    bytes.len()
                );
            }
            Ok(())
        }
        Command::Verify { file } => {
            let bytes = std::fs::read(&file)
                .map_err(|e| format!("failed to read `{}`: {e}", file.display()))?;
            let prog = decode_program(&bytes).map_err(|e| e.to_string())?;
            let (_, report) = verify(prog).map_err(|e| e.to_string())?;
            println!(
                "VERIFIED `{}`: {} funcs, {} insns, {} blocks, {} loops, max call depth {}, fingerprint 0x{:016x}",
                report.program_name,
                report.functions,
                report.instructions,
                report.basic_blocks,
                report.back_edges,
                report.max_call_depth,
                report.fingerprint
            );
            Ok(())
        }
        Command::Replay {
            file,
            verify: verify_flag,
            headless,
            ascii,
        } => {
            let bytes = std::fs::read(&file)
                .map_err(|e| format!("failed to read `{}`: {e}", file.display()))?;
            let replay = decode_replay(&bytes).map_err(|e| e.to_string())?;
            if verify_flag {
                let vr = verify_replay(&replay).map_err(|e| e.to_string())?;
                println!(
                    "DETERMINISM VERIFIED: {} ticks matched 64-bit hashes (final=0x{:016x}, winner={})",
                    vr.ticks_verified,
                    vr.final_state_hash,
                    vr.winner_name.as_deref().unwrap_or("Draw")
                );
                return Ok(());
            }

            let mut programs = Vec::with_capacity(replay.bots.len());
            for (name, phxb) in &replay.bots {
                let prog = decode_program(phxb).map_err(|e| e.to_string())?;
                let (vp, _) = verify(prog).map_err(|e| e.to_string())?;
                programs.push((name.clone(), vp));
            }
            let (summary, frames) = simulate_replay_frames(&replay).map_err(|e| e.to_string())?;
            if headless {
                print_match_summary(&summary);
                return Ok(());
            }

            tui::run_replay_tui(programs, replay.seed, replay.config, frames, ascii)
                .map_err(|e| format!("TUI error: {e}"))
        }
        Command::Tourney {
            bots,
            rounds,
            threads,
            format,
            seed,
            audit_determinism,
            json,
        } => {
            let specs = if bots.is_empty() {
                bots::default_bot_specs()
            } else {
                bots
            };
            let mut entries = Vec::with_capacity(specs.len());
            for s in &specs {
                let (name, vp, _) = bots::load_bot(s, OptLevel::Standard)?;
                entries.push(BotEntry { name, program: vp });
            }
            let worker_threads = threads
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, usize::from));
            let cfg = TournamentConfig {
                format: match format {
                    TourneyFormatArg::RoundRobin => TournamentFormat::RoundRobin1v1,
                    TourneyFormatArg::Royale => TournamentFormat::Royale,
                },
                rounds,
                threads: worker_threads,
                base_seed: seed,
                arena: ArenaConfig::default(),
                audit_determinism,
            };
            let report = run_tournament(&entries, &cfg);
            print_tourney_report(&report);

            if let Some(json_path) = json {
                let payload = serde_json::to_string_pretty(&report)
                    .map_err(|e| format!("failed to serialize JSON: {e}"))?;
                std::fs::write(&json_path, payload)
                    .map_err(|e| format!("failed to write `{}`: {e}", json_path.display()))?;
                println!("Wrote JSON tournament report to {}", json_path.display());
            }

            if let Some(audit) = &report.audit
                && !audit.passed
            {
                return Err(format!(
                    "determinism audit failed: {}",
                    audit.mismatch_detail.as_deref().unwrap_or("unknown")
                ));
            }
            Ok(())
        }
    }
}

fn print_match_summary(summary: &phalanx_sim::MatchSummary) {
    let winner_str = summary
        .winner
        .and_then(|w| summary.bots.get(w))
        .map_or("Draw", |b| b.name.as_str());
    println!(
        "Match completed in {} ticks | Winner: {} | Final Hash: 0x{:016x} | VM Insns: {}",
        summary.ticks_elapsed, winner_str, summary.final_state_hash, summary.total_vm_instructions
    );
    println!(
        "{:<12} {:>4} {:>5} {:>6} {:>7} {:>9} {:>5} {:>4}",
        "Bot", "HP", "Kills", "Dmg", "Surv", "Gas/Tick", "Traps", "OOG"
    );
    for b in &summary.bots {
        println!(
            "{:<12} {:>4} {:>5} {:>6} {:>7} {:>9.1} {:>5} {:>4}",
            b.name,
            b.hp,
            b.kills,
            b.damage_dealt,
            b.survival_ticks,
            b.avg_gas_per_tick,
            b.traps,
            b.out_of_gas_count
        );
    }
}

fn print_tourney_report(report: &phalanx_tourney::TournamentReport) {
    println!(
        "=== PHALANX TOURNAMENT ({}) ===\nThreads: {} | Matches: {} | Ticks: {} | VM Insns: {} | Elapsed: {:.3}s | {:.1} matches/s | {:.2} VM MIPS",
        report.format_name,
        report.threads_used,
        report.matches_played,
        report.total_ticks,
        report.total_vm_instructions,
        report.elapsed_secs,
        report.matches_per_sec,
        report.vm_mips
    );
    println!(
        "{:<4} {:<12} {:>7} {:>4} {:>4} {:>4} {:>6} {:>7} {:>8} {:>5}",
        "Rank", "Bot", "Elo", "W", "L", "D", "Kills", "Dmg", "Gas/Tk", "Traps"
    );
    for (i, s) in report.standings.iter().enumerate() {
        println!(
            "#{:<3} {:<12} {:>7.1} {:>4} {:>4} {:>4} {:>6} {:>7} {:>8.1} {:>5}",
            i + 1,
            s.name,
            s.elo,
            s.wins,
            s.losses,
            s.draws,
            s.kills,
            s.damage_dealt,
            s.avg_gas_per_tick,
            s.traps
        );
    }
    if let Some(audit) = &report.audit {
        println!(
            "Determinism Audit: {} ({}/{} matches matched across worker threads)",
            if audit.passed { "PASSED" } else { "FAILED" },
            audit.hashes_matched,
            audit.matches_audited
        );
    }
}
