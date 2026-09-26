//! CPU erosion, using conservative neighbour fluxes and fixed-order gathers.
//! See `docs/erosion.md` for equations, units, boundary conditions and limitations.
mod hardness;
mod hydraulic;
mod thermal;

pub use hardness::RockHardness;
pub use hydraulic::Hydraulic;
pub use thermal::Thermal;

use rayon::prelude::*;
use std::sync::Arc;
use terrain_core::error::{CoreError, Result};
use terrain_core::{EvalContext, Grid, NodeSchema, Outputs, ParamDef, PortDef, PortType, Value};

/// Fixed order: west, east, north, south, then the diagonals north-west,
/// north-east, south-west, south-east. Missing neighbours refer to self; the
/// corresponding flux is always zero (closed boundaries).
#[inline]
fn neighbours(i: usize, width: usize, len: usize) -> [usize; 8] {
    let x = i % width;
    let (w, e) = (x > 0, x + 1 < width);
    let (n, s) = (i >= width, i + width < len);
    [
        if w { i - 1 } else { i },
        if e { i + 1 } else { i },
        if n { i - width } else { i },
        if s { i + width } else { i },
        if n && w { i - width - 1 } else { i },
        if n && e { i - width + 1 } else { i },
        if s && w { i + width - 1 } else { i },
        if s && e { i + width + 1 } else { i },
    ]
}
const OPPOSITE: [usize; 8] = [1, 0, 3, 2, 7, 6, 5, 4];

fn schema(
    id: &str,
    label: &str,
    description: &str,
    masks: &[(&str, &str)],
    params: Vec<ParamDef>,
) -> NodeSchema {
    let mut outputs = vec![PortDef::new("height", "Height", PortType::Heightfield)];
    outputs.extend(
        masks
            .iter()
            .map(|(key, label)| PortDef::new(key, label, PortType::Mask)),
    );
    NodeSchema {
        type_id: id.into(),
        type_version: 1,
        label: label.into(),
        category: "Simulate".into(),
        description: description.into(),
        gpu: false,
        inputs: vec![
            PortDef::new("in", "Terrain", PortType::Heightfield),
            PortDef::new("mask", "Strength", PortType::Mask).optional(),
            PortDef::new("hardness", "Rock hardness", PortType::Mask).optional(),
        ],
        outputs,
        params,
    }
}

fn duration() -> ParamDef {
    ParamDef::float("duration_s", "Duration", 60.0, 0.0, 600.0)
        .unit("s")
        .describe("Simulation time. Zero leaves the terrain unchanged.")
}

fn check_cancel(ctx: &EvalContext) -> Result<()> {
    if ctx.is_cancelled() {
        Err(CoreError::Cancelled)
    } else {
        Ok(())
    }
}

struct Domain<'a> {
    terrain: &'a Grid,
    mask: Option<&'a Grid>,
    hardness: Option<&'a Grid>,
    /// Distance to each of the [`neighbours`], metres.
    distance: [f32; 8],
    width: usize,
    len: usize,
}
impl<'a> Domain<'a> {
    fn new(ctx: &'a EvalContext) -> Result<Self> {
        check_cancel(ctx)?;
        let terrain = ctx.input_grid("in")?.as_ref();
        let mask = ctx.input("mask").map(|v| v.grid().as_ref());
        let hardness = ctx.input("hardness").map(|v| v.grid().as_ref());
        let fail = |message: &str| CoreError::NodeFailed {
            node: ctx.node_id.into(),
            message: message.into(),
        };
        if ctx.spec.width < 2
            || ctx.spec.height < 2
            || ctx.spec.width > 16384
            || ctx.spec.height > 16384
            || ctx.spec.origin_m.iter().any(|v| !v.is_finite())
            || ctx.spec.extent_m.iter().any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err(fail(
                "erosion needs a finite grid with at least two samples per axis",
            ));
        }
        for grid in [Some(terrain), mask, hardness].into_iter().flatten() {
            if grid.spec != ctx.spec || grid.data.len() != ctx.spec.len() {
                return Err(fail("erosion input grids must match the evaluation grid"));
            }
            if grid.data.par_iter().any(|v| !v.is_finite() || v.abs() > 1.0e8) {
                return Err(fail("erosion inputs must be finite and within +/-100 million"));
            }
        }
        let [dx, dy] = ctx.spec.cell_size_m().map(|v| v as f32);
        if !dx.is_finite() || !dy.is_finite() || dx < 0.01 || dy < 0.01 || dx > 1.0e8 || dy > 1.0e8 {
            return Err(fail(
                "erosion cell spacing must be between 0.01 and 100 million metres",
            ));
        }
        Ok(Self {
            terrain,
            mask,
            hardness,
            distance: {
                let diag = (dx * dx + dy * dy).sqrt();
                [dx, dx, dy, dy, diag, diag, diag, diag]
            },
            width: ctx.spec.width as usize,
            len: ctx.spec.len(),
        })
    }
    #[inline]
    fn strength(&self, i: usize) -> f32 {
        self.mask.map_or(1.0, |g| g.data[i].clamp(0.0, 1.0))
    }
    #[inline]
    fn softness(&self, i: usize) -> f32 {
        1.0 - self.hardness.map_or(0.0, |g| g.data[i].clamp(0.0, 1.0))
    }
}

/// Number and length of time steps no longer than `max_dt`.
fn time_steps(duration: f64, max_dt: f32, ctx: &EvalContext) -> Result<(usize, f32)> {
    let steps = (duration / max_dt as f64).ceil().max(1.0);
    if steps > 100_000.0 {
        return Err(CoreError::NodeFailed {
            node: ctx.node_id.into(),
            message: "erosion needs more than 100,000 steps; reduce duration or increase world extent".into(),
        });
    }
    Ok((steps as usize, (duration / steps) as f32))
}

/// Stable physical scale, not per-image min/max: output does not change contrast
/// when an unrelated region or resolution changes. Half saturation at `scale`.
fn mask_out(data: Vec<f32>, scale: f32, ctx: &EvalContext) -> Value {
    let grid = Grid { spec: ctx.spec, data };
    Value::Mask(Arc::new(grid.map(|v| v.max(0.0) / (scale + v.max(0.0)))))
}
fn height_out(data: Vec<f32>, ctx: &EvalContext) -> Outputs {
    Outputs::from([(
        "height".into(),
        Value::Heightfield(Arc::new(Grid { spec: ctx.spec, data })),
    )])
}
