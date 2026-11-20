#![forbid(unsafe_code)]

//! Elo rating system and tournament standings aggregator.

use serde::{Deserialize, Serialize};

/// Per-bot tournament leaderboard record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotStanding {
    /// Bot identifier.
    pub name: String,
    /// Current Elo rating (starts at `1500.0`).
    pub elo: f64,
    /// Matches played.
    pub matches: u32,
    /// First-place wins.
    pub wins: u32,
    /// Non-tie losses.
    pub losses: u32,
    /// Draws / multi-way ties.
    pub draws: u32,
    /// Cumulative kills scored.
    pub kills: u32,
    /// Cumulative damage dealt.
    pub damage_dealt: u64,
    /// Mean gas consumed per simulated tick.
    pub avg_gas_per_tick: f64,
    /// Mean survival duration in ticks.
    pub avg_survival_ticks: f64,
    /// Total runtime traps triggered.
    pub traps: u32,
    /// Total gas-limit cutoffs triggered.
    pub out_of_gas: u32,
}

impl BotStanding {
    /// Initializes a standing entry at `1500.0` Elo.
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name,
            elo: 1500.0,
            matches: 0,
            wins: 0,
            losses: 0,
            draws: 0,
            kills: 0,
            damage_dealt: 0,
            avg_gas_per_tick: 0.0,
            avg_survival_ticks: 0.0,
            traps: 0,
            out_of_gas: 0,
        }
    }
}

/// Pairwise rank key for ordering bots within a single match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchRankKey {
    /// Alive at match end.
    pub alive: bool,
    /// Remaining HP.
    pub hp: i32,
    /// Kills scored.
    pub kills: u32,
    /// Damage dealt.
    pub damage_dealt: u32,
    /// Ticks survived.
    pub survival_ticks: u32,
}

/// Updates Elo ratings and aggregate stats for a completed match deterministically.
pub fn apply_match_to_standings(
    standings: &mut [BotStanding],
    participants: &[usize],
    rank_keys: &[MatchRankKey],
    winner: Option<usize>,
    gas_avgs: &[f64],
    traps: &[u32],
    oogs: &[u32],
) {
    const K: f64 = 32.0;
    let n = participants.len();
    if n < 2 {
        return;
    }

    let snapshot_elos: Vec<f64> = participants.iter().map(|&idx| standings[idx].elo).collect();
    let scale = K / ((n - 1) as f64);

    for i in 0..n {
        let mut delta = 0.0;
        for j in 0..n {
            if i == j {
                continue;
            }
            let expected =
                1.0 / (1.0 + 10.0_f64.powf((snapshot_elos[j] - snapshot_elos[i]) / 400.0));
            let score = match rank_keys[i].cmp(&rank_keys[j]) {
                std::cmp::Ordering::Greater => 1.0,
                std::cmp::Ordering::Equal => 0.5,
                std::cmp::Ordering::Less => 0.0,
            };
            delta += scale * (score - expected);
        }

        let st = &mut standings[participants[i]];
        st.elo += delta;
        let prev_matches = f64::from(st.matches);
        st.matches += 1;
        let new_matches = f64::from(st.matches);
        st.kills += rank_keys[i].kills;
        st.damage_dealt += u64::from(rank_keys[i].damage_dealt);
        st.avg_gas_per_tick = (st.avg_gas_per_tick * prev_matches + gas_avgs[i]) / new_matches;
        st.avg_survival_ticks = (st.avg_survival_ticks * prev_matches
            + f64::from(rank_keys[i].survival_ticks))
            / new_matches;
        st.traps += traps[i];
        st.out_of_gas += oogs[i];

        match winner {
            Some(w) if w == i => st.wins += 1,
            Some(_) => st.losses += 1,
            None => st.draws += 1,
        }
    }
}
