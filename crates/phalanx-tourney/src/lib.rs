#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Phalanx Parallel Tournament Engine (`phalanx-tourney`).
//!
//! Executes autonomous bot tournaments across multiple worker threads using a
//! lock-free cache-line-padded work-stealing scheduler, computes deterministic
//! Elo standings, and optionally cross-audits state-hash determinism across cores.

pub mod elo;
pub mod pool;
pub mod scheduler;

pub use elo::BotStanding;
pub use scheduler::{
    BotEntry, DeterminismAudit, TournamentConfig, TournamentFormat, TournamentReport,
    run_tournament,
};
