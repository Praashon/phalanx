#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Phalanx Deterministic Arena Simulator (`phalanx-sim`).
//!
//! Combines bit-exact Q16.16 fixed-point physics and pure-integer trigonometry,
//! a zero-allocation uniform spatial hash grid, pillar line-of-sight raycasting,
//! simultaneous intent resolution, and self-contained `.phxr` binary replays
//! with full per-tick determinism verification.

pub mod arena;
pub mod fixed;
pub mod replay;
pub mod rng;
pub mod spatial;
pub mod world;

pub use arena::{ArenaConfig, Rect};
pub use fixed::Fix;
pub use replay::{
    Replay, ReplayError, VerifyReplayReport, decode_replay, encode_replay, record_match,
    simulate_replay_frames, verify_replay,
};
pub use rng::Rng;
pub use spatial::SpatialHashGrid;
pub use world::{
    BotMatchStats, BotState, CombatEvent, FrameSnapshot, MAX_ENERGY, MAX_HP, MatchSummary,
    Projectile, Simulation,
};
