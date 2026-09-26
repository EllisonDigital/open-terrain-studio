//! Drainage shared by erosion and water nodes: Priority-Flood routing,
//! depression filling and the fixed simulation grid that keeps river-scale
//! results the same at every resolution. See `docs/water.md`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rayon::prelude::*;
use terrain_core::{Grid, GridSpec};

/// Flow masks: drained area (m²) on a log scale, black to white.
pub(crate) const FLOW_LO_M2: f64 = 5.0e3;
pub(crate) const FLOW_HI_M2: f64 = 5.0e7;

/// Routing gradient added across flats and filled pits (m per m).
pub(crate) const EPSILON_SLOPE: f64 = 1.0e-5;

/// Neighbour offsets (dx, dy) in a fixed order: 4 edges, then 4 diagonals.
pub(crate) const D8: [(i64, i64); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

/// An f64 as an order-preserving integer, for priority queues.
#[inline]
pub(crate) fn order_key(v: f64) -> u64 {
    let b = v.to_bits();
    if b >> 63 == 1 { !b } else { b | (1 << 63) }
}

/// A fixed pseudo-random value in 0..1 for a cell.
#[inline]
pub(crate) fn unit_hash(i: u64) -> f64 {
    (terrain_core::seed::mix64(i ^ 0x9e37_79b9_7f4a_7c15) >> 11) as f64 / (1u64 << 53) as f64
}

/// Kinds of link from a cell to its receiver, indexing [`Routing::lengths`]
/// and the per-step tables built from them.
pub(crate) const LINK_X: u8 = 0;
pub(crate) const LINK_Y: u8 = 1;
pub(crate) const LINK_DIAGONAL: u8 = 2;
pub(crate) const LINK_OUTLET: u8 = 3;

/// Flow routing for one surface.
pub(crate) struct Routing {
    /// Downstream neighbour of each cell (itself for outlets).
    pub receiver: Vec<usize>,
    /// Kind of link to the receiver (`LINK_*`).
    pub link: Vec<u8>,
    /// Length of each kind of link, metres (1 for outlets).
    pub lengths: [f64; 4],
    /// Cells ordered so every receiver comes before its donors; filled
    /// heights never fall along it.
    pub stack: Vec<usize>,
    /// Surface with pits filled, plus a tiny ε gradient (routing only).
    pub filled: Vec<f64>,
}

impl Routing {
    /// Distance from cell `i` to its receiver, metres.
    #[inline]
    pub fn distance(&self, i: usize) -> f64 {
        self.lengths[self.link[i] as usize]
    }

    /// `f(length)` for every kind of link, so per-cell work can look it up.
    pub fn per_link(&self, f: impl Fn(f64) -> f64) -> [f64; 4] {
        self.lengths.map(f)
    }
}

/// The fixed random factor `0.25 + 1.5 × unit_hash(cell)` that makes ε
/// routing wander across flats (the same for every step).
pub(crate) fn flat_jitter(n: usize) -> Vec<f64> {
    (0..n)
        .into_par_iter()
        .map(|j| 0.25 + 1.5 * unit_hash(j as u64))
        .collect()
}

#[inline]
fn is_edge(i: usize, w: usize, ht: usize) -> bool {
    let (x, y) = (i % w, i / w);
    x == 0 || y == 0 || x + 1 == w || y + 1 == ht
}

/// D8 steps in metres: x, x, y, y, then diagonals.
#[inline]
pub(crate) fn d8_steps(dx: f64, dy: f64) -> [f64; 8] {
    let diag = (dx * dx + dy * dy).sqrt();
    [dx, dx, dy, dy, diag, diag, diag, diag]
}

/// Priority-Flood+ε (Barnes, Lehman & Mulla 2014) from the world's edges,
/// then steepest-descent (D8, O'Callaghan & Mark 1984) receivers on the
/// filled surface.
pub(crate) fn route(h: &[f64], jitter: &[f64], w: usize, ht: usize, dx: f64, dy: f64) -> Routing {
    let n = h.len();
    let step = d8_steps(dx, dy);
    let diag = step[4];
    // Priority-Flood+ε pops cells in order of (filled height, index): every
    // cell it pushes is above the one just popped, so that order only rises.
    //
    // Most cells end up unraised (filled = own height). Such a cell is always
    // discovered before its turn (by a lower neighbour), so these cells pop
    // exactly in order of (height, index): sort them once, in parallel, and
    // walk that list. Only raised cells (pits and flats) go through a heap.
    // A cell still undiscovered when its turn in the list comes will be
    // raised, and joins the heap when discovered. Merging the two gives the
    // same pop order as one heap of every cell.
    let key = |height: f64, cell: usize| ((order_key(height) as u128) << 64) | cell as u128;
    let mut by_height: Vec<u128> = (0..n).into_par_iter().map(|i| key(h[i], i)).collect();
    by_height.par_sort_unstable();
    let mut filled = vec![f64::NAN; n];
    let mut stack = Vec::with_capacity(n);
    let mut raised: BinaryHeap<Reverse<u128>> = BinaryHeap::new();
    for i in (0..n).filter(|&i| is_edge(i, w, ht)) {
        filled[i] = h[i];
    }
    let mut next = 0;
    loop {
        let top = raised.peek().map(|r| r.0);
        let c = match by_height.get(next) {
            Some(&k) if top.is_none_or(|t| k < t) => {
                next += 1;
                let c = k as u64 as usize;
                // Undiscovered (will be raised) or raised (in the heap): not now.
                if filled[c] != h[c] {
                    continue;
                }
                c
            }
            _ => match raised.pop() {
                Some(Reverse(k)) => k as u64 as usize,
                None => break,
            },
        };
        stack.push(c);
        let (cx, cy) = ((c % w) as i64, (c / w) as i64);
        for (k, (ox, oy)) in D8.iter().enumerate() {
            let (x, y) = (cx + ox, cy + oy);
            if x < 0 || y < 0 || x >= w as i64 || y >= ht as i64 {
                continue;
            }
            let j = y as usize * w + x as usize;
            if !filled[j].is_nan() {
                continue;
            }
            // Jittered ε: across flats and filled pits a plain ε makes flow
            // run in straight lines; a per-cell random factor lets it wander.
            filled[j] = h[j].max(filled[c] + EPSILON_SLOPE * step[k] * jitter[j]);
            if filled[j] != h[j] {
                raised.push(Reverse(key(filled[j], j)));
            }
        }
    }
    // Steepest descent on the filled surface (always downhill, thanks to ε).
    let kind = [
        LINK_X,
        LINK_X,
        LINK_Y,
        LINK_Y,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
        LINK_DIAGONAL,
    ];
    let (receiver, link): (Vec<usize>, Vec<u8>) = (0..n)
        .into_par_iter()
        .map(|i| {
            if is_edge(i, w, ht) {
                return (i, LINK_OUTLET);
            }
            let (cx, cy) = ((i % w) as i64, (i / w) as i64);
            let mut best = (i, LINK_OUTLET, 0.0);
            for (k, (ox, oy)) in D8.iter().enumerate() {
                let j = (cy + oy) as usize * w + (cx + ox) as usize;
                let s = (filled[i] - filled[j]) / step[k];
                if s > best.2 {
                    best = (j, kind[k], s);
                }
            }
            (best.0, best.1)
        })
        .unzip();
    Routing {
        receiver,
        link,
        lengths: [dx, dy, diag, 1.0],
        stack,
        filled,
    }
}

/// Plain Priority-Flood (no ε): every depression filled flat to the level
/// where it would spill, draining to the world's edges. `h` values of NaN are
/// not allowed.
pub(crate) fn fill_depressions(h: &[f64], w: usize, ht: usize) -> Vec<f64> {
    let n = h.len();
    let key = |height: f64, cell: usize| ((order_key(height) as u128) << 64) | cell as u128;
    let mut filled = vec![f64::NAN; n];
    let mut open: BinaryHeap<Reverse<u128>> = BinaryHeap::new();
    for i in (0..n).filter(|&i| is_edge(i, w, ht)) {
        filled[i] = h[i];
        open.push(Reverse(key(h[i], i)));
    }
    while let Some(Reverse(k)) = open.pop() {
        let c = k as u64 as usize;
        let (cx, cy) = ((c % w) as i64, (c / w) as i64);
        for (ox, oy) in D8 {
            let (x, y) = (cx + ox, cy + oy);
            if x < 0 || y < 0 || x >= w as i64 || y >= ht as i64 {
                continue;
            }
            let j = y as usize * w + x as usize;
            if !filled[j].is_nan() {
                continue;
            }
            filled[j] = h[j].max(filled[c]);
            open.push(Reverse(key(filled[j], j)));
        }
    }
    filled
}

/// Drained area of every cell (m²), each cell passing its own area and
/// everything it receives to its receiver.
pub(crate) fn accumulate_d8(r: &Routing, cell_area: f64) -> Vec<f64> {
    let mut a = vec![cell_area; r.receiver.len()];
    for &i in r.stack.iter().rev() {
        let j = r.receiver[i];
        if j != i {
            a[j] += a[i];
        }
    }
    a
}

/// Map a positive quantity to 0..1 on a log scale between `lo` and `hi`.
#[inline]
pub(crate) fn log_mask(v: f64, lo: f64, hi: f64) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    (libm::log10(v.max(lo) / lo) / libm::log10(hi / lo)).min(1.0) as f32
}

/// The simulation grid: the canonical grid with `detail_m` cells over the
/// region, so every resolution finer than twice the detail size computes
/// exactly the same drainage. Much coarser grids (quick previews) compute on
/// themselves.
pub(crate) fn simulation_spec(spec: GridSpec, detail_m: f64) -> GridSpec {
    let cell = spec.cell_size_m();
    if cell[0] >= 2.0 * detail_m && cell[1] >= 2.0 * detail_m {
        return spec;
    }
    let samples =
        |extent: f64| ((extent / detail_m).round() as u32 + 1).clamp(2, terrain_core::grid::MAX_RESOLUTION);
    let canonical = GridSpec::new(
        samples(spec.extent_m[0]),
        samples(spec.extent_m[1]),
        spec.origin_m,
        spec.extent_m,
    );
    if canonical.width == spec.width && canonical.height == spec.height {
        spec
    } else {
        canonical
    }
}

/// Resample onto `to` after a low-pass filter of half a `to` cell (in
/// metres), whatever the source resolution, so every resolution feeds the
/// simulation the same band-limited terrain.
pub(crate) fn resample(grid: &Grid, to: GridSpec) -> Grid {
    let smooth = terrain_core::ops::gaussian_blur(grid, 0.5 * to.cell_size_m()[0]);
    Grid::from_fn(to, |x, y| smooth.sample_bilinear_m(x, y))
}

/// Value of the nearest sample of `grid` to world position (x, y), for
/// categories and angles that must not be blended.
#[inline]
pub(crate) fn sample_nearest_m(grid: &Grid, x: f64, y: f64) -> f32 {
    let s = grid.spec;
    let [cx, cy] = s.cell_size_m();
    let i = libm::round((x - s.origin_m[0]) / cx).clamp(0.0, (s.width - 1) as f64) as usize;
    let j = libm::round((y - s.origin_m[1]) / cy).clamp(0.0, (s.height - 1) as f64) as usize;
    grid.data[j * s.width as usize + i]
}

/// Resampled 0..1 masks: snap values within rounding of 0 or 1, so a
/// uniform "fully hard" or "fully protected" map stays exactly that.
#[inline]
pub(crate) fn snap01(v: f32) -> f32 {
    if v < 1.0e-6 {
        0.0
    } else if v > 1.0 - 1.0e-6 {
        1.0
    } else {
        v
    }
}
