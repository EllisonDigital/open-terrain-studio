//! Gradient noise functions, implemented from the published algorithms
//! (Perlin 2002 "Improving Noise"; Gustavson 2005 "Simplex noise demystified").
//!
//! Written in-house rather than taken from a crate so results can never change
//! under us: every terrain ever made depends on these exact numbers. Only
//! `+ - * /` and `floor` are used, which are identical on every platform.

use std::f64::consts::FRAC_1_SQRT_2;

use terrain_core::seed::mix64;

/// 16 unit gradient directions (angles k × 22.5°), as exact constants so no
/// platform trigonometry is involved.
const GRADIENTS: [(f64, f64); 16] = [
    (1.0, 0.0),
    (0.923_879_532_511_286_7, 0.382_683_432_365_089_8),
    (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (0.382_683_432_365_089_8, 0.923_879_532_511_286_7),
    (0.0, 1.0),
    (-0.382_683_432_365_089_8, 0.923_879_532_511_286_7),
    (-FRAC_1_SQRT_2, FRAC_1_SQRT_2),
    (-0.923_879_532_511_286_7, 0.382_683_432_365_089_8),
    (-1.0, 0.0),
    (-0.923_879_532_511_286_7, -0.382_683_432_365_089_8),
    (-FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (-0.382_683_432_365_089_8, -0.923_879_532_511_286_7),
    (0.0, -1.0),
    (0.382_683_432_365_089_8, -0.923_879_532_511_286_7),
    (FRAC_1_SQRT_2, -FRAC_1_SQRT_2),
    (0.923_879_532_511_286_7, -0.382_683_432_365_089_8),
];

/// Hash an integer lattice point with a seed.
#[inline]
fn hash2(ix: i64, iy: i64, seed: u64) -> u64 {
    mix64(
        mix64(seed ^ (ix as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15))
            ^ (iy as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f),
    )
}

#[inline]
fn gradient(ix: i64, iy: i64, seed: u64) -> (f64, f64) {
    GRADIENTS[(hash2(ix, iy, seed) >> 60) as usize]
}

#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// 2D Perlin gradient noise at `(x, y)` (lattice units). Range roughly -1..1.
pub fn perlin(x: f64, y: f64, seed: u64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let (ix, iy) = (x0 as i64, y0 as i64);
    let (fx, fy) = (x - x0, y - y0);

    let dot = |gx: i64, gy: i64, dx: f64, dy: f64| {
        let g = gradient(gx, gy, seed);
        g.0 * dx + g.1 * dy
    };
    let n00 = dot(ix, iy, fx, fy);
    let n10 = dot(ix + 1, iy, fx - 1.0, fy);
    let n01 = dot(ix, iy + 1, fx, fy - 1.0);
    let n11 = dot(ix + 1, iy + 1, fx - 1.0, fy - 1.0);

    let u = fade(fx);
    let v = fade(fy);
    // The largest possible value with unit gradients is sqrt(0.5); scale to ±1.
    lerp(lerp(n00, n10, u), lerp(n01, n11, u), v) * std::f64::consts::SQRT_2
}

/// Skew/unskew factors for 2D simplex: (sqrt(3) - 1) / 2 and (3 - sqrt(3)) / 6.
const F2: f64 = 0.366_025_403_784_438_6;
const G2: f64 = 0.211_324_865_405_187_1;

/// Normalisation so the output spans about -1..1 with unit gradients.
const SIMPLEX_SCALE: f64 = 99.204_334_582_718_71;

/// 2D simplex gradient noise at `(x, y)` (lattice units). Range roughly -1..1.
pub fn simplex(x: f64, y: f64, seed: u64) -> f64 {
    let s = (x + y) * F2;
    let i = (x + s).floor();
    let j = (y + s).floor();
    let t = (i + j) * G2;
    let x0 = x - (i - t);
    let y0 = y - (j - t);
    let (i1, j1) = if x0 > y0 { (1i64, 0i64) } else { (0, 1) };
    let x1 = x0 - i1 as f64 + G2;
    let y1 = y0 - j1 as f64 + G2;
    let x2 = x0 - 1.0 + 2.0 * G2;
    let y2 = y0 - 1.0 + 2.0 * G2;
    let (ii, jj) = (i as i64, j as i64);

    let corner = |gx: i64, gy: i64, dx: f64, dy: f64| {
        let t = 0.5 - dx * dx - dy * dy;
        if t <= 0.0 {
            0.0
        } else {
            let g = gradient(gx, gy, seed);
            let t2 = t * t;
            t2 * t2 * (g.0 * dx + g.1 * dy)
        }
    };
    let n = corner(ii, jj, x0, y0) + corner(ii + i1, jj + j1, x1, y1) + corner(ii + 1, jj + 1, x2, y2);
    (n * SIMPLEX_SCALE).clamp(-1.0, 1.0)
}

/// Which basis function a fractal uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    Perlin,
    Simplex,
}

impl Basis {
    #[inline]
    pub fn sample(self, x: f64, y: f64, seed: u64) -> f64 {
        match self {
            Basis::Perlin => perlin(x, y, seed),
            Basis::Simplex => simplex(x, y, seed),
        }
    }
}

/// Fractal Brownian motion: `octaves` layers of noise, each `lacunarity` times
/// finer and `gain` times weaker. Each octave is rotated (by the exact 3-4-5
/// angle) and offset to hide lattice alignment. Returns about -1..1.
pub fn fbm(basis: Basis, x: f64, y: f64, seed: u64, octaves: u32, lacunarity: f64, gain: f64) -> f64 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    let (mut px, mut py) = (x, y);
    for o in 0..octaves {
        let s = terrain_core::seed::derive(seed, o as u64);
        sum += amp * basis.sample(px, py, s);
        norm += amp;
        amp *= gain;
        // Rotate by atan(3/4) and scale; the offset keeps the origin off-lattice.
        let rx = 0.8 * px - 0.6 * py;
        let ry = 0.6 * px + 0.8 * py;
        px = rx * lacunarity + 17.31;
        py = ry * lacunarity + 43.17;
    }
    if norm > 0.0 { sum / norm } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_of(f: impl Fn(f64, f64) -> f64) -> (f64, f64) {
        let mut lo = f64::MAX;
        let mut hi = f64::MIN;
        for j in 0..400 {
            for i in 0..400 {
                let v = f(i as f64 * 0.137, j as f64 * 0.093);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        (lo, hi)
    }

    #[test]
    fn perlin_is_zero_on_lattice_and_bounded() {
        assert_eq!(perlin(3.0, -7.0, 42), 0.0);
        let (lo, hi) = range_of(|x, y| perlin(x, y, 7));
        assert!(lo >= -1.0 && hi <= 1.0, "{lo} {hi}");
        assert!(lo < -0.6 && hi > 0.6, "range too small: {lo} {hi}");
    }

    #[test]
    fn simplex_is_bounded_and_uses_its_range() {
        let (lo, hi) = range_of(|x, y| simplex(x, y, 7));
        assert!(lo >= -1.0 && hi <= 1.0);
        assert!(lo < -0.6 && hi > 0.6, "range too small: {lo} {hi}");
    }

    #[test]
    fn seeds_change_output() {
        assert_ne!(perlin(0.5, 0.5, 1), perlin(0.5, 0.5, 2));
        assert_ne!(simplex(0.3, 0.7, 1), simplex(0.3, 0.7, 2));
    }

    #[test]
    fn noise_is_continuous() {
        for basis in [Basis::Perlin, Basis::Simplex] {
            let a = basis.sample(10.0, 10.0, 3);
            let b = basis.sample(10.0 + 1e-6, 10.0, 3);
            assert!((a - b).abs() < 1e-4, "{basis:?} jumps");
        }
    }

    #[test]
    fn fbm_is_bounded() {
        let (lo, hi) = range_of(|x, y| fbm(Basis::Perlin, x, y, 9, 6, 2.0, 0.5));
        assert!(lo >= -1.0 && hi <= 1.0);
    }

    /// Pins exact output values so any accidental change to the algorithm (which
    /// would change every user's terrain) fails CI.
    #[test]
    fn golden_values_never_change() {
        let p = perlin(1.25, 2.5, 12345);
        let s = simplex(1.25, 2.5, 12345);
        let f = fbm(Basis::Perlin, 1.25, 2.5, 12345, 6, 2.0, 0.5);
        println!("perlin={p:.17} simplex={s:.17} fbm={f:.17}");
        assert_eq!(p.to_bits(), GOLDEN_PERLIN.to_bits(), "perlin changed: {p:.17}");
        assert_eq!(s.to_bits(), GOLDEN_SIMPLEX.to_bits(), "simplex changed: {s:.17}");
        assert_eq!(f.to_bits(), GOLDEN_FBM.to_bits(), "fbm changed: {f:.17}");
    }

    const GOLDEN_PERLIN: f64 = 0.379_768_169_143_668_4;
    const GOLDEN_SIMPLEX: f64 = 0.322_913_333_697_025_8;
    const GOLDEN_FBM: f64 = -0.136_460_215_063_985_7;
}

#[cfg(test)]
mod calibrate {
    /// Run with `cargo test -p terrain-nodes calibrate -- --ignored --nocapture`
    /// to measure the raw simplex peak used for SIMPLEX_SCALE.
    #[test]
    #[ignore]
    fn measure_simplex_peak() {
        let mut peak: f64 = 0.0;
        for s in 0..20u64 {
            for j in 0..2000 {
                for i in 0..2000 {
                    let v = super::simplex(i as f64 * 0.0173, j as f64 * 0.0191, s) / super::SIMPLEX_SCALE;
                    peak = peak.max(v.abs());
                }
            }
        }
        println!("raw simplex peak = {peak:.8}, scale for ±1 = {:.8}", 1.0 / peak);
    }
}
