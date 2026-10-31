//! xoshiro256** (Blackman and Vigna, ACM TOMS 2021), seeded from 32 bytes.
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    /// The seed must be high-entropy bytes, such as a SHA-256 digest. A sparse seed
    /// (mostly zero bytes) gives near-zero first outputs.
    pub fn from_seed(seed: [u8; 32]) -> Rng {
        let mut s = [0u64; 4];
        for (w, c) in s.iter_mut().zip(seed.as_chunks::<8>().0) {
            *w = u64::from_le_bytes(*c);
        }
        // The all-zero state is a fixed point of the generator.
        if s == [0; 4] {
            s[0] = 0x9E37_79B9_7F4A_7C15;
        }
        Rng { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let out = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        out
    }

    /// Uniform on (0, 1]. Zero is excluded so `ln` is always finite.
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) + 1) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform on `0..n` by rejection: draw x, draw again while x ≥ 2^64 − (2^64 mod n),
    /// then return x mod n. `n = 0` stands for 2^64, so the draw comes back as it is.
    pub fn below(&mut self, n: u64) -> u64 {
        // 2^64 mod n is (2^64 − n) mod n.
        let zone = n.wrapping_neg().checked_rem(n).unwrap_or(0);
        loop {
            let x = self.next_u64();
            if x <= u64::MAX - zone {
                return x.checked_rem(n).unwrap_or(x);
            }
        }
    }

    /// `min(k, available)` distinct values of `0..available`: the first positions of a forward
    /// Fisher-Yates shuffle, where step i swaps position i with i + below(available − i).
    /// Only moved positions are stored, so memory grows with k, not with `available`.
    pub fn sample(&mut self, available: u64, k: u64) -> Vec<u64> {
        let mut moved = BTreeMap::new();
        (0..k.min(available))
            .map(|i| {
                let j = i + self.below(available - i);
                let at_i = moved.get(&i).copied().unwrap_or(i);
                moved.insert(j, at_i).unwrap_or(j)
            })
            .collect()
    }

    /// Standard normal by Box-Muller; one value per call.
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform();
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}
