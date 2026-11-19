//! Self-contained binary `.phxr` match replays with full per-tick determinism verification.

use core::fmt;

use phalanx_vm::crc32c::crc32c;
use phalanx_vm::{decode_program, encode_program, verify};

use crate::arena::{ArenaConfig, Rect};
use crate::fixed::Fix;
use crate::world::{FrameSnapshot, MatchSummary, Simulation};

/// 4-byte magic header identifying Phalanx match replay files.
pub const REPLAY_MAGIC: &[u8; 4] = b"PHXR";
/// Replay format version.
pub const REPLAY_VERSION: u16 = 1;

/// Self-contained match replay holding seed, arena geometry, bot bytecodes, and tick hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    /// Deterministic match seed.
    pub seed: u64,
    /// Arena rules and pillar layout.
    pub config: ArenaConfig,
    /// Embedded bots `(name, .phxb bytes)`.
    pub bots: Vec<(String, Vec<u8>)>,
    /// Recorded 64-bit world state hash after every simulated tick.
    pub tick_hashes: Vec<u64>,
    /// Recorded winner slot index, if any.
    pub winner: Option<u16>,
}

/// Summary report returned after verifying a [`Replay`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReplayReport {
    /// Match seed.
    pub seed: u64,
    /// Number of bots.
    pub bots: usize,
    /// Number of ticks re-simulated and verified.
    pub ticks_verified: usize,
    /// Final 64-bit deterministic state hash.
    pub final_state_hash: u64,
    /// Winning bot name if non-tie.
    pub winner_name: Option<String>,
}

/// Errors encountered while encoding, decoding, or verifying `.phxr` replays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    /// Input byte stream too short.
    Truncated,
    /// Invalid `PHXR` magic header.
    InvalidMagic,
    /// Unsupported replay container version.
    UnsupportedVersion(u16),
    /// Castagnoli CRC32C mismatch in replay container.
    ChecksumMismatch {
        /// Recorded CRC32C.
        expected: u32,
        /// Computed CRC32C.
        actual: u32,
    },
    /// Corrupt metadata string.
    InvalidUtf8,
    /// Section size exceeds bounds.
    LimitExceeded(&'static str),
    /// Embedded `.phxb` bytecode failed decoding or static verification.
    InvalidBotProgram(String),
    /// Re-simulated tick count disagreed with recorded replay.
    TickCountMismatch {
        /// Recorded tick count.
        expected: usize,
        /// Re-simulated tick count.
        actual: usize,
    },
    /// Deterministic state hash diverged at `tick`.
    DeterminismMismatch {
        /// Divergent tick index.
        tick: usize,
        /// Hash recorded in replay.
        expected: u64,
        /// Hash computed during verification.
        actual: u64,
    },
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("unexpected end of .phxr replay"),
            Self::InvalidMagic => f.write_str("invalid .phxr magic header"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported .phxr version {v}"),
            Self::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "replay CRC32C mismatch: expected 0x{expected:08x}, got 0x{actual:08x}"
                )
            }
            Self::InvalidUtf8 => f.write_str("invalid UTF-8 in replay metadata"),
            Self::LimitExceeded(w) => write!(f, "replay limit exceeded: {w}"),
            Self::InvalidBotProgram(e) => write!(f, "embedded bot failed verification: {e}"),
            Self::TickCountMismatch { expected, actual } => {
                write!(
                    f,
                    "tick count mismatch: expected {expected}, simulated {actual}"
                )
            }
            Self::DeterminismMismatch {
                tick,
                expected,
                actual,
            } => write!(
                f,
                "determinism hash diverged at tick {tick}: expected 0x{expected:016x}, got 0x{actual:016x}"
            ),
        }
    }
}

impl std::error::Error for ReplayError {}

/// Executes a full match while recording tick hashes and UI snapshots into a self-contained [`Replay`].
pub fn record_match(
    entries: Vec<(String, phalanx_vm::VerifiedProgram)>,
    seed: u64,
    config: ArenaConfig,
) -> Result<(MatchSummary, Replay, Vec<FrameSnapshot>), ReplayError> {
    let mut encoded_bots = Vec::with_capacity(entries.len());
    for (name, vp) in &entries {
        let bytes = encode_program(vp.program())
            .map_err(|e| ReplayError::InvalidBotProgram(e.to_string()))?;
        encoded_bots.push((name.clone(), bytes));
    }

    let mut sim = Simulation::new(entries, seed, config.clone());
    let mut tick_hashes = Vec::with_capacity(config.max_ticks as usize);
    let mut frames = Vec::with_capacity(config.max_ticks as usize);
    frames.push(sim.snapshot());

    while sim.step_tick() {
        tick_hashes.push(sim.state_hash());
        frames.push(sim.snapshot());
    }

    let summary = sim.summary();
    let replay = Replay {
        seed,
        config,
        bots: encoded_bots,
        tick_hashes,
        winner: summary.winner.map(|w| w as u16),
    };
    Ok((summary, replay, frames))
}

/// Reconstructs the arena from `replay`, re-simulates every tick, and verifies every 64-bit hash.
pub fn verify_replay(replay: &Replay) -> Result<VerifyReplayReport, ReplayError> {
    let mut entries = Vec::with_capacity(replay.bots.len());
    for (name, phxb) in &replay.bots {
        let prog =
            decode_program(phxb).map_err(|e| ReplayError::InvalidBotProgram(e.to_string()))?;
        let (vp, _) = verify(prog).map_err(|e| ReplayError::InvalidBotProgram(e.to_string()))?;
        entries.push((name.clone(), vp));
    }

    let mut sim = Simulation::new(entries, replay.seed, replay.config.clone());
    let mut actual_ticks = 0usize;
    while sim.step_tick() {
        let idx = actual_ticks;
        actual_ticks += 1;
        let expected =
            replay
                .tick_hashes
                .get(idx)
                .copied()
                .ok_or(ReplayError::TickCountMismatch {
                    expected: replay.tick_hashes.len(),
                    actual: actual_ticks,
                })?;
        let actual = sim.state_hash();
        if actual != expected {
            return Err(ReplayError::DeterminismMismatch {
                tick: idx + 1,
                expected,
                actual,
            });
        }
    }

    if actual_ticks != replay.tick_hashes.len() {
        return Err(ReplayError::TickCountMismatch {
            expected: replay.tick_hashes.len(),
            actual: actual_ticks,
        });
    }

    let summary = sim.summary();
    let winner_name = summary
        .winner
        .and_then(|w| summary.bots.get(w))
        .map(|b| b.name.clone());

    Ok(VerifyReplayReport {
        seed: replay.seed,
        bots: replay.bots.len(),
        ticks_verified: actual_ticks,
        final_state_hash: summary.final_state_hash,
        winner_name,
    })
}

/// Replays `replay` and collects all [`FrameSnapshot`]s for interactive TUI scrubbing.
pub fn simulate_replay_frames(
    replay: &Replay,
) -> Result<(MatchSummary, Vec<FrameSnapshot>), ReplayError> {
    let mut entries = Vec::with_capacity(replay.bots.len());
    for (name, phxb) in &replay.bots {
        let prog =
            decode_program(phxb).map_err(|e| ReplayError::InvalidBotProgram(e.to_string()))?;
        let (vp, _) = verify(prog).map_err(|e| ReplayError::InvalidBotProgram(e.to_string()))?;
        entries.push((name.clone(), vp));
    }
    let mut sim = Simulation::new(entries, replay.seed, replay.config.clone());
    let mut frames = Vec::with_capacity(replay.tick_hashes.len() + 1);
    frames.push(sim.snapshot());
    while sim.step_tick() {
        frames.push(sim.snapshot());
    }
    Ok((sim.summary(), frames))
}

/// Encodes a [`Replay`] into a CRC32C-checked `.phxr` binary stream.
#[must_use]
pub fn encode_replay(replay: &Replay) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    out.extend_from_slice(REPLAY_MAGIC);
    out.extend_from_slice(&REPLAY_VERSION.to_le_bytes());
    out.extend_from_slice(&replay.seed.to_le_bytes());

    out.extend_from_slice(&replay.config.width.raw().to_le_bytes());
    out.extend_from_slice(&replay.config.height.raw().to_le_bytes());
    out.extend_from_slice(&replay.config.max_ticks.to_le_bytes());
    out.extend_from_slice(&replay.config.gas_budget.to_le_bytes());
    out.extend_from_slice(&replay.config.bot_radius.raw().to_le_bytes());
    out.extend_from_slice(&replay.config.storm_start_tick.to_le_bytes());
    out.extend_from_slice(&replay.config.storm_min_radius.raw().to_le_bytes());

    out.extend_from_slice(&(replay.config.pillars.len() as u16).to_le_bytes());
    for r in &replay.config.pillars {
        out.extend_from_slice(&r.x.raw().to_le_bytes());
        out.extend_from_slice(&r.y.raw().to_le_bytes());
        out.extend_from_slice(&r.w.raw().to_le_bytes());
        out.extend_from_slice(&r.h.raw().to_le_bytes());
    }

    out.extend_from_slice(&(replay.bots.len() as u16).to_le_bytes());
    for (name, phxb) in &replay.bots {
        let nb = name.as_bytes();
        out.extend_from_slice(&(nb.len().min(256) as u16).to_le_bytes());
        out.extend_from_slice(&nb[..nb.len().min(256)]);
        out.extend_from_slice(&(phxb.len() as u32).to_le_bytes());
        out.extend_from_slice(phxb);
    }

    match replay.winner {
        None => out.push(0),
        Some(w) => {
            out.push(1);
            out.extend_from_slice(&w.to_le_bytes());
        }
    }

    out.extend_from_slice(&(replay.tick_hashes.len() as u32).to_le_bytes());
    for &h in &replay.tick_hashes {
        out.extend_from_slice(&h.to_le_bytes());
    }

    let crc = crc32c(&out);
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

/// Decodes and CRC32C-verifies a `.phxr` binary replay.
pub fn decode_replay(bytes: &[u8]) -> Result<Replay, ReplayError> {
    if bytes.len() < 10 {
        return Err(ReplayError::Truncated);
    }
    if &bytes[0..4] != REPLAY_MAGIC {
        return Err(ReplayError::InvalidMagic);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != REPLAY_VERSION {
        return Err(ReplayError::UnsupportedVersion(version));
    }
    let split = bytes.len() - 4;
    let expected = u32::from_le_bytes([
        bytes[split],
        bytes[split + 1],
        bytes[split + 2],
        bytes[split + 3],
    ]);
    let actual = crc32c(&bytes[..split]);
    if expected != actual {
        return Err(ReplayError::ChecksumMismatch { expected, actual });
    }

    let mut cur = Reader::new(&bytes[6..split]);
    let seed = cur.read_u64()?;
    let width = Fix::from_raw(cur.read_i32()?);
    let height = Fix::from_raw(cur.read_i32()?);
    let max_ticks = cur.read_u32()?;
    let gas_budget = cur.read_u32()?;
    let bot_radius = Fix::from_raw(cur.read_i32()?);
    let storm_start_tick = cur.read_u32()?;
    let storm_min_radius = Fix::from_raw(cur.read_i32()?);

    let pillar_count = usize::from(cur.read_u16()?);
    if pillar_count > 64 {
        return Err(ReplayError::LimitExceeded("pillars"));
    }
    let mut pillars = Vec::with_capacity(pillar_count);
    for _ in 0..pillar_count {
        pillars.push(Rect::new(
            Fix::from_raw(cur.read_i32()?),
            Fix::from_raw(cur.read_i32()?),
            Fix::from_raw(cur.read_i32()?),
            Fix::from_raw(cur.read_i32()?),
        ));
    }

    let bot_count = usize::from(cur.read_u16()?);
    if bot_count > 32 {
        return Err(ReplayError::LimitExceeded("bots"));
    }
    let mut bots = Vec::with_capacity(bot_count);
    for _ in 0..bot_count {
        let name_len = usize::from(cur.read_u16()?);
        if name_len > 256 {
            return Err(ReplayError::LimitExceeded("bot_name"));
        }
        let name_raw = cur.take(name_len)?;
        let name = std::str::from_utf8(name_raw)
            .map(str::to_owned)
            .map_err(|_| ReplayError::InvalidUtf8)?;
        let phxb_len = cur.read_u32()? as usize;
        if phxb_len > 512 * 1024 {
            return Err(ReplayError::LimitExceeded("phxb_size"));
        }
        let phxb = cur.take(phxb_len)?.to_vec();
        bots.push((name, phxb));
    }

    let has_winner = cur.take(1)?[0];
    let winner = if has_winner != 0 {
        Some(cur.read_u16()?)
    } else {
        None
    };

    let hash_count = cur.read_u32()? as usize;
    if hash_count > 100_000 {
        return Err(ReplayError::LimitExceeded("tick_hashes"));
    }
    let mut tick_hashes = Vec::with_capacity(hash_count);
    for _ in 0..hash_count {
        tick_hashes.push(cur.read_u64()?);
    }

    Ok(Replay {
        seed,
        config: ArenaConfig {
            width,
            height,
            max_ticks,
            gas_budget,
            bot_radius,
            storm_start_tick,
            storm_min_radius,
            pillars,
        },
        bots,
        tick_hashes,
        winner,
    })
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        let end = self.pos.checked_add(n).ok_or(ReplayError::Truncated)?;
        if end > self.buf.len() {
            return Err(ReplayError::Truncated);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn read_u16(&mut self) -> Result<u16, ReplayError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32, ReplayError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(&mut self) -> Result<i32, ReplayError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64, ReplayError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}
