use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::world::World;

/// Largest resolution per axis of one grid held in memory.
pub const MAX_RESOLUTION: u32 = 16384;

/// Largest build resolution per axis. Builds are evaluated in tiles, so no
/// single grid gets this big.
pub const MAX_BUILD_RESOLUTION: u32 = 65537;

/// Where a grid sits in the world and how finely it samples it.
///
/// Grids are **vertex-aligned**: sample `(0, 0)` is exactly at `origin_m` and
/// sample `(width-1, height-1)` is exactly at `origin_m + extent_m`, like the
/// vertices of a terrain mesh (and like Unreal's 1009/2017/4033 landscape sizes).
///
/// A tile is a [`GridSpec::window`] of a larger grid. Nodes must compute
/// positions through [`GridSpec::x_m`] / [`GridSpec::y_m`] (and cell sizes
/// through [`GridSpec::cell_size_m`]) so the same world position gives the
/// same value in any tile and at any resolution.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GridSpec {
    pub width: u32,
    pub height: u32,
    /// World position of sample (0, 0), in metres.
    pub origin_m: [f64; 2],
    /// World distance covered from the first to the last sample, in metres.
    pub extent_m: [f64; 2],
    /// Set for a window of a larger grid (a tile of a build): positions and
    /// cell size then come from that grid, so every sample of a tile is
    /// bit-identical to the same sample of the whole grid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
}

/// Where a [`GridSpec`] window sits in the grid it was cut from.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Column and row of the window's sample (0, 0) in the whole grid.
    pub offset: [u32; 2],
    /// The whole grid's width and height.
    pub size: [u32; 2],
    /// The whole grid's origin and extent, in metres.
    pub origin_m: [f64; 2],
    pub extent_m: [f64; 2],
}

impl GridSpec {
    /// A grid covering the whole world at `resolution` samples per axis.
    pub fn full_world(world: &World, resolution: u32) -> Result<Self> {
        if !(2..=MAX_RESOLUTION).contains(&resolution) {
            return Err(CoreError::InvalidResolution(resolution));
        }
        Ok(Self::new(resolution, resolution, [0.0, 0.0], world.size_m))
    }

    /// The whole world at a build resolution, which may exceed
    /// [`MAX_RESOLUTION`]: only ever evaluated through windows.
    pub fn build_world(world: &World, resolution: u32) -> Result<Self> {
        if !(2..=MAX_BUILD_RESOLUTION).contains(&resolution) {
            return Err(CoreError::InvalidResolution(resolution));
        }
        Ok(Self::new(resolution, resolution, [0.0, 0.0], world.size_m))
    }

    /// A whole grid (not a window) of `width × height` samples.
    pub fn new(width: u32, height: u32, origin_m: [f64; 2], extent_m: [f64; 2]) -> Self {
        Self {
            width,
            height,
            origin_m,
            extent_m,
            window: None,
        }
    }

    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The grid this is a window of, or itself.
    pub fn whole(&self) -> GridSpec {
        match self.window {
            Some(w) => GridSpec::new(w.size[0], w.size[1], w.origin_m, w.extent_m),
            None => *self,
        }
    }

    /// Column and row of sample (0, 0) in [`GridSpec::whole`].
    pub fn offset(&self) -> [u32; 2] {
        self.window.map_or([0, 0], |w| w.offset)
    }

    /// The window of columns `i0..i0 + width` and rows `j0..j0 + height` of
    /// this grid's [whole grid](GridSpec::whole). The whole grid itself if
    /// the window covers all of it.
    ///
    /// # Panics
    /// If the window reaches outside the whole grid or is under 2 × 2.
    pub fn window(&self, i0: u32, j0: u32, width: u32, height: u32) -> GridSpec {
        let whole = self.whole();
        assert!(
            width >= 2 && height >= 2 && i0 + width <= whole.width && j0 + height <= whole.height,
            "window {i0},{j0} {width}×{height} outside a {}×{} grid",
            whole.width,
            whole.height
        );
        if i0 == 0 && j0 == 0 && width == whole.width && height == whole.height {
            return whole;
        }
        let first = [whole.x_m(i0), whole.y_m(j0)];
        let last = [whole.x_m(i0 + width - 1), whole.y_m(j0 + height - 1)];
        GridSpec {
            width,
            height,
            origin_m: first,
            extent_m: [last[0] - first[0], last[1] - first[1]],
            window: Some(Window {
                offset: [i0, j0],
                size: [whole.width, whole.height],
                origin_m: whole.origin_m,
                extent_m: whole.extent_m,
            }),
        }
    }

    /// Distance between neighbouring samples, in metres.
    pub fn cell_size_m(&self) -> [f64; 2] {
        if self.window.is_some() {
            return self.whole().cell_size_m();
        }
        [
            self.extent_m[0] / (self.width.max(2) - 1) as f64,
            self.extent_m[1] / (self.height.max(2) - 1) as f64,
        ]
    }

    /// World x (metres) of column `i`.
    #[inline]
    pub fn x_m(&self, i: u32) -> f64 {
        match self.window {
            Some(w) => w.origin_m[0] + w.extent_m[0] * ((i + w.offset[0]) as f64 / (w.size[0] - 1) as f64),
            None => self.origin_m[0] + self.extent_m[0] * (i as f64 / (self.width - 1) as f64),
        }
    }

    /// World y (metres) of row `j`.
    #[inline]
    pub fn y_m(&self, j: u32) -> f64 {
        match self.window {
            Some(w) => w.origin_m[1] + w.extent_m[1] * ((j + w.offset[1]) as f64 / (w.size[1] - 1) as f64),
            None => self.origin_m[1] + self.extent_m[1] * (j as f64 / (self.height - 1) as f64),
        }
    }

    /// Fractional column of world x (metres): 0 at the first sample.
    #[inline]
    pub fn column_at(&self, x_m: f64) -> f64 {
        match self.window {
            Some(w) => (x_m - w.origin_m[0]) / w.extent_m[0] * (w.size[0] - 1) as f64 - w.offset[0] as f64,
            None => (x_m - self.origin_m[0]) / self.extent_m[0] * (self.width - 1) as f64,
        }
    }

    /// Fractional row of world y (metres): 0 at the first sample.
    #[inline]
    pub fn row_at(&self, y_m: f64) -> f64 {
        match self.window {
            Some(w) => (y_m - w.origin_m[1]) / w.extent_m[1] * (w.size[1] - 1) as f64 - w.offset[1] as f64,
            None => (y_m - self.origin_m[1]) / self.extent_m[1] * (self.height - 1) as f64,
        }
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

    /// The part of this grid covered by `to`, a window of the same whole
    /// grid lying inside this one.
    pub fn crop(&self, to: GridSpec) -> Grid {
        Grid {
            spec: to,
            data: crop_samples(&self.data, 1, self.spec, to),
        }
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
        let fx = self.spec.column_at(x_m);
        let fy = self.spec.row_at(y_m);
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

    /// Value of the sample nearest a world position in metres (clamped at
    /// the edges), for categories and angles that must not be blended.
    pub fn sample_nearest_m(&self, x_m: f64, y_m: f64) -> f32 {
        let i = self.spec.column_at(x_m).round() as i64;
        let j = self.spec.row_at(y_m).round() as i64;
        self.get_clamped(i, j)
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

/// The samples (`channels` values each) of `from` that `to` covers. Both
/// must be windows of the same whole grid, `to` inside `from`.
fn crop_samples(data: &[f32], channels: usize, from: GridSpec, to: GridSpec) -> Vec<f32> {
    if from == to {
        return data.to_vec();
    }
    let (fo, to_o) = (from.offset(), to.offset());
    assert!(
        from.whole() == to.whole()
            && to_o[0] >= fo[0]
            && to_o[1] >= fo[1]
            && to_o[0] + to.width <= fo[0] + from.width
            && to_o[1] + to.height <= fo[1] + from.height,
        "crop to {to:?} from {from:?}: not a window inside it"
    );
    let (di, dj) = ((to_o[0] - fo[0]) as usize, (to_o[1] - fo[1]) as usize);
    let (fw, tw) = (from.width as usize * channels, to.width as usize * channels);
    let mut out = vec![0.0f32; to.len() * channels];
    out.par_chunks_mut(tw).enumerate().for_each(|(j, row)| {
        let start = (j + dj) * fw + di * channels;
        row.copy_from_slice(&data[start..start + tw]);
    });
    out
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

    /// The part of this colour map covered by `to` (see [`Grid::crop`]).
    pub fn crop(&self, to: GridSpec) -> ColorGrid {
        ColorGrid {
            spec: to,
            data: crop_samples(&self.data, 4, self.spec, to),
        }
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
            let fx = s.column_at(x_m);
            let fy = s.row_at(y_m);
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
