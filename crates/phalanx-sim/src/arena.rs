//! Arena geometry, cover pillars, ray-box intersection, and spawn layout.

use crate::fixed::Fix;
use crate::rng::Rng;

/// Axis-aligned tactical cover pillar in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left X coordinate.
    pub x: Fix,
    /// Bottom Y coordinate.
    pub y: Fix,
    /// Width along X.
    pub w: Fix,
    /// Height along Y.
    pub h: Fix,
}

impl Rect {
    /// Creates an axis-aligned rectangle.
    #[must_use]
    pub const fn new(x: Fix, y: Fix, w: Fix, h: Fix) -> Self {
        Self { x, y, w, h }
    }

    /// Resolves circle-vs-AABB penetration by pushing `(cx, cy)` out along the minimum axis.
    #[must_use]
    pub fn push_out_circle(&self, cx: Fix, cy: Fix, radius: Fix) -> Option<(Fix, Fix)> {
        let closest_x = cx.clamp(self.x, self.x + self.w);
        let closest_y = cy.clamp(self.y, self.y + self.h);
        let dx = cx - closest_x;
        let dy = cy - closest_y;

        if dx == Fix::ZERO && dy == Fix::ZERO {
            // Center is inside the rectangle: eject along closest edge.
            let left = cx - self.x;
            let right = (self.x + self.w) - cx;
            let bottom = cy - self.y;
            let top = (self.y + self.h) - cy;
            let min_x = left.min(right);
            let min_y = bottom.min(top);
            if min_x < min_y {
                let nx = if left < right {
                    self.x - radius
                } else {
                    self.x + self.w + radius
                };
                return Some((nx, cy));
            }
            let ny = if bottom < top {
                self.y - radius
            } else {
                self.y + self.h + radius
            };
            return Some((cx, ny));
        }

        let dist_sq = Fix::dist_sq_i64(dx, dy);
        let r_sq = i64::from(radius.raw()) * i64::from(radius.raw());
        if dist_sq >= r_sq {
            return None;
        }
        let dist = Fix::hypot(dx, dy);
        if dist == Fix::ZERO {
            return None;
        }
        let push = radius - dist;
        let nx = cx + (dx * push) / dist;
        let ny = cy + (dy * push) / dist;
        Some((nx, ny))
    }

    /// Returns `true` if point `(px, py)` lies inside this rectangle (expanded by `margin`).
    #[must_use]
    pub fn contains_point(&self, px: Fix, py: Fix, margin: Fix) -> bool {
        px >= self.x - margin
            && px <= self.x + self.w + margin
            && py >= self.y - margin
            && py <= self.y + self.h + margin
    }

    /// Pure-integer slab intersection of segment `(x1, y1) -> (x2, y2)` with this rectangle.
    #[must_use]
    pub fn intersects_segment(&self, x1: Fix, y1: Fix, x2: Fix, y2: Fix) -> bool {
        const ONE_Q16: i64 = 65_536;
        let mut t_min = 0i64;
        let mut t_max = ONE_Q16;

        let dx = i64::from(x2.raw()) - i64::from(x1.raw());
        if dx == 0 {
            if x1 < self.x || x1 > self.x + self.w {
                return false;
            }
        } else {
            let t1 = ((i64::from(self.x.raw()) - i64::from(x1.raw())) << 16) / dx;
            let t2 = ((i64::from((self.x + self.w).raw()) - i64::from(x1.raw())) << 16) / dx;
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
            if t_min > t_max {
                return false;
            }
        }

        let dy = i64::from(y2.raw()) - i64::from(y1.raw());
        if dy == 0 {
            if y1 < self.y || y1 > self.y + self.h {
                return false;
            }
        } else {
            let t1 = ((i64::from(self.y.raw()) - i64::from(y1.raw())) << 16) / dy;
            let t2 = ((i64::from((self.y + self.h).raw()) - i64::from(y1.raw())) << 16) / dy;
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
            if t_min > t_max {
                return false;
            }
        }

        true
    }

    /// Ray-box intersection returning distance `t >= 0` along unit direction `(dx, dy)`.
    #[must_use]
    pub fn ray_intersect(&self, ox: Fix, oy: Fix, dx: Fix, dy: Fix) -> Option<Fix> {
        let mut t_min = 0i64;
        let mut t_max = i64::from(i32::MAX);

        let dx_i = i64::from(dx.raw());
        if dx_i == 0 {
            if ox < self.x || ox > self.x + self.w {
                return None;
            }
        } else {
            let t1 = ((i64::from(self.x.raw()) - i64::from(ox.raw())) << 16) / dx_i;
            let t2 = ((i64::from((self.x + self.w).raw()) - i64::from(ox.raw())) << 16) / dx_i;
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
            if t_min > t_max {
                return None;
            }
        }

        let dy_i = i64::from(dy.raw());
        if dy_i == 0 {
            if oy < self.y || oy > self.y + self.h {
                return None;
            }
        } else {
            let t1 = ((i64::from(self.y.raw()) - i64::from(oy.raw())) << 16) / dy_i;
            let t2 = ((i64::from((self.y + self.h).raw()) - i64::from(oy.raw())) << 16) / dy_i;
            t_min = t_min.max(t1.min(t2));
            t_max = t_max.min(t1.max(t2));
            if t_min > t_max {
                return None;
            }
        }

        Some(Fix::from_raw(t_min.clamp(0, i64::from(i32::MAX)) as i32))
    }
}

/// Complete arena configuration determining map dimensions, rules, and cover geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaConfig {
    /// Arena width in world units.
    pub width: Fix,
    /// Arena height in world units.
    pub height: Fix,
    /// Maximum match ticks before tie/HP decision.
    pub max_ticks: u32,
    /// Per-tick VM gas budget per bot.
    pub gas_budget: u32,
    /// Bot collision radius.
    pub bot_radius: Fix,
    /// Tick number when the storm ring begins closing.
    pub storm_start_tick: u32,
    /// Minimum safe-zone radius once fully closed.
    pub storm_min_radius: Fix,
    /// Symmetric cover pillar obstacles.
    pub pillars: Vec<Rect>,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self {
            width: Fix::from_int(100),
            height: Fix::from_int(60),
            max_ticks: 1200,
            gas_budget: 2500,
            bot_radius: Fix::from_ratio(8, 5), // 1.6
            storm_start_tick: 380,
            storm_min_radius: Fix::from_int(10),
            pillars: vec![
                Rect::new(
                    Fix::from_int(25),
                    Fix::from_int(16),
                    Fix::from_int(6),
                    Fix::from_int(6),
                ),
                Rect::new(
                    Fix::from_int(69),
                    Fix::from_int(16),
                    Fix::from_int(6),
                    Fix::from_int(6),
                ),
                Rect::new(
                    Fix::from_int(25),
                    Fix::from_int(38),
                    Fix::from_int(6),
                    Fix::from_int(6),
                ),
                Rect::new(
                    Fix::from_int(69),
                    Fix::from_int(38),
                    Fix::from_int(6),
                    Fix::from_int(6),
                ),
            ],
        }
    }
}

impl ArenaConfig {
    /// Center point of the arena `(width / 2, height / 2)`.
    #[must_use]
    pub fn center(&self) -> (Fix, Fix) {
        (
            self.width / Fix::from_int(2),
            self.height / Fix::from_int(2),
        )
    }

    /// Computes symmetric starting `(x, y, heading)` poses around the arena ellipse.
    #[must_use]
    pub fn spawn_poses(&self, count: usize, seed: u64) -> Vec<(Fix, Fix, Fix)> {
        if count == 0 {
            return Vec::new();
        }
        let (cx, cy) = self.center();
        let rx = (self.width / Fix::from_int(2)) - Fix::from_int(11);
        let ry = (self.height / Fix::from_int(2)) - Fix::from_int(8);
        let mut rng = Rng::derive(seed, 0xABCD);
        let base_phase = Fix::from_raw(rng.next_bounded_i32(Fix::TAU.raw()));

        let mut poses = Vec::with_capacity(count);
        for i in 0..count {
            let step =
                Fix::from_raw(((i64::from(Fix::TAU.raw()) * (i as i64)) / (count as i64)) as i32);
            let theta = (base_phase + step).wrap_angle();
            let mut x =
                (cx + rx * theta.cos()).clamp(Fix::from_int(6), self.width - Fix::from_int(6));
            let mut y =
                (cy + ry * theta.sin()).clamp(Fix::from_int(6), self.height - Fix::from_int(6));

            for pillar in &self.pillars {
                if let Some((nx, ny)) = pillar.push_out_circle(x, y, self.bot_radius + Fix::ONE) {
                    x = nx;
                    y = ny;
                }
            }
            let heading = Fix::atan2(cy - y, cx - x);
            poses.push((x, y, heading));
        }
        poses
    }

    /// Raycasts from `(ox, oy)` along `angle` against walls and all cover pillars.
    #[must_use]
    pub fn raycast_wall(&self, ox: Fix, oy: Fix, angle: Fix) -> Fix {
        let dx = angle.cos();
        let dy = angle.sin();
        let mut best = Fix::from_int(1000);

        if dx.raw() > 0 {
            let t = ((i64::from((self.width - ox).raw()) << 16) / i64::from(dx.raw()))
                .clamp(0, i64::from(i32::MAX)) as i32;
            best = best.min(Fix::from_raw(t));
        } else if dx.raw() < 0 {
            let t = ((i64::from((-ox).raw()) << 16) / i64::from(dx.raw()))
                .clamp(0, i64::from(i32::MAX)) as i32;
            best = best.min(Fix::from_raw(t));
        }

        if dy.raw() > 0 {
            let t = ((i64::from((self.height - oy).raw()) << 16) / i64::from(dy.raw()))
                .clamp(0, i64::from(i32::MAX)) as i32;
            best = best.min(Fix::from_raw(t));
        } else if dy.raw() < 0 {
            let t = ((i64::from((-oy).raw()) << 16) / i64::from(dy.raw()))
                .clamp(0, i64::from(i32::MAX)) as i32;
            best = best.min(Fix::from_raw(t));
        }

        for pillar in &self.pillars {
            if let Some(hit) = pillar.ray_intersect(ox, oy, dx, dy) {
                best = best.min(hit);
            }
        }
        best
    }

    /// Returns `true` if no cover pillar blocks the segment between `(x1, y1)` and `(x2, y2)`.
    #[must_use]
    pub fn has_line_of_sight(&self, x1: Fix, y1: Fix, x2: Fix, y2: Fix) -> bool {
        !self
            .pillars
            .iter()
            .any(|p| p.intersects_segment(x1, y1, x2, y2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_of_sight_detects_pillar_occlusion() {
        let cfg = ArenaConfig::default();
        // Pillar 0 is [25..31, 16..22]
        assert!(!cfg.has_line_of_sight(
            Fix::from_int(20),
            Fix::from_int(19),
            Fix::from_int(35),
            Fix::from_int(19)
        ));
        assert!(cfg.has_line_of_sight(
            Fix::from_int(20),
            Fix::from_int(30),
            Fix::from_int(80),
            Fix::from_int(30)
        ));
    }
}
