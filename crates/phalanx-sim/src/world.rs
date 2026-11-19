//! Deterministic arena simulation loop, host syscall implementation, and snapshot capture.

use phalanx_vm::{ExecStatus, HostApi, SyscallId, TrapReason, VerifiedProgram, VmInstance};

use crate::arena::ArenaConfig;
use crate::fixed::Fix;
use crate::rng::Rng;
use crate::spatial::SpatialHashGrid;

/// Maximum bot HP.
pub const MAX_HP: i32 = 100;
/// Maximum bot energy.
pub const MAX_ENERGY: i32 = 100;

/// Simulation event emitted during a tick for combat logs and TUI visualizers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatEvent {
    /// A bot fired a projectile.
    Shot {
        /// Tick number.
        tick: u32,
        /// Shooter slot index.
        bot_id: usize,
        /// Power level `1..=3`.
        power: i32,
    },
    /// A projectile struck a bot.
    Hit {
        /// Tick number.
        tick: u32,
        /// Shooter slot index.
        attacker: usize,
        /// Victim slot index.
        victim: usize,
        /// Applied damage after shield mitigation.
        damage: i32,
        /// World impact X.
        x: Fix,
        /// World impact Y.
        y: Fix,
    },
    /// A bot was eliminated.
    Kill {
        /// Tick number.
        tick: u32,
        /// Killer slot index (`None` if storm).
        killer: Option<usize>,
        /// Eliminated slot index.
        victim: usize,
    },
    /// A bot triggered an evasive dash.
    Dash {
        /// Tick number.
        tick: u32,
        /// Bot slot index.
        bot_id: usize,
    },
    /// A bot took storm damage outside the safe zone.
    StormDamage {
        /// Tick number.
        tick: u32,
        /// Bot slot index.
        bot_id: usize,
        /// Damage taken.
        damage: i32,
    },
    /// A bot VM trapped during its tick.
    BotTrapped {
        /// Tick number.
        tick: u32,
        /// Bot slot index.
        bot_id: usize,
        /// Trap cause.
        reason: TrapReason,
    },
    /// A bot VM exhausted its per-tick gas budget.
    BotOutOfGas {
        /// Tick number.
        tick: u32,
        /// Bot slot index.
        bot_id: usize,
    },
    /// User `debug_log` emitted by a bot.
    DebugLog {
        /// Tick number.
        tick: u32,
        /// Bot slot index.
        bot_id: usize,
        /// Integer tag.
        tag: i32,
        /// Fixed-point value.
        val: Fix,
    },
}

/// Public physics and combat state of a single bot.
#[derive(Debug, Clone)]
pub struct BotState {
    /// Slot index in the match.
    pub id: usize,
    /// Display name.
    pub name: String,
    /// X position.
    pub x: Fix,
    /// Y position.
    pub y: Fix,
    /// X velocity.
    pub vx: Fix,
    /// Y velocity.
    pub vy: Fix,
    /// Current facing / aim heading in radians.
    pub heading: Fix,
    /// Hit points (`0..=MAX_HP`).
    pub hp: i32,
    /// Energy (`0..=MAX_ENERGY`).
    pub energy: i32,
    /// Whether bot is still alive.
    pub alive: bool,
    /// Whether defensive shield is currently active.
    pub shield_active: bool,
    /// Ticks remaining before weapon can fire again.
    pub fire_cooldown: u32,
    /// Ticks remaining before dash can be triggered again.
    pub dash_cooldown: u32,
    /// Total kills scored.
    pub kills: u32,
    /// Total damage dealt to enemies.
    pub damage_dealt: u32,
    /// Last tick this bot was alive.
    pub survival_ticks: u32,
    /// Cumulative gas used across all ticks.
    pub total_gas_used: u64,
    /// Cumulative VM instructions executed across all ticks.
    pub total_instructions: u64,
    /// Number of VM ticks executed.
    pub ticks_executed: u32,
    /// Last tick's gas consumption.
    pub last_gas_used: u32,
    /// Last tick's VM exit status.
    pub last_vm_status: ExecStatus,
    /// Total times VM trapped.
    pub traps: u32,
    /// Total times VM ran out of gas.
    pub out_of_gas_count: u32,
}

/// Active projectile in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projectile {
    /// Unique deterministic projectile ID.
    pub id: usize,
    /// Owner bot slot index.
    pub owner: usize,
    /// X position.
    pub x: Fix,
    /// Y position.
    pub y: Fix,
    /// X velocity.
    pub vx: Fix,
    /// Y velocity.
    pub vy: Fix,
    /// Base impact damage.
    pub damage: i32,
    /// Remaining flight ticks.
    pub ttl: u32,
    /// Active flag.
    pub alive: bool,
}

/// Lightweight immutable snapshot for TUI rendering and time-travel scrubbing.
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    /// Simulation tick.
    pub tick: u32,
    /// Safe-zone radius.
    pub zone_radius: Fix,
    /// Deterministic 64-bit world state hash.
    pub state_hash: u64,
    /// Bot snapshots.
    pub bots: Vec<BotState>,
    /// Active projectile positions `(owner, x, y, vx, vy)`.
    pub projectiles: Vec<(usize, Fix, Fix, Fix, Fix)>,
    /// Events emitted during this tick.
    pub events: Vec<CombatEvent>,
}

/// Per-bot final telemetry summary returned at the end of a match.
#[derive(Debug, Clone)]
pub struct BotMatchStats {
    /// Bot slot index.
    pub id: usize,
    /// Bot name.
    pub name: String,
    /// Remaining HP (`0` if dead).
    pub hp: i32,
    /// Whether bot survived to end.
    pub alive: bool,
    /// Kills scored.
    pub kills: u32,
    /// Damage dealt.
    pub damage_dealt: u32,
    /// Ticks survived.
    pub survival_ticks: u32,
    /// Average gas used per tick.
    pub avg_gas_per_tick: f64,
    /// Total VM instructions executed.
    pub total_instructions: u64,
    /// VM traps triggered.
    pub traps: u32,
    /// Gas-limit cutoffs.
    pub out_of_gas_count: u32,
}

/// Match result summary returned upon simulation completion.
#[derive(Debug, Clone)]
pub struct MatchSummary {
    /// Match seed.
    pub seed: u64,
    /// Winning bot slot index, or `None` on tie.
    pub winner: Option<usize>,
    /// Total ticks simulated.
    pub ticks_elapsed: u32,
    /// Final 64-bit world state hash.
    pub final_state_hash: u64,
    /// Total VM instructions executed across all bots.
    pub total_vm_instructions: u64,
    /// Per-bot statistics.
    pub bots: Vec<BotMatchStats>,
}

struct BotSlot {
    program: VerifiedProgram,
    vm: VmInstance,
    state: BotState,
    rng: Rng,
}

#[derive(Debug, Clone, Default)]
struct ActionIntent {
    thrust: Option<(Fix, Fix)>,
    fire: Option<(Fix, i32)>,
    dash: Option<Fix>,
    shield: bool,
    logs: Vec<(i32, Fix)>,
}

/// Deterministic fixed-point arena simulation engine.
pub struct Simulation {
    config: ArenaConfig,
    seed: u64,
    bots: Vec<BotSlot>,
    projectiles: Vec<Projectile>,
    bot_grid: SpatialHashGrid,
    bullet_grid: SpatialHashGrid,
    tick: u32,
    zone_radius: Fix,
    state_hash: u64,
    next_projectile_id: usize,
    tick_events: Vec<CombatEvent>,
    debug_intents: Vec<ActionIntent>,
}

impl Simulation {
    /// Creates a new deterministic simulation for `entries` `(name, VerifiedProgram)` and `seed`.
    #[must_use]
    pub fn new(entries: Vec<(String, VerifiedProgram)>, seed: u64, config: ArenaConfig) -> Self {
        let poses = config.spawn_poses(entries.len(), seed);
        let mut bots = Vec::with_capacity(entries.len());

        for (id, ((name, program), (x, y, heading))) in entries.into_iter().zip(poses).enumerate() {
            let vm = VmInstance::new(&program);
            let rng = Rng::derive(seed, (id as u64) + 1);
            let state = BotState {
                id,
                name,
                x,
                y,
                vx: Fix::ZERO,
                vy: Fix::ZERO,
                heading,
                hp: MAX_HP,
                energy: MAX_ENERGY,
                alive: true,
                shield_active: false,
                fire_cooldown: 0,
                dash_cooldown: 0,
                kills: 0,
                damage_dealt: 0,
                survival_ticks: 0,
                total_gas_used: 0,
                total_instructions: 0,
                ticks_executed: 0,
                last_gas_used: 0,
                last_vm_status: ExecStatus::Completed,
                traps: 0,
                out_of_gas_count: 0,
            };
            bots.push(BotSlot {
                program,
                vm,
                state,
                rng,
            });
        }

        let half_w = config.width / Fix::from_int(2);
        let half_h = config.height / Fix::from_int(2);
        let init_zone = Fix::hypot(half_w, half_h) + Fix::from_int(2);
        let bot_count = bots.len().max(1);

        let bot_grid =
            SpatialHashGrid::new(config.width, config.height, Fix::from_int(10), bot_count);
        let bullet_grid = SpatialHashGrid::new(config.width, config.height, Fix::from_int(10), 256);

        let mut sim = Self {
            config,
            seed,
            debug_intents: vec![ActionIntent::default(); bot_count],
            bots,
            projectiles: Vec::with_capacity(128),
            bot_grid,
            bullet_grid,
            tick: 0,
            zone_radius: init_zone,
            state_hash: 0,
            next_projectile_id: 0,
            tick_events: Vec::with_capacity(32),
        };
        sim.rebuild_grids();
        sim.state_hash = sim.compute_state_hash();
        sim
    }

    /// Returns arena configuration.
    #[must_use]
    pub fn config(&self) -> &ArenaConfig {
        &self.config
    }

    /// Current simulation tick index.
    #[must_use]
    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Current deterministic 64-bit world state hash.
    #[must_use]
    pub fn state_hash(&self) -> u64 {
        self.state_hash
    }

    /// Current safe-zone radius.
    #[must_use]
    pub fn zone_radius(&self) -> Fix {
        self.zone_radius
    }

    /// Number of bots in this match.
    #[must_use]
    pub fn bot_count(&self) -> usize {
        self.bots.len()
    }

    /// Verified program for bot `idx`.
    #[must_use]
    pub fn bot_program(&self, idx: usize) -> &VerifiedProgram {
        &self.bots[idx].program
    }

    /// Live VM instance for bot `idx` (for TUI debugger inspection).
    #[must_use]
    pub fn bot_vm(&self, idx: usize) -> &VmInstance {
        &self.bots[idx].vm
    }

    /// Returns `true` if match termination conditions have been reached.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        let alive = self.bots.iter().filter(|b| b.state.alive).count();
        let active_bullets = self.projectiles.iter().any(|p| p.alive);
        self.tick >= self.config.max_ticks
            || (alive <= 1 && !active_bullets && self.bots.len() > 1)
            || (alive == 0)
    }

    /// Advances the simulation by one deterministic tick; returns `false` if already finished.
    pub fn step_tick(&mut self) -> bool {
        if self.is_finished() {
            return false;
        }
        self.tick_events.clear();
        self.tick += 1;

        // 1. Update spatial grids on start-of-tick immutable world state.
        self.rebuild_grids();
        let snapshot_bots: Vec<BotState> = self.bots.iter().map(|b| b.state.clone()).collect();
        let alive_count = snapshot_bots.iter().filter(|b| b.alive).count() as i32;

        // 2. Execute each alive bot's sandboxed VM against the shared tick snapshot.
        let mut intents = Vec::with_capacity(self.bots.len());
        for idx in 0..self.bots.len() {
            if !snapshot_bots[idx].alive {
                intents.push(ActionIntent::default());
                continue;
            }

            let mut intent = ActionIntent::default();
            let bot_slot = &mut self.bots[idx];
            let mut host = BotHostView {
                self_idx: idx,
                tick: self.tick,
                alive_count,
                zone_radius: self.zone_radius,
                config: &self.config,
                bots: &snapshot_bots,
                projectiles: &self.projectiles,
                bot_grid: &self.bot_grid,
                bullet_grid: &self.bullet_grid,
                rng: &mut bot_slot.rng,
                intent: &mut intent,
                energy_budget: snapshot_bots[idx].energy,
                fire_cd: snapshot_bots[idx].fire_cooldown,
                dash_cd: snapshot_bots[idx].dash_cooldown,
            };

            let res = bot_slot
                .vm
                .run_tick(&bot_slot.program, &mut host, self.config.gas_budget);

            let slot = &mut bot_slot.state;
            slot.ticks_executed += 1;
            slot.last_gas_used = res.gas_used;
            slot.last_vm_status = res.status;
            slot.total_gas_used += u64::from(res.gas_used);
            slot.total_instructions += u64::from(res.instructions_executed);
            slot.survival_ticks = self.tick;

            match res.status {
                ExecStatus::OutOfGas => {
                    slot.out_of_gas_count += 1;
                    self.tick_events.push(CombatEvent::BotOutOfGas {
                        tick: self.tick,
                        bot_id: idx,
                    });
                }
                ExecStatus::Trapped(reason) => {
                    slot.traps += 1;
                    self.tick_events.push(CombatEvent::BotTrapped {
                        tick: self.tick,
                        bot_id: idx,
                        reason,
                    });
                }
                ExecStatus::Completed | ExecStatus::Running => {}
            }

            for &(tag, val) in &intent.logs {
                self.tick_events.push(CombatEvent::DebugLog {
                    tick: self.tick,
                    bot_id: idx,
                    tag,
                    val,
                });
            }
            intents.push(intent);
        }

        // 3. Apply all movement, shield, dash, and fire intents simultaneously.
        self.apply_intents(&intents);

        // 4. Advance physics, resolve bot/wall/pillar collisions, and step projectiles.
        self.step_physics();
        self.step_projectiles();
        self.apply_storm();

        self.state_hash = self.compute_state_hash();
        true
    }

    /// Single-steps the VM of `bot_idx` by one instruction for interactive debugging.
    pub fn step_bot_instruction(&mut self, bot_idx: usize) -> ExecStatus {
        if bot_idx >= self.bots.len() || !self.bots[bot_idx].state.alive {
            return ExecStatus::Completed;
        }
        if self.bots[bot_idx].vm.status() != ExecStatus::Running {
            let slot = &mut self.bots[bot_idx];
            slot.vm.begin_tick(&slot.program, self.config.gas_budget);
            self.debug_intents[bot_idx] = ActionIntent::default();
        }

        let snapshot_bots: Vec<BotState> = self.bots.iter().map(|b| b.state.clone()).collect();
        let alive_count = snapshot_bots.iter().filter(|b| b.alive).count() as i32;
        let slot = &mut self.bots[bot_idx];
        let mut host = BotHostView {
            self_idx: bot_idx,
            tick: self.tick,
            alive_count,
            zone_radius: self.zone_radius,
            config: &self.config,
            bots: &snapshot_bots,
            projectiles: &self.projectiles,
            bot_grid: &self.bot_grid,
            bullet_grid: &self.bullet_grid,
            rng: &mut slot.rng,
            intent: &mut self.debug_intents[bot_idx],
            energy_budget: snapshot_bots[bot_idx].energy,
            fire_cd: snapshot_bots[bot_idx].fire_cooldown,
            dash_cd: snapshot_bots[bot_idx].dash_cooldown,
        };
        let status = slot.vm.step(&slot.program, &mut host);
        slot.state.last_vm_status = status;
        slot.state.last_gas_used = slot.vm.gas_used();
        status
    }

    /// Captures a lightweight immutable frame snapshot for rendering or scrubbing.
    #[must_use]
    pub fn snapshot(&self) -> FrameSnapshot {
        FrameSnapshot {
            tick: self.tick,
            zone_radius: self.zone_radius,
            state_hash: self.state_hash,
            bots: self.bots.iter().map(|b| b.state.clone()).collect(),
            projectiles: self
                .projectiles
                .iter()
                .filter(|p| p.alive)
                .map(|p| (p.owner, p.x, p.y, p.vx, p.vy))
                .collect(),
            events: self.tick_events.clone(),
        }
    }

    /// Runs the match to completion and builds [`MatchSummary`].
    pub fn run_to_completion(&mut self) -> MatchSummary {
        while self.step_tick() {}
        self.summary()
    }

    /// Builds [`MatchSummary`] from the current simulation state.
    #[must_use]
    pub fn summary(&self) -> MatchSummary {
        let winner = self.determine_winner();
        let mut total_vm_instructions = 0u64;
        let bots = self
            .bots
            .iter()
            .map(|b| {
                let s = &b.state;
                total_vm_instructions += s.total_instructions;
                let avg_gas_per_tick = if s.ticks_executed == 0 {
                    0.0
                } else {
                    (s.total_gas_used as f64) / f64::from(s.ticks_executed)
                };
                BotMatchStats {
                    id: s.id,
                    name: s.name.clone(),
                    hp: s.hp,
                    alive: s.alive,
                    kills: s.kills,
                    damage_dealt: s.damage_dealt,
                    survival_ticks: s.survival_ticks,
                    avg_gas_per_tick,
                    total_instructions: s.total_instructions,
                    traps: s.traps,
                    out_of_gas_count: s.out_of_gas_count,
                }
            })
            .collect();

        MatchSummary {
            seed: self.seed,
            winner,
            ticks_elapsed: self.tick,
            final_state_hash: self.state_hash,
            total_vm_instructions,
            bots,
        }
    }

    fn determine_winner(&self) -> Option<usize> {
        let alive: Vec<&BotSlot> = self.bots.iter().filter(|b| b.state.alive).collect();
        if alive.len() == 1 {
            return Some(alive[0].state.id);
        }
        let candidates: Vec<&BotState> = if alive.is_empty() {
            self.bots.iter().map(|b| &b.state).collect()
        } else {
            alive.into_iter().map(|b| &b.state).collect()
        };
        let best = candidates
            .iter()
            .max_by_key(|s| (s.alive, s.hp, s.kills, s.damage_dealt, s.survival_ticks))?;
        let tied = candidates
            .iter()
            .filter(|s| {
                (s.alive, s.hp, s.kills, s.damage_dealt, s.survival_ticks)
                    == (
                        best.alive,
                        best.hp,
                        best.kills,
                        best.damage_dealt,
                        best.survival_ticks,
                    )
            })
            .count();
        if tied == 1 { Some(best.id) } else { None }
    }

    fn rebuild_grids(&mut self) {
        self.bot_grid.clear();
        for (i, b) in self.bots.iter().enumerate() {
            if b.state.alive {
                self.bot_grid.insert(i, b.state.x, b.state.y);
            }
        }
        self.bullet_grid.clear();
        self.bullet_grid.ensure_capacity(self.projectiles.len());
        for (i, p) in self.projectiles.iter().enumerate() {
            if p.alive {
                self.bullet_grid.insert(i, p.x, p.y);
            }
        }
    }

    fn apply_intents(&mut self, intents: &[ActionIntent]) {
        const THRUST_ACCEL: Fix = Fix(9_175); // ~0.14
        const DASH_IMPULSE: Fix = Fix(95_027); // ~1.45

        for (idx, intent) in intents.iter().enumerate() {
            let bot = &mut self.bots[idx].state;
            if !bot.alive {
                continue;
            }

            if bot.fire_cooldown > 0 {
                bot.fire_cooldown -= 1;
            }
            if bot.dash_cooldown > 0 {
                bot.dash_cooldown -= 1;
            }

            if let Some((angle, power)) = intent.thrust {
                let p = power.clamp(Fix::ZERO, Fix::ONE);
                let accel = THRUST_ACCEL * p;
                bot.vx += angle.cos() * accel;
                bot.vy += angle.sin() * accel;
                if p > Fix::ZERO {
                    bot.heading = angle.wrap_angle();
                }
            }

            if intent.shield && bot.energy >= 3 {
                bot.shield_active = true;
                bot.energy -= 3;
            } else {
                bot.shield_active = false;
                bot.energy = (bot.energy + 2).min(MAX_ENERGY);
            }

            if let Some(angle) = intent.dash
                && bot.dash_cooldown == 0
                && bot.energy >= 22
            {
                bot.energy -= 22;
                bot.dash_cooldown = 28;
                bot.vx += angle.cos() * DASH_IMPULSE;
                bot.vy += angle.sin() * DASH_IMPULSE;
                bot.heading = angle.wrap_angle();
                self.tick_events.push(CombatEvent::Dash {
                    tick: self.tick,
                    bot_id: idx,
                });
            }

            if let Some((angle, power)) = intent.fire {
                let lvl = power.clamp(1, 3);
                let (cost, cd, speed, dmg) = weapon_params(lvl);
                if bot.fire_cooldown == 0 && bot.energy >= cost {
                    bot.energy -= cost;
                    bot.fire_cooldown = cd;
                    bot.heading = angle.wrap_angle();
                    let cos_a = angle.cos();
                    let sin_a = angle.sin();
                    let spawn_dist = self.config.bot_radius + Fix::HALF;
                    let proj = Projectile {
                        id: self.next_projectile_id,
                        owner: idx,
                        x: bot.x + cos_a * spawn_dist,
                        y: bot.y + sin_a * spawn_dist,
                        vx: cos_a * speed,
                        vy: sin_a * speed,
                        damage: dmg,
                        ttl: 70,
                        alive: true,
                    };
                    self.next_projectile_id += 1;
                    self.projectiles.push(proj);
                    self.tick_events.push(CombatEvent::Shot {
                        tick: self.tick,
                        bot_id: idx,
                        power: lvl,
                    });
                }
            }
        }
    }

    fn step_physics(&mut self) {
        const DRAG: Fix = Fix(58_982); // 0.90
        const MAX_SPEED: Fix = Fix(62_259); // 0.95
        const MAX_DASH_SPEED: Fix = Fix(124_518); // 1.90

        let r = self.config.bot_radius;
        for b in &mut self.bots {
            let s = &mut b.state;
            if !s.alive {
                continue;
            }
            s.vx = s.vx * DRAG;
            s.vy = s.vy * DRAG;
            let cap = if s.dash_cooldown > 23 {
                MAX_DASH_SPEED
            } else {
                MAX_SPEED
            };
            let spd = Fix::hypot(s.vx, s.vy);
            if spd > cap {
                s.vx = (s.vx * cap) / spd;
                s.vy = (s.vy * cap) / spd;
            }

            s.x += s.vx;
            s.y += s.vy;

            // Arena walls
            if s.x < r {
                s.x = r;
                s.vx = -s.vx * Fix::HALF;
            } else if s.x > self.config.width - r {
                s.x = self.config.width - r;
                s.vx = -s.vx * Fix::HALF;
            }
            if s.y < r {
                s.y = r;
                s.vy = -s.vy * Fix::HALF;
            } else if s.y > self.config.height - r {
                s.y = self.config.height - r;
                s.vy = -s.vy * Fix::HALF;
            }

            // Cover pillars
            for pillar in &self.config.pillars {
                if let Some((nx, ny)) = pillar.push_out_circle(s.x, s.y, r) {
                    s.x = nx;
                    s.y = ny;
                    s.vx = s.vx * Fix::HALF;
                    s.vy = s.vy * Fix::HALF;
                }
            }
        }

        // Pairwise bot-vs-bot elastic separation
        let min_sep = r + r;
        let min_sep_sq = i64::from(min_sep.raw()) * i64::from(min_sep.raw());
        let n = self.bots.len();
        for i in 0..n {
            if !self.bots[i].state.alive {
                continue;
            }
            for j in (i + 1)..n {
                if !self.bots[j].state.alive {
                    continue;
                }
                let dx = self.bots[j].state.x - self.bots[i].state.x;
                let dy = self.bots[j].state.y - self.bots[i].state.y;
                let d2 = Fix::dist_sq_i64(dx, dy);
                if d2 < min_sep_sq && d2 > 0 {
                    let dist = Fix::hypot(dx, dy);
                    let overlap = (min_sep - dist) * Fix::HALF;
                    let px = (dx * overlap) / dist;
                    let py = (dy * overlap) / dist;
                    self.bots[i].state.x -= px;
                    self.bots[i].state.y -= py;
                    self.bots[j].state.x += px;
                    self.bots[j].state.y += py;
                }
            }
        }
    }

    fn step_projectiles(&mut self) {
        let hit_radius = self.config.bot_radius + Fix::from_ratio(2, 5); // +0.4
        let hit_sq = i64::from(hit_radius.raw()) * i64::from(hit_radius.raw());

        for p in &mut self.projectiles {
            if !p.alive {
                continue;
            }
            let prev_x = p.x;
            let prev_y = p.y;
            p.x += p.vx;
            p.y += p.vy;
            if p.ttl == 0
                || p.x < Fix::ZERO
                || p.x > self.config.width
                || p.y < Fix::ZERO
                || p.y > self.config.height
                || self
                    .config
                    .pillars
                    .iter()
                    .any(|rect| rect.intersects_segment(prev_x, prev_y, p.x, p.y))
            {
                p.alive = false;
                continue;
            }
            p.ttl -= 1;

            for victim_idx in 0..self.bots.len() {
                if victim_idx == p.owner || !self.bots[victim_idx].state.alive {
                    continue;
                }
                let vx = self.bots[victim_idx].state.x;
                let vy = self.bots[victim_idx].state.y;
                if Fix::dist_sq_i64(p.x - vx, p.y - vy) <= hit_sq {
                    p.alive = false;
                    let dmg = if self.bots[victim_idx].state.shield_active {
                        (p.damage * 2 + 2) / 5
                    } else {
                        p.damage
                    };
                    self.bots[victim_idx].state.hp = (self.bots[victim_idx].state.hp - dmg).max(0);
                    self.bots[p.owner].state.damage_dealt += dmg as u32;
                    self.tick_events.push(CombatEvent::Hit {
                        tick: self.tick,
                        attacker: p.owner,
                        victim: victim_idx,
                        damage: dmg,
                        x: p.x,
                        y: p.y,
                    });
                    if self.bots[victim_idx].state.hp == 0 {
                        self.bots[victim_idx].state.alive = false;
                        self.bots[victim_idx].state.survival_ticks = self.tick;
                        self.bots[p.owner].state.kills += 1;
                        self.tick_events.push(CombatEvent::Kill {
                            tick: self.tick,
                            killer: Some(p.owner),
                            victim: victim_idx,
                        });
                    }
                    break;
                }
            }
        }

        self.projectiles.retain(|p| p.alive);
    }

    fn apply_storm(&mut self) {
        if self.tick >= self.config.storm_start_tick
            && self.zone_radius > self.config.storm_min_radius
        {
            let shrink = Fix::from_ratio(1, 12); // ~0.0833/tick
            self.zone_radius = (self.zone_radius - shrink).max(self.config.storm_min_radius);
        }

        let (cx, cy) = self.config.center();
        let zr_sq = i64::from(self.zone_radius.raw()) * i64::from(self.zone_radius.raw());
        for idx in 0..self.bots.len() {
            let bot = &mut self.bots[idx].state;
            if !bot.alive {
                continue;
            }
            let d2 = Fix::dist_sq_i64(bot.x - cx, bot.y - cy);
            if d2 > zr_sq {
                let dmg = if bot.shield_active { 1 } else { 2 };
                bot.hp = (bot.hp - dmg).max(0);
                self.tick_events.push(CombatEvent::StormDamage {
                    tick: self.tick,
                    bot_id: idx,
                    damage: dmg,
                });
                if bot.hp == 0 {
                    bot.alive = false;
                    bot.survival_ticks = self.tick;
                    self.tick_events.push(CombatEvent::Kill {
                        tick: self.tick,
                        killer: None,
                        victim: idx,
                    });
                }
            }
        }
    }

    fn compute_state_hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        mix_u32(&mut h, self.tick);
        mix_i32(&mut h, self.zone_radius.raw());
        for b in &self.bots {
            let s = &b.state;
            mix_i32(&mut h, s.x.raw());
            mix_i32(&mut h, s.y.raw());
            mix_i32(&mut h, s.vx.raw());
            mix_i32(&mut h, s.vy.raw());
            mix_i32(&mut h, s.heading.raw());
            mix_i32(&mut h, s.hp);
            mix_i32(&mut h, s.energy);
            mix_u32(&mut h, u32::from(s.alive));
            mix_u32(&mut h, s.kills);
            mix_u32(&mut h, s.damage_dealt);
            let slot_count = b.program.program().state_layout.len();
            for &val in b.vm.state_slice(slot_count) {
                mix_i32(&mut h, val);
            }
        }
        mix_u32(&mut h, self.projectiles.len() as u32);
        for p in &self.projectiles {
            mix_u32(&mut h, p.owner as u32);
            mix_i32(&mut h, p.x.raw());
            mix_i32(&mut h, p.y.raw());
            mix_i32(&mut h, p.vx.raw());
            mix_i32(&mut h, p.vy.raw());
        }
        h
    }
}

fn weapon_params(power: i32) -> (i32, u32, Fix, i32) {
    match power {
        1 => (10, 7, Fix::from_ratio(23, 10), 11),
        3 => (30, 22, Fix::from_ratio(29, 20), 34),
        _ => (18, 13, Fix::from_ratio(37, 20), 20),
    }
}

fn mix_u32(h: &mut u64, v: u32) {
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

fn mix_i32(h: &mut u64, v: i32) {
    mix_u32(h, v as u32);
}

struct BotHostView<'a> {
    self_idx: usize,
    tick: u32,
    alive_count: i32,
    zone_radius: Fix,
    config: &'a ArenaConfig,
    bots: &'a [BotState],
    projectiles: &'a [Projectile],
    bot_grid: &'a SpatialHashGrid,
    bullet_grid: &'a SpatialHashGrid,
    rng: &'a mut Rng,
    intent: &'a mut ActionIntent,
    energy_budget: i32,
    fire_cd: u32,
    dash_cd: u32,
}

impl HostApi for BotHostView<'_> {
    fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32 {
        let me = &self.bots[self.self_idx];
        match id {
            SyscallId::SelfX => me.x.raw(),
            SyscallId::SelfY => me.y.raw(),
            SyscallId::SelfVx => me.vx.raw(),
            SyscallId::SelfVy => me.vy.raw(),
            SyscallId::SelfHeading => me.heading.raw(),
            SyscallId::SelfHp => me.hp,
            SyscallId::SelfEnergy => self.energy_budget,
            SyscallId::SelfId => self.self_idx as i32,
            SyscallId::TickNum => self.tick as i32,
            SyscallId::ArenaWidth => self.config.width.raw(),
            SyscallId::ArenaHeight => self.config.height.raw(),
            SyscallId::AliveCount => self.alive_count,
            SyscallId::ZoneRadius => self.zone_radius.raw(),
            SyscallId::ZoneDist => {
                let (cx, cy) = self.config.center();
                Fix::hypot(me.x - cx, me.y - cy).raw()
            }
            SyscallId::RadarNearest => self.find_nearest_enemy(),
            SyscallId::TargetDist => self
                .valid_enemy(args[0])
                .map_or(0, |t| Fix::hypot(t.x - me.x, t.y - me.y).raw()),
            SyscallId::TargetAngle => self
                .valid_enemy(args[0])
                .map_or(0, |t| Fix::atan2(t.y - me.y, t.x - me.x).raw()),
            SyscallId::TargetX => self.valid_enemy(args[0]).map_or(0, |t| t.x.raw()),
            SyscallId::TargetY => self.valid_enemy(args[0]).map_or(0, |t| t.y.raw()),
            SyscallId::TargetVx => self.valid_enemy(args[0]).map_or(0, |t| t.vx.raw()),
            SyscallId::TargetVy => self.valid_enemy(args[0]).map_or(0, |t| t.vy.raw()),
            SyscallId::TargetHp => self.valid_enemy(args[0]).map_or(0, |t| t.hp),
            SyscallId::ScanCone => {
                let angle = Fix::from_raw(args[0]);
                let half_w = Fix::from_raw(args[1]).abs();
                let max_range = Fix::from_raw(args[2]).max(Fix::ZERO);
                self.scan_cone(angle, half_w, max_range)
            }
            SyscallId::NearestBullet => self.find_nearest_bullet(),
            SyscallId::BulletDist => self
                .valid_bullet(args[0])
                .map_or(0, |b| Fix::hypot(b.x - me.x, b.y - me.y).raw()),
            SyscallId::BulletAngle => self
                .valid_bullet(args[0])
                .map_or(0, |b| Fix::atan2(b.y - me.y, b.x - me.x).raw()),
            SyscallId::BulletVx => self.valid_bullet(args[0]).map_or(0, |b| b.vx.raw()),
            SyscallId::BulletVy => self.valid_bullet(args[0]).map_or(0, |b| b.vy.raw()),
            SyscallId::WallDist => self
                .config
                .raycast_wall(me.x, me.y, Fix::from_raw(args[0]))
                .raw(),
            SyscallId::Thrust => {
                self.intent.thrust = Some((Fix::from_raw(args[0]), Fix::from_raw(args[1])));
                0
            }
            SyscallId::Fire => {
                let lvl = args[1].clamp(1, 3);
                let (cost, _, _, _) = weapon_params(lvl);
                if self.intent.fire.is_none() && self.fire_cd == 0 && self.energy_budget >= cost {
                    self.energy_budget -= cost;
                    self.intent.fire = Some((Fix::from_raw(args[0]), lvl));
                    1
                } else {
                    0
                }
            }
            SyscallId::Shield => {
                let want = args[0] != 0;
                if want && self.energy_budget >= 3 {
                    self.intent.shield = true;
                    1
                } else {
                    self.intent.shield = false;
                    0
                }
            }
            SyscallId::Dash => {
                if self.intent.dash.is_none() && self.dash_cd == 0 && self.energy_budget >= 22 {
                    self.energy_budget -= 22;
                    self.intent.dash = Some(Fix::from_raw(args[0]));
                    1
                } else {
                    0
                }
            }
            SyscallId::Sin => Fix::from_raw(args[0]).sin().raw(),
            SyscallId::Cos => Fix::from_raw(args[0]).cos().raw(),
            SyscallId::Atan2 => Fix::atan2(Fix::from_raw(args[0]), Fix::from_raw(args[1])).raw(),
            SyscallId::Sqrt => Fix::from_raw(args[0]).sqrt().raw(),
            SyscallId::AbsFix => Fix::from_raw(args[0]).abs().raw(),
            SyscallId::AbsInt => args[0].saturating_abs(),
            SyscallId::MinFix => args[0].min(args[1]),
            SyscallId::MaxFix => args[0].max(args[1]),
            SyscallId::ClampFix => Fix::from_raw(args[0])
                .clamp(Fix::from_raw(args[1]), Fix::from_raw(args[2]))
                .raw(),
            SyscallId::RandInt => self.rng.next_bounded_i32(args[0]),
            SyscallId::DebugLog => {
                if self.intent.logs.len() < 16 {
                    self.intent.logs.push((args[0], Fix::from_raw(args[1])));
                }
                0
            }
            SyscallId::AngleDiff => {
                Fix::angle_diff(Fix::from_raw(args[0]), Fix::from_raw(args[1])).raw()
            }
        }
    }
}

impl BotHostView<'_> {
    fn valid_enemy(&self, raw_id: i32) -> Option<&BotState> {
        let idx = usize::try_from(raw_id).ok()?;
        let b = self.bots.get(idx)?;
        if idx != self.self_idx && b.alive {
            Some(b)
        } else {
            None
        }
    }

    fn valid_bullet(&self, raw_id: i32) -> Option<&Projectile> {
        let idx = usize::try_from(raw_id).ok()?;
        let p = self.projectiles.get(idx)?;
        if p.alive && p.owner != self.self_idx {
            Some(p)
        } else {
            None
        }
    }

    fn find_nearest_enemy(&self) -> i32 {
        let me = &self.bots[self.self_idx];
        let mut best_id = -1i32;
        let mut best_d2 = i64::MAX;
        for (idx, b) in self.bots.iter().enumerate() {
            if idx == self.self_idx || !b.alive {
                continue;
            }
            let d2 = Fix::dist_sq_i64(b.x - me.x, b.y - me.y);
            if d2 < best_d2 {
                best_d2 = d2;
                best_id = idx as i32;
            }
        }
        best_id
    }

    fn scan_cone(&self, angle: Fix, half_w: Fix, max_range: Fix) -> i32 {
        let me = &self.bots[self.self_idx];
        let max_d2 = i64::from(max_range.raw()) * i64::from(max_range.raw());
        let mut best_id = -1i32;
        let mut best_d2 = i64::MAX;

        self.bot_grid.query_aabb(me.x, me.y, max_range, |idx| {
            if idx == self.self_idx {
                return;
            }
            let b = &self.bots[idx];
            if !b.alive {
                return;
            }
            let dx = b.x - me.x;
            let dy = b.y - me.y;
            let d2 = Fix::dist_sq_i64(dx, dy);
            if d2 <= max_d2 && d2 < best_d2 {
                let target_a = Fix::atan2(dy, dx);
                if Fix::angle_diff(target_a, angle).abs() <= half_w
                    && self.config.has_line_of_sight(me.x, me.y, b.x, b.y)
                {
                    best_d2 = d2;
                    best_id = idx as i32;
                }
            }
        });
        best_id
    }

    fn find_nearest_bullet(&self) -> i32 {
        let me = &self.bots[self.self_idx];
        let mut best_id = -1i32;
        let mut best_d2 = i64::MAX;

        self.bullet_grid
            .query_aabb(me.x, me.y, Fix::from_int(32), |idx| {
                let p = &self.projectiles[idx];
                if p.alive && p.owner != self.self_idx {
                    let d2 = Fix::dist_sq_i64(p.x - me.x, p.y - me.y);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_id = idx as i32;
                    }
                }
            });

        if best_id >= 0 {
            return best_id;
        }
        for (idx, p) in self.projectiles.iter().enumerate() {
            if p.alive && p.owner != self.self_idx {
                let d2 = Fix::dist_sq_i64(p.x - me.x, p.y - me.y);
                if d2 < best_d2 {
                    best_d2 = d2;
                    best_id = idx as i32;
                }
            }
        }
        best_id
    }
}
