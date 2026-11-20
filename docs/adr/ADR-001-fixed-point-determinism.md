# ADR-001: Deterministic Q16.16 Fixed-Point Arithmetic & Pure-Integer Math

## Status
Accepted

## Context
IEEE-754 `f32`/`f64` operations (`sin`, `cos`, `atan2`, `sqrt`, fused multiply-add contraction) vary across CPU architectures (x86_64 SSE/AVX vs ARM64 NEON), compiler codegen flags, and standard math library (`libm`) implementations. Any single-ULP divergence in bot trajectories or projectile collisions causes cascading desynchronization in multi-core tournament verification and portable `.phxr` match replays.

## Decision
1. **Zero floats in simulation physics and bot VM execution**: Both `phalanx-vm` (`Opcode::AddFix`, `SubFix`, `MulFix`, `DivFix`, `NegFix`) and `phalanx-sim` (`Fix(i32)`) execute exclusively on signed Q16.16 fixed-point integers (`1.0 == 65_536`) using widened `i64` intermediates (`(a * b) >> 16`, `(a << 16) / b`) with saturation.
2. **Pure-integer trigonometry**:
   - `Fix::sin` and `Fix::cos` interpolate a compile-time `const` 257-entry first-quadrant lookup table generated via pure `i128` Q32.32 Taylor series (`build_sin_lut`).
   - `Fix::atan2` uses octant reduction plus the half-angle identity $\arctan(t) = \frac{\pi}{4} - \arctan\!\left(\frac{1 - t}{1 + t}\right)$ for $t > \tan(\pi/8)$, followed by a 9th-order integer polynomial on $[0, 0.4142]$.
   - `Fix::sqrt` and `Fix::hypot` use exact digit-by-digit binary integer square roots (`isqrt_u64`, `isqrt_u128`).
3. **Deterministic decimal literal parsing**: `phalanx-lang`'s lexer converts decimal `.phx` literals like `3.14159` directly into Q16.16 `i32` bits using `u128` integer scaling and half-up rounding, never calling `f64::from_str`.
4. **UI-only float boundary**: `Fix::to_f64()` is restricted solely to presentation (`ratatui` Canvas coordinates and human-readable telemetry tables).

## Consequences
- Every match replay (`.phxr`) and parallel tournament audit produces identical 64-bit FNV-1a per-tick world hashes across Windows, Linux, and macOS regardless of CPU vector units or thread count.
- Fixed-point range is bounded to $[-32768.0, +32767.99998]$ with resolution $2^{-16} \approx 1.525 \times 10^{-5}$, which comfortably covers arena scales ($100 \times 60$ units) and keeps trigonometric error below $10^{-3}$ radians.
