//! Deterministic, dependency-free hashing and noise.
//!
//! Deepcore must generate the *same* world on every machine from one seed. That rules out
//! anything that depends on floating point library behaviour across platforms where we can
//! avoid it, and rules out unseeded randomness entirely. What is left is integer hashing:
//! a splitmix64 finaliser over lattice coordinates, then smooth interpolation.
//!
//! Everything here is pure: same inputs, same outputs, forever.

/// splitmix64 finaliser: a strong, cheap 64-bit mixer.
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Hashes a 3D integer lattice point plus a seed to a `u64`.
#[inline]
pub fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut h = seed;
    h ^= (x as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= (y as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= (z as i64 as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    mix64(h)
}

/// Hashes a 2D integer point plus a seed to a `u64`.
#[inline]
pub fn hash2(seed: u64, x: i32, z: i32) -> u64 {
    hash3(seed, x, 0, z)
}

/// Converts a hash to a float in `0..1` with 24 bits of precision.
#[inline]
pub fn unit_float(hash: u64) -> f32 {
    // Take the top 24 bits so every value is exactly representable as f32.
    ((hash >> 40) as f32) / ((1u32 << 24) as f32)
}

/// A seeded 3D value-noise field.
#[derive(Clone, Copy, Debug)]
pub struct Noise3 {
    seed: u64,
}

impl Noise3 {
    /// Creates a noise field for a seed.
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// Value at an integer lattice point, in `0..1`.
    #[inline]
    pub fn lattice(&self, x: i32, y: i32, z: i32) -> f32 {
        unit_float(hash3(self.seed, x, y, z))
    }

    /// Smoothstep interpolation weight.
    #[inline]
    fn fade(t: f32) -> f32 {
        t * t * (3.0 - 2.0 * t)
    }

    /// Trilinearly interpolated value noise at a world position.
    ///
    /// `scale` is the wavelength in blocks: a scale of 32 means the field repeats its
    /// features roughly every 32 blocks.
    pub fn sample(&self, x: f32, y: f32, z: f32, scale: f32) -> f32 {
        let fx = x / scale;
        let fy = y / scale;
        let fz = z / scale;

        let x0 = fx.floor();
        let y0 = fy.floor();
        let z0 = fz.floor();
        let tx = Self::fade(fx - x0);
        let ty = Self::fade(fy - y0);
        let tz = Self::fade(fz - z0);

        let (x0, y0, z0) = (x0 as i32, y0 as i32, z0 as i32);
        let (x1, y1, z1) = (x0 + 1, y0 + 1, z0 + 1);

        let c000 = self.lattice(x0, y0, z0);
        let c100 = self.lattice(x1, y0, z0);
        let c010 = self.lattice(x0, y1, z0);
        let c110 = self.lattice(x1, y1, z0);
        let c001 = self.lattice(x0, y0, z1);
        let c101 = self.lattice(x1, y0, z1);
        let c011 = self.lattice(x0, y1, z1);
        let c111 = self.lattice(x1, y1, z1);

        let c00 = c000 + (c100 - c000) * tx;
        let c10 = c010 + (c110 - c010) * tx;
        let c01 = c001 + (c101 - c001) * tx;
        let c11 = c011 + (c111 - c011) * tx;

        let c0 = c00 + (c10 - c00) * ty;
        let c1 = c01 + (c11 - c01) * ty;

        c0 + (c1 - c0) * tz
    }

    /// Two-octave fractal noise in `0..1`, which looks less blobby than plain value noise.
    pub fn fbm(&self, x: f32, y: f32, z: f32, scale: f32) -> f32 {
        let a = self.sample(x, y, z, scale);
        let b = self.sample(x + 1000.0, y + 1000.0, z + 1000.0, scale * 0.45);
        (a * 0.68 + b * 0.32).clamp(0.0, 1.0)
    }
}

/// A seeded 2D height field for the surface.
#[derive(Clone, Copy, Debug)]
pub struct Noise2 {
    seed: u64,
}

impl Noise2 {
    /// Creates a 2D noise field for a seed.
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// Value at a lattice point, in `0..1`.
    #[inline]
    pub fn lattice(&self, x: i32, z: i32) -> f32 {
        unit_float(hash2(self.seed, x, z))
    }

    /// Bilinearly interpolated value noise at a world position.
    pub fn sample(&self, x: f32, z: f32, scale: f32) -> f32 {
        let fx = x / scale;
        let fz = z / scale;
        let x0 = fx.floor();
        let z0 = fz.floor();
        let tx = Noise3::fade(fx - x0);
        let tz = Noise3::fade(fz - z0);
        let (x0, z0) = (x0 as i32, z0 as i32);

        let c00 = self.lattice(x0, z0);
        let c10 = self.lattice(x0 + 1, z0);
        let c01 = self.lattice(x0, z0 + 1);
        let c11 = self.lattice(x0 + 1, z0 + 1);

        let c0 = c00 + (c10 - c00) * tx;
        let c1 = c01 + (c11 - c01) * tx;
        c0 + (c1 - c0) * tz
    }
}

/// A small deterministic RNG for per-chunk decisions.
///
/// This is a splitmix64 generator: one `u64` of state, excellent distribution, and identical
/// output on every platform. Seeded from `(world seed, chunk position)` so a chunk's ore
/// rolls do not depend on the order chunks are generated in.
#[derive(Clone, Debug)]
pub struct Rng64 {
    state: u64,
}

impl Rng64 {
    /// Creates an RNG from a seed.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Creates an RNG deterministically tied to a world seed and a chunk.
    pub fn for_chunk(world_seed: u64, chunk: crate::voxel::ChunkPos, salt: u64) -> Self {
        let seed = mix64(world_seed ^ mix64(salt ^ hash3(world_seed, chunk.x, chunk.y, chunk.z)));
        Self::new(seed)
    }

    /// Next raw 64-bit value.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix64(self.state)
    }

    /// Next float in `0..1`.
    pub fn next_f32(&mut self) -> f32 {
        unit_float(self.next_u64())
    }

    /// Next integer in `0..bound` (exclusive). `bound` must be positive.
    pub fn next_below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % u64::from(bound)) as u32
    }

    /// Next integer in `low..=high`.
    pub fn next_range(&mut self, low: i32, high: i32) -> i32 {
        if high <= low {
            return low;
        }
        low + self.next_below((high - low + 1) as u32) as i32
    }

    /// True with probability `chance` (0..1).
    pub fn chance(&mut self, chance: f32) -> bool {
        self.next_f32() < chance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashing_is_deterministic_and_varies() {
        assert_eq!(hash3(7, 1, 2, 3), hash3(7, 1, 2, 3));
        assert_ne!(hash3(7, 1, 2, 3), hash3(7, 1, 2, 4));
        assert_ne!(hash3(7, 1, 2, 3), hash3(8, 1, 2, 3));
    }

    #[test]
    fn unit_float_stays_in_range() {
        for i in 0..10_000u64 {
            let f = unit_float(hash3(1, i as i32, 0, 0));
            assert!((0.0..1.0).contains(&f), "value out of range: {f}");
        }
    }

    #[test]
    fn noise_is_in_range_and_smooth() {
        let noise = Noise3::new(42);
        let mut previous = noise.sample(0.0, 0.0, 0.0, 16.0);
        for i in 1..500 {
            let value = noise.sample(i as f32, 0.0, 0.0, 16.0);
            assert!((0.0..=1.0).contains(&value));
            assert!((value - previous).abs() < 0.35, "noise jumps too fast at {i}");
            previous = value;
        }
    }

    #[test]
    fn noise_is_reproducible_across_instances() {
        let a = Noise3::new(99);
        let b = Noise3::new(99);
        for i in 0..50 {
            let p = i as f32 * 1.7;
            assert_eq!(a.fbm(p, p, p, 24.0), b.fbm(p, p, p, 24.0));
        }
    }

    #[test]
    fn rng_is_deterministic_for_the_same_chunk() {
        let a = Rng64::for_chunk(1234, crate::voxel::ChunkPos::new(3, 4, 5), 1);
        let b = Rng64::for_chunk(1234, crate::voxel::ChunkPos::new(3, 4, 5), 1);
        let c = Rng64::for_chunk(1234, crate::voxel::ChunkPos::new(3, 4, 6), 1);
        let mut a = a;
        let mut b = b;
        let mut c = c;
        assert_eq!(a.next_u64(), b.next_u64());
        assert_ne!(a.next_u64(), c.next_u64());
    }

    #[test]
    fn rng_ranges_are_respected() {
        let mut rng = Rng64::new(5);
        for _ in 0..1000 {
            let v = rng.next_below(10);
            assert!(v < 10);
            let r = rng.next_range(4, 8);
            assert!((4..=8).contains(&r));
        }
    }

    #[test]
    fn chance_is_roughly_calibrated() {
        let mut rng = Rng64::new(77);
        let hits = (0..20_000).filter(|_| rng.chance(0.25)).count();
        assert!((4500..5500).contains(&hits), "got {hits} hits out of 20000 at p=0.25");
    }
}
