//! Deterministic Xoshiro256** PRNG with independent per-stream seed derivation.

/// Deterministic 256-bit pseudorandom number generator (`Xoshiro256**`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng {
    state: [u64; 4],
}

impl Rng {
    /// Initializes an independent stream from `(seed, stream_id)` via SplitMix64.
    #[must_use]
    pub fn derive(seed: u64, stream_id: u64) -> Self {
        let mut sm = seed ^ stream_id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut state = [0u64; 4];
        for slot in &mut state {
            sm = splitmix64(&mut sm);
            *slot = sm;
        }
        if state == [0; 4] {
            state[0] = 0x5555_AAAA_1234_5678;
        }
        Self { state }
    }

    /// Advances state and returns the next 64-bit pseudorandom integer.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;

        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];

        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    /// Returns a uniformly distributed integer in `[0, max_excl)`, or `0` if `max_excl <= 0`.
    pub fn next_bounded_i32(&mut self, max_excl: i32) -> i32 {
        if max_excl <= 0 {
            return 0;
        }
        let r = (self.next_u64() >> 32) as u32;
        ((u64::from(r) * u64::from(max_excl as u32)) >> 32) as i32
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
