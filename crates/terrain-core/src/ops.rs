//! Grid algorithms shared by nodes: blur, gradients and distance transforms.
//!
//! Every function takes sizes in metres and converts to cells using the
//! grid's cell size, so results look the same at any resolution. Rows (and
//! columns) are processed independently in parallel, and every sum runs in a
//! fixed order, so results are identical for any number of threads.

use rayon::prelude::*;

use crate::grid::Grid;

/// Hermite smoothstep: 0 below `e0`, 1 above `e1`, smooth in between.
/// Works for `e0 > e1` too (reversed).
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e0 == e1 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Same as [`smoothstep`] in f64.
#[inline]
pub fn smoothstep64(e0: f64, e1: f64, x: f64) -> f64 {
    if e0 == e1 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A soft "is `x` inside `lo..hi`" test: 1 inside, 0 further than `falloff`
/// outside, smooth in between.
#[inline]
pub fn soft_range(x: f32, lo: f32, hi: f32, falloff: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if falloff <= 0.0 {
        return if x >= lo && x <= hi { 1.0 } else { 0.0 };
    }
    smoothstep(lo - falloff, lo, x) * (1.0 - smoothstep(hi, hi + falloff, x))
}

/// Transpose a row-major `w × h` buffer.
fn transpose(data: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; data.len()];
    out.par_chunks_mut(h).enumerate().for_each(|(i, col)| {
        for (j, v) in col.iter_mut().enumerate() {
            *v = data[j * w + i];
        }
    });
    out
}

/// Blur every row of a row-major `w`-wide buffer with a Gaussian of standard
/// deviation `sigma` cells.
fn blur_rows(data: &mut [f32], w: usize, sigma: f64) {
    if sigma < 0.2 || w < 2 {
        return;
    }
    if sigma <= 6.0 {
        // Exact separable kernel.
        let r = (sigma * 3.0).ceil() as i64;
        let mut kernel: Vec<f64> = (-r..=r)
            .map(|x| libm::exp(-((x * x) as f64) / (2.0 * sigma * sigma)))
            .collect();
        let sum: f64 = kernel.iter().sum();
        kernel.iter_mut().for_each(|k| *k /= sum);
        data.par_chunks_mut(w).for_each(|row| {
            let src = row.to_vec();
            let last = w as i64 - 1;
            for (i, v) in row.iter_mut().enumerate() {
                let mut acc = 0.0f64;
                for (k, weight) in kernel.iter().enumerate() {
                    let j = (i as i64 + k as i64 - r).clamp(0, last) as usize;
                    acc += src[j] as f64 * weight;
                }
                *v = acc as f32;
            }
        });
    } else {
        // Three box blurs approximate a Gaussian (Kovesi 2010).
        let boxes = box_sizes(sigma, 3);
        data.par_chunks_mut(w).for_each(|row| {
            let mut buf = row.to_vec();
            let mut ext = Vec::new();
            for &size in &boxes {
                box_blur_row(&mut buf, size / 2, &mut ext);
            }
            row.copy_from_slice(&buf);
        });
    }
}

/// Widths of `n` box filters whose combination approximates a Gaussian.
fn box_sizes(sigma: f64, n: usize) -> Vec<usize> {
    let nf = n as f64;
    let ideal = (12.0 * sigma * sigma / nf + 1.0).sqrt();
    let mut wl = ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let wlf = wl as f64;
    let m = ((12.0 * sigma * sigma - nf * wlf * wlf - 4.0 * nf * wlf - 3.0 * nf) / (-4.0 * wlf - 4.0)).round()
        as i64;
    (0..n as i64)
        .map(|i| if i < m { wl as usize } else { wu as usize })
        .collect()
}

/// Box blur of radius `r` cells with edge values repeated past the ends.
fn box_blur_row(row: &mut [f32], r: usize, ext: &mut Vec<f64>) {
    let n = row.len();
    // Prefix sums over the row extended by r repeated edge values on each side.
    ext.clear();
    ext.push(0.0);
    let mut acc = 0.0f64;
    for k in 0..n + 2 * r {
        let j = (k as i64 - r as i64).clamp(0, n as i64 - 1) as usize;
        acc += row[j] as f64;
        ext.push(acc);
    }
    let width = (2 * r + 1) as f64;
    for (i, v) in row.iter_mut().enumerate() {
        *v = ((ext[i + 2 * r + 1] - ext[i]) / width) as f32;
    }
}

/// Gaussian blur with standard deviation `sigma_m` metres. Edges repeat.
pub fn gaussian_blur(grid: &Grid, sigma_m: f64) -> Grid {
    let (w, h) = (grid.spec.width as usize, grid.spec.height as usize);
    let cell = grid.spec.cell_size_m();
    let mut data = grid.data.clone();
    blur_rows(&mut data, w, sigma_m / cell[0]);
    let mut t = transpose(&data, w, h);
    blur_rows(&mut t, h, sigma_m / cell[1]);
    Grid {
        spec: grid.spec,
        data: transpose(&t, h, w),
    }
}

/// Height gradient `(dh/dx, dh/dy)` at every sample, in metres per metre
/// (central differences, one-sided at the edges).
pub fn gradient(grid: &Grid) -> (Grid, Grid) {
    let (w, h) = (grid.spec.width as i64, grid.spec.height as i64);
    let cell = grid.spec.cell_size_m();
    let (cx, cy) = (cell[0] as f32, cell[1] as f32);
    let mut gx = Grid::filled(grid.spec, 0.0);
    let mut gy = Grid::filled(grid.spec, 0.0);
    gx.data
        .par_chunks_mut(w as usize)
        .zip(gy.data.par_chunks_mut(w as usize))
        .enumerate()
        .for_each(|(j, (rx, ry))| {
            let j = j as i64;
            let (j0, j1) = ((j - 1).max(0), (j + 1).min(h - 1));
            for i in 0..w {
                let (i0, i1) = ((i - 1).max(0), (i + 1).min(w - 1));
                rx[i as usize] =
                    (grid.get_clamped(i1, j) - grid.get_clamped(i0, j)) / ((i1 - i0).max(1) as f32 * cx);
                ry[i as usize] =
                    (grid.get_clamped(i, j1) - grid.get_clamped(i, j0)) / ((j1 - j0).max(1) as f32 * cy);
            }
        });
    (gx, gy)
}

/// Slope angle in degrees (0 = flat, 90 = vertical) at every sample.
pub fn slope_degrees(grid: &Grid) -> Grid {
    let (gx, gy) = gradient(grid);
    let data = gx
        .data
        .par_iter()
        .zip(&gy.data)
        .map(|(&x, &y)| libm::atan((x as f64 * x as f64 + y as f64 * y as f64).sqrt()).to_degrees() as f32)
        .collect();
    Grid {
        spec: grid.spec,
        data,
    }
}

const FAR: f64 = 1e20;

/// 1D squared-distance transform (Felzenszwalb & Huttenlocher 2012) of `f`,
/// with samples `step` apart.
fn dt_1d(f: &[f64], step: f64, out: &mut [f64], v: &mut Vec<usize>, z: &mut Vec<f64>) {
    let n = f.len();
    v.clear();
    z.clear();
    v.resize(n, 0);
    z.resize(n + 1, 0.0);
    let s2 = step * step;
    let meet = |q: usize, p: usize| {
        ((f[q] + s2 * (q * q) as f64) - (f[p] + s2 * (p * p) as f64)) / (2.0 * s2 * (q - p) as f64)
    };
    let mut k = 0usize;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        // z[0] is -inf, so k never goes below 0.
        let mut s = meet(q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = meet(q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let d = (q as f64 - v[k] as f64) * step;
        *o = d * d + f[v[k]];
    }
}

/// Euclidean distance in metres from every sample to the nearest sample where
/// `inside` is true. Returns a large value everywhere if nothing is inside.
pub fn distance_to(grid_spec: crate::grid::GridSpec, inside: &[bool]) -> Grid {
    let (w, h) = (grid_spec.width as usize, grid_spec.height as usize);
    let cell = grid_spec.cell_size_m();
    // Columns first (transpose so they're contiguous), then rows.
    let init: Vec<f64> = inside.iter().map(|&b| if b { 0.0 } else { FAR }).collect();
    let mut cols = vec![0.0f64; w * h];
    cols.par_chunks_mut(h).enumerate().for_each(|(i, col)| {
        let f: Vec<f64> = (0..h).map(|j| init[j * w + i]).collect();
        let (mut v, mut z) = (Vec::new(), Vec::new());
        dt_1d(&f, cell[1], col, &mut v, &mut z);
    });
    let mut data = vec![0.0f32; w * h];
    data.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
        let f: Vec<f64> = (0..w).map(|i| cols[i * h + j]).collect();
        let mut out = vec![0.0f64; w];
        let (mut v, mut z) = (Vec::new(), Vec::new());
        dt_1d(&f, cell[0], &mut out, &mut v, &mut z);
        for (o, d) in row.iter_mut().zip(out) {
            *o = d.sqrt() as f32;
        }
    });
    Grid {
        spec: grid_spec,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridSpec;
    use crate::world::World;

    fn spec(res: u32) -> GridSpec {
        GridSpec::full_world(&World::default(), res).unwrap()
    }

    #[test]
    fn blur_keeps_constants_and_mean() {
        let g = Grid::filled(spec(64), 5.0);
        let b = gaussian_blur(&g, 500.0);
        assert!(b.data.iter().all(|&v| (v - 5.0).abs() < 1e-4));
        // A single spike spreads out but keeps (roughly) its total.
        let mut spike = Grid::filled(spec(129), 0.0);
        spike.data[64 * 129 + 64] = 1000.0;
        for sigma_m in [100.0, 1000.0] {
            let b = gaussian_blur(&spike, sigma_m);
            let total: f64 = b.data.iter().map(|&v| v as f64).sum();
            assert!((total - 1000.0).abs() < 1.0, "sigma {sigma_m}: total {total}");
            assert!(b.get(64, 64) < 1000.0 && b.get(64, 64) > b.get(70, 64));
        }
    }

    #[test]
    fn blur_matches_across_resolutions() {
        // A step edge blurred by 400 m looks the same at 129 and 513 samples.
        let f = |x: f64, _y: f64| if x < 4096.0 { 0.0 } else { 100.0 };
        let lo = gaussian_blur(&Grid::from_fn(spec(129), f), 400.0);
        let hi = gaussian_blur(&Grid::from_fn(spec(513), f), 400.0);
        for i in 0..129 {
            let d = (lo.get(i, 64) - hi.get(i * 4, 256)).abs();
            assert!(d < 4.0, "column {i}: {d}");
        }
    }

    #[test]
    fn gradient_of_a_plane() {
        let g = Grid::from_fn(spec(33), |x, y| (x * 0.5 - y * 0.25) as f32);
        let (gx, gy) = gradient(&g);
        assert!(gx.data.iter().all(|&v| (v - 0.5).abs() < 1e-4));
        assert!(gy.data.iter().all(|&v| (v + 0.25).abs() < 1e-4));
        let s = slope_degrees(&Grid::from_fn(spec(33), |x, _| x as f32));
        assert!(s.data.iter().all(|&v| (v - 45.0).abs() < 1e-3));
    }

    #[test]
    fn distance_transform_is_euclidean() {
        let sp = spec(65); // 128 m cells
        let mut inside = vec![false; sp.len()];
        inside[32 * 65 + 32] = true;
        let d = distance_to(sp, &inside);
        assert_eq!(d.get(32, 32), 0.0);
        assert!((d.get(35, 32) - 384.0).abs() < 1e-3);
        assert!((d.get(35, 36) - 640.0).abs() < 1e-3); // 3-4-5 triangle
        let none = distance_to(sp, &vec![false; sp.len()]);
        assert!(none.data[0] > 1e9);
    }

    #[test]
    fn soft_range_edges() {
        assert_eq!(soft_range(5.0, 0.0, 10.0, 1.0), 1.0);
        assert_eq!(soft_range(-2.0, 0.0, 10.0, 1.0), 0.0);
        assert!((soft_range(-0.5, 0.0, 10.0, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(soft_range(10.5, 0.0, 10.0, 0.0), 0.0);
    }
}
