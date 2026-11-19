//! Bit-exact Q16.16 fixed-point arithmetic and pure-integer trigonometry.
//!
//! Zero IEEE-754 floating-point instructions are used in any simulation or math
//! calculation, ensuring identical results across x86_64, ARM64, Windows, Linux,
//! and macOS.

use core::fmt;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Signed Q16.16 fixed-point scalar (`1.0 == 65_536`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Fix(pub i32);

impl Fix {
    /// Fractional bit width.
    pub const FRAC_BITS: u32 = 16;
    /// `0.0`
    pub const ZERO: Self = Self(0);
    /// `0.5`
    pub const HALF: Self = Self(32_768);
    /// `1.0`
    pub const ONE: Self = Self(65_536);
    /// `pi` (`3.141586...`, closest Q16.16 representation `205_887`).
    pub const PI: Self = Self(205_887);
    /// `2 * pi` (`411_775`).
    pub const TAU: Self = Self(411_775);
    /// `pi / 2` (`102_944`).
    pub const HALF_PI: Self = Self(102_944);
    /// `pi / 4` (`51_472`).
    pub const QUARTER_PI: Self = Self(51_472);

    /// Wraps a raw Q16.16 `i32` representation.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// Returns the raw underlying Q16.16 `i32`.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// Converts a signed integer to Q16.16 with saturation.
    #[must_use]
    pub const fn from_int(v: i32) -> Self {
        let shifted = (v as i64) << 16;
        if shifted > i32::MAX as i64 {
            Self(i32::MAX)
        } else if shifted < i32::MIN as i64 {
            Self(i32::MIN)
        } else {
            Self(shifted as i32)
        }
    }

    /// Constructs `num / den` directly in Q16.16 without floats.
    #[must_use]
    pub const fn from_ratio(num: i32, den: i32) -> Self {
        if den == 0 {
            return Self::ZERO;
        }
        let q = ((num as i64) << 16) / (den as i64);
        if q > i32::MAX as i64 {
            Self(i32::MAX)
        } else if q < i32::MIN as i64 {
            Self(i32::MIN)
        } else {
            Self(q as i32)
        }
    }

    /// Floors to the nearest signed integer (`raw >> 16`).
    #[must_use]
    pub const fn to_int(self) -> i32 {
        self.0 >> 16
    }

    /// Converts to `f64` solely for UI rendering and test assertions.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        f64::from(self.0) / 65_536.0
    }

    /// Saturating absolute value.
    #[must_use]
    pub const fn abs(self) -> Self {
        Self(self.0.saturating_abs())
    }

    /// Returns the smaller of two fixed-point values.
    #[must_use]
    pub const fn min(self, other: Self) -> Self {
        if self.0 < other.0 { self } else { other }
    }

    /// Returns the larger of two fixed-point values.
    #[must_use]
    pub const fn max(self, other: Self) -> Self {
        if self.0 > other.0 { self } else { other }
    }

    /// Clamps `self` to `[lo, hi]`.
    #[must_use]
    pub const fn clamp(self, lo: Self, hi: Self) -> Self {
        self.max(lo).min(hi)
    }

    /// Checked fixed-point division returning `None` when dividing by zero.
    #[must_use]
    pub const fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.0 == 0 {
            return None;
        }
        let q = ((self.0 as i64) << 16) / (rhs.0 as i64);
        if q > i32::MAX as i64 {
            Some(Self(i32::MAX))
        } else if q < i32::MIN as i64 {
            Some(Self(i32::MIN))
        } else {
            Some(Self(q as i32))
        }
    }

    /// Exact integer Q16.16 square root (`0` for non-positive inputs).
    #[must_use]
    pub const fn sqrt(self) -> Self {
        if self.0 <= 0 {
            return Self::ZERO;
        }
        let n = (self.0 as u64) << 16;
        let root = isqrt_u64(n);
        Self(root as i32)
    }

    /// Squared Euclidean distance `(dx^2 + dy^2)` stored in widened `i64` Q32.32.
    #[must_use]
    pub const fn dist_sq_i64(dx: Self, dy: Self) -> i64 {
        let x = dx.0 as i64;
        let y = dy.0 as i64;
        x * x + y * y
    }

    /// Euclidean distance `sqrt(dx^2 + dy^2)` computed without overflow on arena scales.
    #[must_use]
    pub const fn hypot(dx: Self, dy: Self) -> Self {
        let x = (dx.0 as i64).unsigned_abs() as u128;
        let y = (dy.0 as i64).unsigned_abs() as u128;
        let sum = x * x + y * y;
        let root = isqrt_u128(sum);
        if root > i32::MAX as u128 {
            Self(i32::MAX)
        } else {
            Self(root as i32)
        }
    }

    /// Deterministic fixed-point sine via LUT + linear interpolation.
    #[must_use]
    pub fn sin(self) -> Self {
        sin_q16(self)
    }

    /// Deterministic fixed-point cosine via LUT + linear interpolation.
    #[must_use]
    pub fn cos(self) -> Self {
        sin_q16(self + Self::HALF_PI)
    }

    /// Deterministic fixed-point 2-argument arctangent returning angle in `[-PI, PI]`.
    #[must_use]
    pub fn atan2(y: Self, x: Self) -> Self {
        atan2_q16(y, x)
    }

    /// Wraps an angle into `[-PI, PI]`.
    #[must_use]
    pub const fn wrap_angle(self) -> Self {
        let tau = Self::TAU.0 as i64;
        let pi = Self::PI.0 as i64;
        let mut a = (self.0 as i64 + pi).rem_euclid(tau);
        if a < 0 {
            a += tau;
        }
        Self((a - pi) as i32)
    }

    /// Shortest signed angular difference `target - current` in `[-PI, PI]`.
    #[must_use]
    pub const fn angle_diff(target: Self, current: Self) -> Self {
        let diff = (target.0 as i64) - (current.0 as i64);
        let tau = Self::TAU.0 as i64;
        let pi = Self::PI.0 as i64;
        let wrapped = (diff + pi).rem_euclid(tau) - pi;
        Self(wrapped as i32)
    }
}

impl Add for Fix {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl AddAssign for Fix {
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_add(rhs.0);
    }
}

impl Sub for Fix {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl SubAssign for Fix {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.saturating_sub(rhs.0);
    }
}

impl Neg for Fix {
    type Output = Self;
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl Mul for Fix {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        let prod = (i64::from(self.0) * i64::from(rhs.0)) >> 16;
        Self(prod.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }
}

impl Div for Fix {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        self.checked_div(rhs).unwrap_or(Self::ZERO)
    }
}

impl fmt::Display for Fix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}", self.to_f64())
    }
}

const fn isqrt_u64(n: u64) -> u64 {
    let mut op = n;
    let mut res = 0u64;
    let mut one = 1u64 << 62;
    while one > op {
        one >>= 2;
    }
    while one != 0 {
        if op >= res + one {
            op -= res + one;
            res = (res >> 1) + one;
        } else {
            res >>= 1;
        }
        one >>= 2;
    }
    res
}

const fn isqrt_u128(n: u128) -> u128 {
    let mut op = n;
    let mut res = 0u128;
    let mut one = 1u128 << 126;
    while one > op {
        one >>= 2;
    }
    while one != 0 {
        if op >= res + one {
            op -= res + one;
            res = (res >> 1) + one;
        } else {
            res >>= 1;
        }
        one >>= 2;
    }
    res
}

// Compile-time Q16.16 first-quadrant sine LUT (257 entries for [0, PI/2]).
// Computed in Q32.32 pure integer arithmetic using 11-term Taylor series.
const SIN_LUT_LEN: usize = 256;
const SIN_LUT: [i32; SIN_LUT_LEN + 1] = build_sin_lut();

const fn build_sin_lut() -> [i32; SIN_LUT_LEN + 1] {
    // pi/2 in Q32.32 = 6746518852
    const HALF_PI_Q32: i128 = 6_746_518_852;
    const ONE_Q32: i128 = 1i128 << 32;

    let mut lut = [0i32; SIN_LUT_LEN + 1];
    let mut i = 0usize;
    while i <= SIN_LUT_LEN {
        let x = (HALF_PI_Q32 * (i as i128)) / (SIN_LUT_LEN as i128);
        let x2 = (x * x) >> 32;
        let mut term = x;
        let mut sum = x;
        let mut k = 1i128;
        while k <= 7 {
            let denom = (2 * k) * (2 * k + 1);
            term = -((term * x2) >> 32) / denom;
            sum += term;
            k += 1;
        }
        // Round from Q32 to Q16
        let q16 = (sum + (ONE_Q32 >> 17)) >> 16;
        lut[i] = if q16 > 65_536 {
            65_536
        } else if q16 < 0 {
            0
        } else {
            q16 as i32
        };
        i += 1;
    }
    lut
}

fn sin_q16(angle: Fix) -> Fix {
    let tau = i64::from(Fix::TAU.0);
    let a = i64::from(angle.0).rem_euclid(tau);
    let q_len = i64::from(Fix::HALF_PI.0); // ~pi/2
    let half_circle = i64::from(Fix::PI.0);
    let three_quarter = half_circle + q_len;

    let (sign, q_angle) = if a <= q_len {
        (1i32, a)
    } else if a <= half_circle {
        (1i32, half_circle - a)
    } else if a <= three_quarter {
        (-1i32, a - half_circle)
    } else {
        (-1i32, tau - a)
    };

    // Interpolate in SIN_LUT over [0, q_len]
    let scaled = (q_angle.clamp(0, q_len) * (SIN_LUT_LEN as i64) * 65_536) / q_len;
    let idx = ((scaled >> 16) as usize).min(SIN_LUT_LEN);
    let frac = scaled & 0xFFFF;
    let v0 = i64::from(SIN_LUT[idx]);
    let v1 = if idx < SIN_LUT_LEN {
        i64::from(SIN_LUT[idx + 1])
    } else {
        v0
    };
    let interp = v0 + (((v1 - v0) * frac) >> 16);
    Fix((interp as i32) * sign)
}

// Pure-integer atan(t) for 0 <= t <= ONE (Q16.16), using range reduction at tan(pi/8).
fn atan_unit_q16(t: i64) -> i64 {
    // tan(pi/8) ≈ 0.41421356 * 65536 = 27146
    const TAN_PI_8: i64 = 27_146;
    const ONE: i64 = 65_536;

    if t > TAN_PI_8 {
        // atan(t) = pi/4 - atan((1 - t) / (1 + t))
        let reduced = ((ONE - t) << 16) / (ONE + t);
        return i64::from(Fix::QUARTER_PI.0) - atan_small_q16(reduced);
    }
    atan_small_q16(t)
}

// 9th-order Taylor series in Q32 for 0 <= t <= 0.41421356.
fn atan_small_q16(t_q16: i64) -> i64 {
    let t = (t_q16 as i128) << 16; // Q32
    let t2 = (t * t) >> 32;
    let t3 = (t * t2) >> 32;
    let t5 = (t3 * t2) >> 32;
    let t7 = (t5 * t2) >> 32;
    let t9 = (t7 * t2) >> 32;
    let sum = t - (t3 / 3) + (t5 / 5) - (t7 / 7) + (t9 / 9);
    ((sum + 32_768) >> 16) as i64
}

fn atan2_q16(y: Fix, x: Fix) -> Fix {
    let xi = i64::from(x.0);
    let yi = i64::from(y.0);
    if xi == 0 && yi == 0 {
        return Fix::ZERO;
    }
    let ax = xi.abs();
    let ay = yi.abs();

    let base_angle = if ax >= ay {
        let t = (ay << 16) / ax;
        atan_unit_q16(t)
    } else {
        let t = (ax << 16) / ay;
        i64::from(Fix::HALF_PI.0) - atan_unit_q16(t)
    };

    let res = match (xi >= 0, yi >= 0) {
        (true, true) => base_angle,
        (false, true) => i64::from(Fix::PI.0) - base_angle,
        (false, false) => base_angle - i64::from(Fix::PI.0),
        (true, false) => -base_angle,
    };
    Fix(res as i32)
}

#[cfg(test)]
mod tests {
    use super::Fix;

    #[test]
    fn trig_and_sqrt_match_ieee754_within_tolerance() {
        for deg in -360..=360 {
            let rad = f64::from(deg).to_radians();
            let fx = Fix((rad * 65_536.0).round() as i32);
            let s_err = (fx.sin().to_f64() - rad.sin()).abs();
            let c_err = (fx.cos().to_f64() - rad.cos()).abs();
            assert!(s_err < 0.001, "sin({deg} deg) err={s_err}");
            assert!(c_err < 0.001, "cos({deg} deg) err={c_err}");
        }

        for deg in (-179..=180).step_by(3) {
            let rad = f64::from(deg).to_radians();
            let y = Fix((rad.sin() * 10.0 * 65_536.0).round() as i32);
            let x = Fix((rad.cos() * 10.0 * 65_536.0).round() as i32);
            let got = Fix::atan2(y, x).to_f64();
            let expected = (y.to_f64()).atan2(x.to_f64());
            let err = (got - expected).abs();
            assert!(err < 0.001, "atan2({deg} deg) err={err}");
        }

        for i in 0..500 {
            let v = Fix::from_ratio(i, 4);
            let got = v.sqrt().to_f64();
            let expected = v.to_f64().sqrt();
            assert!((got - expected).abs() < 0.001);
        }
    }
}
