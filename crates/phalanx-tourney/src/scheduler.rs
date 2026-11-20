#![forbid(unsafe_code)]

//! Tournament scheduling, deterministic reduction, and parallel determinism auditing.

use std::sync::atomic::Ordering;
use std::time::Instant;

use phalanx_sim::{ArenaConfig, MatchSummary, Simulation};
use phalanx_vm::VerifiedProgram;
use serde::{Deserialize, Serialize};

use crate::elo::{BotStanding, MatchRankKey, apply_match_to_standings};
use crate::pool::{PoolTelemetry, execute_work_stealing};

/// Tournament pairing format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TournamentFormat {
    /// Every pair of bots fights 1v1 across `rounds` distinct seeds.
    RoundRobin1v1,
    /// All bots fight simultaneously in `rounds` multi-bot Battle Royale matches.
    Royale,
}

/// Compiled bot entry submitted to a tournament.
#[derive(Debug, Clone)]
pub struct BotEntry {
    /// Display name.
    pub name: String,
    /// Statically verified bytecode program.
    pub program: VerifiedProgram,
}

/// Configuration for [`run_tournament`].
#[derive(Debug, Clone)]
pub struct TournamentConfig {
    /// Tournament pairing format.
    pub format: TournamentFormat,
    /// Rounds per pairing (1v1) or total royale rounds.
    pub rounds: u32,
    /// Worker threads to spawn (defaults to logical CPU count).
    pub threads: usize,
    /// Base RNG seed.
    pub base_seed: u64,
    /// Arena rules and pillar configuration.
    pub arena: ArenaConfig,
    /// If `true`, re-runs every scheduled match on an independent task and asserts identical hashes.
    pub audit_determinism: bool,
}

/// Results of parallel determinism cross-verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeterminismAudit {
    /// Total matches double-executed across worker threads.
    pub matches_audited: usize,
    /// Total per-match state hashes verified identical.
    pub hashes_matched: usize,
    /// Whether all audited pairs produced identical ticks and final state hashes.
    pub passed: bool,
    /// First divergence description if `passed == false`.
    pub mismatch_detail: Option<String>,
}

/// Complete tournament report including sorted standings, throughput, and determinism audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TournamentReport {
    /// Tournament format label.
    pub format_name: String,
    /// Worker threads used.
    pub threads_used: usize,
    /// Total matches executed (excluding duplicate audit runs).
    pub matches_played: usize,
    /// Cumulative simulated ticks across primary matches.
    pub total_ticks: u64,
    /// Cumulative VM instructions executed across primary matches.
    pub total_vm_instructions: u64,
    /// Wall-clock duration in seconds.
    pub elapsed_secs: f64,
    /// Throughput in matches/second.
    pub matches_per_sec: f64,
    /// Throughput in millions of VM instructions/second.
    pub vm_mips: f64,
    /// Final leaderboard sorted by Elo descending.
    pub standings: Vec<BotStanding>,
    /// Optional determinism audit result.
    pub audit: Option<DeterminismAudit>,
}

struct MatchTask {
    match_idx: usize,
    participants: Vec<usize>,
    seed: u64,
    is_audit_duplicate: bool,
}

struct MatchOutcome {
    match_idx: usize,
    participants: Vec<usize>,
    is_audit_duplicate: bool,
    summary: MatchSummary,
}

/// Executes a parallel tournament and reduces results into a deterministic [`TournamentReport`].
#[must_use]
pub fn run_tournament(entries: &[BotEntry], config: &TournamentConfig) -> TournamentReport {
    let mut tasks = Vec::new();
    let mut next_idx = 0usize;

    match config.format {
        TournamentFormat::RoundRobin1v1 => {
            for i in 0..entries.len() {
                for j in (i + 1)..entries.len() {
                    for r in 0..config.rounds {
                        let seed = config.base_seed.wrapping_add(
                            (next_idx as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                        );
                        let _ = r;
                        tasks.push(MatchTask {
                            match_idx: next_idx,
                            participants: vec![i, j],
                            seed,
                            is_audit_duplicate: false,
                        });
                        if config.audit_determinism {
                            tasks.push(MatchTask {
                                match_idx: next_idx,
                                participants: vec![i, j],
                                seed,
                                is_audit_duplicate: true,
                            });
                        }
                        next_idx += 1;
                    }
                }
            }
        }
        TournamentFormat::Royale => {
            let all_ids: Vec<usize> = (0..entries.len()).collect();
            for r in 0..config.rounds {
                let seed = config
                    .base_seed
                    .wrapping_add((u64::from(r) + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                tasks.push(MatchTask {
                    match_idx: next_idx,
                    participants: all_ids.clone(),
                    seed,
                    is_audit_duplicate: false,
                });
                if config.audit_determinism {
                    tasks.push(MatchTask {
                        match_idx: next_idx,
                        participants: all_ids.clone(),
                        seed,
                        is_audit_duplicate: true,
                    });
                }
                next_idx += 1;
            }
        }
    }

    let threads = config.threads.max(1);
    let telemetry = PoolTelemetry::default();
    let start = Instant::now();

    let raw_outcomes = execute_work_stealing(&tasks, threads, &telemetry, |_, task, tel| {
        let match_entries: Vec<_> = task
            .participants
            .iter()
            .map(|&idx| (entries[idx].name.clone(), entries[idx].program.clone()))
            .collect();
        let mut sim = Simulation::new(match_entries, task.seed, config.arena.clone());
        let summary = sim.run_to_completion();
        tel.matches_done.fetch_add(1, Ordering::Relaxed);
        tel.total_ticks
            .fetch_add(u64::from(summary.ticks_elapsed), Ordering::Relaxed);
        tel.total_vm_instructions
            .fetch_add(summary.total_vm_instructions, Ordering::Relaxed);
        MatchOutcome {
            match_idx: task.match_idx,
            participants: task.participants.clone(),
            is_audit_duplicate: task.is_audit_duplicate,
            summary,
        }
    });

    let elapsed_secs = start.elapsed().as_secs_f64().max(1e-9);

    let mut primary: Vec<MatchOutcome> = Vec::with_capacity(next_idx);
    let mut audits: Vec<MatchOutcome> = Vec::new();
    for (_, outcome) in raw_outcomes {
        if outcome.is_audit_duplicate {
            audits.push(outcome);
        } else {
            primary.push(outcome);
        }
    }

    // Sort strictly by match_idx before Elo reduction so parallel thread count never changes Elo.
    primary.sort_by_key(|o| o.match_idx);
    audits.sort_by_key(|o| o.match_idx);

    let mut standings: Vec<BotStanding> = entries
        .iter()
        .map(|e| BotStanding::new(e.name.clone()))
        .collect();

    let mut primary_ticks = 0u64;
    let mut primary_insns = 0u64;
    for outcome in &primary {
        primary_ticks += u64::from(outcome.summary.ticks_elapsed);
        primary_insns += outcome.summary.total_vm_instructions;
        let rank_keys: Vec<MatchRankKey> = outcome
            .summary
            .bots
            .iter()
            .map(|b| MatchRankKey {
                alive: b.alive,
                hp: b.hp,
                kills: b.kills,
                damage_dealt: b.damage_dealt,
                survival_ticks: b.survival_ticks,
            })
            .collect();
        let gas_avgs: Vec<f64> = outcome
            .summary
            .bots
            .iter()
            .map(|b| b.avg_gas_per_tick)
            .collect();
        let traps: Vec<u32> = outcome.summary.bots.iter().map(|b| b.traps).collect();
        let oogs: Vec<u32> = outcome
            .summary
            .bots
            .iter()
            .map(|b| b.out_of_gas_count)
            .collect();

        apply_match_to_standings(
            &mut standings,
            &outcome.participants,
            &rank_keys,
            outcome.summary.winner,
            &gas_avgs,
            &traps,
            &oogs,
        );
    }

    standings.sort_by(|a, b| {
        b.elo
            .total_cmp(&a.elo)
            .then_with(|| b.wins.cmp(&a.wins))
            .then_with(|| a.name.cmp(&b.name))
    });

    let audit = if config.audit_determinism {
        let mut matched = 0usize;
        let mut mismatch_detail = None;
        for (p, a) in primary.iter().zip(audits.iter()) {
            if p.summary.final_state_hash == a.summary.final_state_hash
                && p.summary.ticks_elapsed == a.summary.ticks_elapsed
            {
                matched += 1;
            } else if mismatch_detail.is_none() {
                mismatch_detail = Some(format!(
                    "match #{} diverged: 0x{:016x} vs 0x{:016x}",
                    p.match_idx, p.summary.final_state_hash, a.summary.final_state_hash
                ));
            }
        }
        Some(DeterminismAudit {
            matches_audited: primary.len(),
            hashes_matched: matched,
            passed: mismatch_detail.is_none() && matched == primary.len(),
            mismatch_detail,
        })
    } else {
        None
    };

    let total_executed_matches = telemetry.matches_done.load(Ordering::Relaxed) as f64;
    let total_executed_insns = telemetry.total_vm_instructions.load(Ordering::Relaxed) as f64;
    let matches_per_sec = total_executed_matches / elapsed_secs;
    let vm_mips = (total_executed_insns / 1_000_000.0) / elapsed_secs;

    TournamentReport {
        format_name: match config.format {
            TournamentFormat::RoundRobin1v1 => "Round-Robin 1v1".into(),
            TournamentFormat::Royale => "Battle Royale".into(),
        },
        threads_used: threads,
        matches_played: primary.len(),
        total_ticks: primary_ticks,
        total_vm_instructions: primary_insns,
        elapsed_secs,
        matches_per_sec,
        vm_mips,
        standings,
        audit,
    }
}
