use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::world::World;

/// Largest supported resolution per axis.
pub const MAX_RESOLUTION: u32 = 16384;

/// Where a grid sits in the world and how finely it samples it.
///
/// Grids are **vertex-aligned**: sample `(0, 0)` is exactly at `origin_m` and
/// sample `(width-1, height-1)` is exactly at `origin_m + extent_m`, like the
/// vertices of a terrain mesh (and like Unreal's 1009/2017/4033 landscape sizes).
///
/// Grids are tile-aware from day one: a tile is simply a grid whose origin is not
/// the world origin. Nodes must compute positions through [`GridSpec::x_m`] /
/// [`GridSpec::y_m`] so the same world position gives the same value in any tile
/// and at any resolution.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridSpec {
    pub width: u32,
    pub height: u32,
    /// World position of sample (0, 0), in metres.
    pub origin_m: [f64; 2],
    /// World distance covered from the first to the last sample, in metres.
    pub extent_m: [f64; 2],
}

impl GridSpec {
    /// A grid covering the whole world at `resolution` samples per axis.
    pub fn full_world(world: &World, resolution: u32) -> Result<Self> {
        if !(2..=MAX_RESOLUTION).contains(&resolution) {
            return Err(CoreError::InvalidResolution(resolution));
        }
        Ok(Self {
            width: resolution,
            height: resolution,
            origin_m: [0.0, 0.0],
            extent_m: world.size_m,
        })
    }

    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Distance between neighbouring samples, in metres.
    pub fn cell_size_m(&self) -> [f64; 2] {
        [
            self.extent_m[0] / (self.width.max(2) - 1) as f64,
            self.extent_m[1] / (self.height.max(2) - 1) as f64,
        ]
    }

    /// World x (metres) of column `i`.
    #[inline]
    pub fn x_m(&self, i: u32) -> f64 {
        self.origin_m[0] + self.extent_m[0] * (i as f64 / (self.width - 1) as f64)
    }

    /// World y (metres) of row `j`.
    #[inline]
    pub fn y_m(&self, j: u32) -> f64 {
        self.origin_m[1] + self.extent_m[1] * (j as f64 / (self.height - 1) as f64)
    }
}

/// A 2D grid of `f32` samples over part of the world.
///
/// Used for heightfields (metres) and masks (0..1). Row-major, row 0 first.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub spec: GridSpec,
    pub data: Vec<f32>,
}

impl Grid {
    pub fn filled(spec: GridSpec, value: f32) -> Self {
        Self {
            spec,
            data: vec![value; spec.len()],
        }
    }

    /// Build a grid by evaluating `f(x_m, y_m)` at every sample, rows in parallel.
    ///
    /// Each sample depends only on its own position, so the result is identical
    /// for any number of threads.
    pub fn from_fn<F>(spec: GridSpec, f: F) -> Self
    where
        F: Fn(f64, f64) -> f32 + Sync,
    {
        let mut data = vec![0.0f32; spec.len()];
        data.par_chunks_mut(spec.width as usize)
            .enumerate()
            .for_each(|(j, row)| {
                let y = spec.y_m(j as u32);
                for (i, v) in row.iter_mut().enumerate() {
                    *v = f(spec.x_m(i as u32), y);
                }
            });
        Self { spec, data }
    }

    /// Like [`Grid::from_fn`], but `f` also gets the flat sample index (for
    /// reading other grids or per-cell parameters at the same sample).
    pub fn from_fn_indexed<F>(spec: GridSpec, f: F) -> Self
    where
        F: Fn(usize, f64, f64) -> f32 + Sync,
    {
        let w = spec.width as usize;
        let mut data = vec![0.0f32; spec.len()];
        data.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            let y = spec.y_m(j as u32);
            for (i, v) in row.iter_mut().enumerate() {
                *v = f(j * w + i, spec.x_m(i as u32), y);
            }
        });
        Self { spec, data }
    }

    /// Apply `f(index, value)` to every sample, in parallel.
    pub fn map_indexed<F>(&self, f: F) -> Self
    where
        F: Fn(usize, f32) -> f32 + Sync,
    {
        let data = self.data.par_iter().enumerate().map(|(i, &v)| f(i, v)).collect();
        Self {
            spec: self.spec,
            data,
        }
    }

    /// Apply `f` to every sample, in parallel.
    pub fn map<F>(&self, f: F) -> Self
    where
        F: Fn(f32) -> f32 + Sync,
    {
        let data = self.data.par_iter().map(|&v| f(v)).collect();
        Self {
            spec: self.spec,
            data,
        }
    }

    #[inline]
    pub fn get(&self, i: u32, j: u32) -> f32 {
        self.data[j as usize * self.spec.width as usize + i as usize]
    }

    /// Sample with clamped coordinates: edges never read out of bounds.
    #[inline]
    pub fn get_clamped(&self, i: i64, j: i64) -> f32 {
        let i = i.clamp(0, self.spec.width as i64 - 1) as u32;
        let j = j.clamp(0, self.spec.height as i64 - 1) as u32;
        self.get(i, j)
    }

    /// Bilinear sample at a world position in metres (clamped at the edges).
    pub fn sample_bilinear_m(&self, x_m: f64, y_m: f64) -> f32 {
        let fx = (x_m - self.spec.origin_m[0]) / self.spec.extent_m[0] * (self.spec.width - 1) as f64;
        let fy = (y_m - self.spec.origin_m[1]) / self.spec.extent_m[1] * (self.spec.height - 1) as f64;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.get_clamped(x0, y0);
        let b = self.get_clamped(x0 + 1, y0);
        let c = self.get_clamped(x0, y0 + 1);
        let d = self.get_clamped(x0 + 1, y0 + 1);
        let top = a + (b - a) * tx;
        let bottom = c + (d - c) * tx;
        top + (bottom - top) * ty
    }

    /// Minimum and maximum sample values.
    pub fn min_max(&self) -> (f32, f32) {
        self.data
            .par_iter()
            .fold(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |(lo, hi), &v| (lo.min(v), hi.max(v)),
            )
            .reduce(
                || (f32::INFINITY, f32::NEG_INFINITY),
                |a, b| (a.0.min(b.0), a.1.max(b.1)),
            )
    }

    /// Mean sample value (computed in f64, order-independent enough for stats).
    pub fn mean(&self) -> f64 {
        if self.data.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.data.iter().map(|&v| v as f64).sum();
        sum / self.data.len() as f64
    }
}

/// A colour per sample: red, green, blue and alpha, each 0..1, row-major and
/// interleaved (`data[4 × index + channel]`). Colours are sRGB-encoded, as
/// painted and as stored in 8/16-bit images; see `docs/colour.md`.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorGrid {
    pub spec: GridSpec,
    pub data: Vec<f32>,
}

impl ColorGrid {
    /// Build by evaluating `f(index, x_m, y_m)` at every sample, rows in parallel.
    pub fn from_fn_indexed<F>(spec: GridSpec, f: F) -> Self
    where
        F: Fn(usize, f64, f64) -> [f32; 4] + Sync,
    {
        let w = spec.width as usize;
        let mut data = vec![0.0f32; spec.len() * 4];
        data.par_chunks_mut(w * 4).enumerate().for_each(|(j, row)| {
            let y = spec.y_m(j as u32);
            for (i, px) in row.chunks_mut(4).enumerate() {
                px.copy_from_slice(&f(j * w + i, spec.x_m(i as u32), y));
            }
        });
        Self { spec, data }
    }

    /// Colour of sample `index`.
    #[inline]
    pub fn at(&self, index: usize) -> [f32; 4] {
        let d = &self.data[index * 4..index * 4 + 4];
        [d[0], d[1], d[2], d[3]]
    }

    /// One channel (0 = red … 3 = alpha) as a grid.
    pub fn channel(&self, c: usize) -> Grid {
        Grid {
            spec: self.spec,
            data: self.data.par_chunks(4).map(|px| px[c]).collect(),
        }
    }

    /// Bilinear colour at a world position in metres (clamped at the edges).
    pub fn sample_bilinear_m(&self, x_m: f64, y_m: f64) -> [f32; 4] {
        std::array::from_fn(|c| {
            let s = self.spec;
            let fx = (x_m - s.origin_m[0]) / s.extent_m[0] * (s.width - 1) as f64;
            let fy = (y_m - s.origin_m[1]) / s.extent_m[1] * (s.height - 1) as f64;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = ((fx - x0) as f32, (fy - y0) as f32);
            let at = |i: i64, j: i64| {
                let i = i.clamp(0, s.width as i64 - 1) as usize;
                let j = j.clamp(0, s.height as i64 - 1) as usize;
                self.data[(j * s.width as usize + i) * 4 + c]
            };
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// sRGB-encoded value (0..1) to linear light.
#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        libm::powf((v + 0.055) / 1.055, 2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_grids_sample_and_split_channels() {
        let c = ColorGrid::from_fn_indexed(spec(3), |i, x, _| [i as f32, (x / 8192.0) as f32, 0.5, 1.0]);
        assert_eq!(c.at(4), [4.0, 0.5, 0.5, 1.0]);
        assert_eq!(c.channel(0).data, (0..9).map(|i| i as f32).collect::<Vec<_>>());
        assert_eq!(c.sample_bilinear_m(2048.0, 0.0), [0.5, 0.25, 0.5, 1.0]);
        assert!((srgb_to_linear(0.5) - 0.214).abs() < 1.0e-3);
    }

    fn spec(res: u32) -> GridSpec {
        GridSpec::full_world(&World::default(), res).unwrap()
    }

    #[test]
    fn vertex_aligned_positions() {
        let s = spec(5);
        assert_eq!(s.x_m(0), 0.0);
        assert_eq!(s.x_m(4), 8192.0);
        assert_eq!(s.x_m(2), 4096.0);
        assert_eq!(s.cell_size_m(), [2048.0, 2048.0]);
    }

    #[test]
    fn rejects_bad_resolution() {
        assert!(GridSpec::full_world(&World::default(), 1).is_err());
        assert!(GridSpec::full_world(&World::default(), MAX_RESOLUTION + 1).is_err());
    }

    #[test]
    fn from_fn_and_bilinear_agree_on_planes() {
        // A linear function is reproduced exactly by bilinear sampling.
        let g = Grid::from_fn(spec(65), |x, y| (x * 0.25 + y * 0.5) as f32);
        let v = g.sample_bilinear_m(1000.0, 3000.0);
        assert!((v - (250.0 + 1500.0)).abs() < 1e-2, "got {v}");
        let (lo, hi) = g.min_max();
        assert_eq!(lo, 0.0);
        assert!((hi - (8192.0 * 0.75)).abs() < 1e-3);
    }

    #[test]
    fn clamped_access_never_panics() {
        let g = Grid::filled(spec(4), 7.0);
        assert_eq!(g.get_clamped(-5, -5), 7.0);
        assert_eq!(g.get_clamped(100, 100), 7.0);
    }
}
