//! Host syscall definitions shared across the compiler, verifier, VM, and simulator.

use crate::isa::ValType;

/// All host-provided Arena and Math syscalls available to `.phx` bots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SyscallId {
    /// Self X position (`fix`).
    SelfX = 0,
    /// Self Y position (`fix`).
    SelfY = 1,
    /// Self X velocity (`fix`).
    SelfVx = 2,
    /// Self Y velocity (`fix`).
    SelfVy = 3,
    /// Self heading angle in radians (`fix`).
    SelfHeading = 4,
    /// Self current hit points (`i32`).
    SelfHp = 5,
    /// Self current energy (`i32`).
    SelfEnergy = 6,
    /// Self arena slot ID (`i32`).
    SelfId = 7,
    /// Current simulation tick number (`i32`).
    TickNum = 8,
    /// Arena width in world units (`fix`).
    ArenaWidth = 9,
    /// Arena height in world units (`fix`).
    ArenaHeight = 10,
    /// Number of currently alive bots (`i32`).
    AliveCount = 11,
    /// Current safe-zone radius (`fix`).
    ZoneRadius = 12,
    /// Self distance from the safe-zone center (`fix`).
    ZoneDist = 13,
    /// Bot ID of nearest alive enemy, or `-1` (`i32`).
    RadarNearest = 14,
    /// Distance to bot `id` (`fix`).
    TargetDist = 15,
    /// Angle from self to bot `id` in `[-PI, PI]` (`fix`).
    TargetAngle = 16,
    /// Target bot X coordinate (`fix`).
    TargetX = 17,
    /// Target bot Y coordinate (`fix`).
    TargetY = 18,
    /// Target bot X velocity (`fix`).
    TargetVx = 19,
    /// Target bot Y velocity (`fix`).
    TargetVy = 20,
    /// Target bot HP (`i32`).
    TargetHp = 21,
    /// Nearest alive enemy visible within cone & unobstructed by pillars, or `-1` (`i32`).
    ScanCone = 22,
    /// ID of nearest hostile projectile, or `-1` (`i32`).
    NearestBullet = 23,
    /// Distance from self to projectile `id` (`fix`).
    BulletDist = 24,
    /// Angle from self to projectile `id` (`fix`).
    BulletAngle = 25,
    /// Projectile `id` X velocity (`fix`).
    BulletVx = 26,
    /// Projectile `id` Y velocity (`fix`).
    BulletVy = 27,
    /// Raycast distance from self along `angle` to arena wall or cover pillar (`fix`).
    WallDist = 28,
    /// Apply engine thrust in `angle` with normalized `power` (`[0, 1]`).
    Thrust = 29,
    /// Fire projectile along `angle` with power level `1..=3`; returns `true` if queued.
    Fire = 30,
    /// Toggle defensive shield (`true` = active); returns whether shield is active.
    Shield = 31,
    /// Trigger directional dash impulse along `angle`; returns `true` if triggered.
    Dash = 32,
    /// Fixed-point sine (`fix`).
    Sin = 33,
    /// Fixed-point cosine (`fix`).
    Cos = 34,
    /// Fixed-point 2-argument arctangent (`fix`).
    Atan2 = 35,
    /// Fixed-point square root (`fix`).
    Sqrt = 36,
    /// Fixed-point absolute value (`fix`).
    AbsFix = 37,
    /// Integer absolute value (`i32`).
    AbsInt = 38,
    /// Minimum of two fixed-point values (`fix`).
    MinFix = 39,
    /// Maximum of two fixed-point values (`fix`).
    MaxFix = 40,
    /// Clamp `x` to `[lo, hi]` (`fix`).
    ClampFix = 41,
    /// Deterministic per-bot PRNG integer in `[0, max_excl)`.
    RandInt = 42,
    /// Emit a debug log entry `(tag, val)` visible in the debugger.
    DebugLog = 43,
    /// Shortest signed angle difference `target - current` wrapped to `[-PI, PI]` (`fix`).
    AngleDiff = 44,
}

/// Static signature and gas cost metadata for a host syscall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyscallSpec {
    /// Numeric syscall identifier.
    pub id: SyscallId,
    /// Function name exposed in `.phx` source code.
    pub name: &'static str,
    /// Parameter type signature.
    pub params: &'static [ValType],
    /// Return type, or `None` for void syscalls.
    pub ret: Option<ValType>,
    /// Gas cost charged per invocation.
    pub gas: u32,
}

const F: ValType = ValType::Fix;
const I: ValType = ValType::Int;
const B: ValType = ValType::Bool;

/// Authoritative table of all 45 syscalls indexed by `SyscallId as usize`.
pub static SYSCALLS: [SyscallSpec; 45] = [
    SyscallSpec {
        id: SyscallId::SelfX,
        name: "self_x",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfY,
        name: "self_y",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfVx,
        name: "self_vx",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfVy,
        name: "self_vy",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfHeading,
        name: "self_heading",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfHp,
        name: "self_hp",
        params: &[],
        ret: Some(I),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfEnergy,
        name: "self_energy",
        params: &[],
        ret: Some(I),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::SelfId,
        name: "self_id",
        params: &[],
        ret: Some(I),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::TickNum,
        name: "tick_num",
        params: &[],
        ret: Some(I),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::ArenaWidth,
        name: "arena_width",
        params: &[],
        ret: Some(F),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::ArenaHeight,
        name: "arena_height",
        params: &[],
        ret: Some(F),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::AliveCount,
        name: "alive_count",
        params: &[],
        ret: Some(I),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::ZoneRadius,
        name: "zone_radius",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::ZoneDist,
        name: "zone_dist",
        params: &[],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::RadarNearest,
        name: "radar_nearest",
        params: &[],
        ret: Some(I),
        gas: 6,
    },
    SyscallSpec {
        id: SyscallId::TargetDist,
        name: "target_dist",
        params: &[I],
        ret: Some(F),
        gas: 3,
    },
    SyscallSpec {
        id: SyscallId::TargetAngle,
        name: "target_angle",
        params: &[I],
        ret: Some(F),
        gas: 4,
    },
    SyscallSpec {
        id: SyscallId::TargetX,
        name: "target_x",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::TargetY,
        name: "target_y",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::TargetVx,
        name: "target_vx",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::TargetVy,
        name: "target_vy",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::TargetHp,
        name: "target_hp",
        params: &[I],
        ret: Some(I),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::ScanCone,
        name: "scan_cone",
        params: &[F, F, F],
        ret: Some(I),
        gas: 8,
    },
    SyscallSpec {
        id: SyscallId::NearestBullet,
        name: "nearest_bullet",
        params: &[],
        ret: Some(I),
        gas: 5,
    },
    SyscallSpec {
        id: SyscallId::BulletDist,
        name: "bullet_dist",
        params: &[I],
        ret: Some(F),
        gas: 3,
    },
    SyscallSpec {
        id: SyscallId::BulletAngle,
        name: "bullet_angle",
        params: &[I],
        ret: Some(F),
        gas: 4,
    },
    SyscallSpec {
        id: SyscallId::BulletVx,
        name: "bullet_vx",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::BulletVy,
        name: "bullet_vy",
        params: &[I],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::WallDist,
        name: "wall_dist",
        params: &[F],
        ret: Some(F),
        gas: 6,
    },
    SyscallSpec {
        id: SyscallId::Thrust,
        name: "thrust",
        params: &[F, F],
        ret: None,
        gas: 4,
    },
    SyscallSpec {
        id: SyscallId::Fire,
        name: "fire",
        params: &[F, I],
        ret: Some(B),
        gas: 5,
    },
    SyscallSpec {
        id: SyscallId::Shield,
        name: "shield",
        params: &[B],
        ret: Some(B),
        gas: 3,
    },
    SyscallSpec {
        id: SyscallId::Dash,
        name: "dash",
        params: &[F],
        ret: Some(B),
        gas: 5,
    },
    SyscallSpec {
        id: SyscallId::Sin,
        name: "sin",
        params: &[F],
        ret: Some(F),
        gas: 3,
    },
    SyscallSpec {
        id: SyscallId::Cos,
        name: "cos",
        params: &[F],
        ret: Some(F),
        gas: 3,
    },
    SyscallSpec {
        id: SyscallId::Atan2,
        name: "atan2",
        params: &[F, F],
        ret: Some(F),
        gas: 4,
    },
    SyscallSpec {
        id: SyscallId::Sqrt,
        name: "sqrt",
        params: &[F],
        ret: Some(F),
        gas: 4,
    },
    SyscallSpec {
        id: SyscallId::AbsFix,
        name: "abs_fix",
        params: &[F],
        ret: Some(F),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::AbsInt,
        name: "abs_int",
        params: &[I],
        ret: Some(I),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::MinFix,
        name: "min_fix",
        params: &[F, F],
        ret: Some(F),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::MaxFix,
        name: "max_fix",
        params: &[F, F],
        ret: Some(F),
        gas: 1,
    },
    SyscallSpec {
        id: SyscallId::ClampFix,
        name: "clamp_fix",
        params: &[F, F, F],
        ret: Some(F),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::RandInt,
        name: "rand_int",
        params: &[I],
        ret: Some(I),
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::DebugLog,
        name: "debug_log",
        params: &[I, F],
        ret: None,
        gas: 2,
    },
    SyscallSpec {
        id: SyscallId::AngleDiff,
        name: "angle_diff",
        params: &[F, F],
        ret: Some(F),
        gas: 2,
    },
];

impl SyscallSpec {
    /// Looks up a syscall specification by numeric ID in O(1) time.
    #[must_use]
    pub fn from_id(id: u8) -> Option<&'static Self> {
        SYSCALLS.get(usize::from(id))
    }

    /// Looks up a syscall specification by its `.phx` source name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<&'static Self> {
        SYSCALLS.iter().find(|s| s.name == name)
    }
}

/// Host interface implemented by the Arena simulator (or test mocks) to service bot syscalls.
pub trait HostApi {
    /// Executes syscall `id` with `args` matching `SyscallSpec::params` and returns the raw result bits.
    fn invoke(&mut self, id: SyscallId, args: &[i32]) -> i32;
}

#[cfg(test)]
mod tests {
    use super::SYSCALLS;

    #[test]
    fn syscall_indices_match_enum_discriminants() {
        for (idx, spec) in SYSCALLS.iter().enumerate() {
            assert_eq!(idx, spec.id as usize);
        }
    }
}
