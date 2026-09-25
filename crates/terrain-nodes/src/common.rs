//! Helpers shared by node implementations.

use std::sync::Arc;

use terrain_core::error::Result;
use terrain_core::{EvalContext, Grid, GridSpec, Outputs, ParamDef, Params, Value};

/// A single `out` Heightfield output.
pub fn heightfield_out(grid: Grid) -> Result<Outputs> {
    Ok(Outputs::from([(
        "out".to_string(),
        Value::Heightfield(Arc::new(grid)),
    )]))
}

/// A single `out` Mask output (values clamped to 0..1).
pub fn mask_out(mut grid: Grid) -> Result<Outputs> {
    for v in &mut grid.data {
        *v = v.clamp(0.0, 1.0);
    }
    Ok(Outputs::from([("out".to_string(), Value::Mask(Arc::new(grid)))]))
}

/// "Position X/Y" parameters: an offset from the world centre, in metres.
pub fn position_params() -> Vec<ParamDef> {
    vec![
        ParamDef::metres("pos_x_m", "Position X", 0.0, -1_000_000.0, 1_000_000.0)
            .describe("East/west offset from the centre of the world, in metres."),
        ParamDef::metres("pos_y_m", "Position Y", 0.0, -1_000_000.0, 1_000_000.0)
            .describe("North/south offset from the centre of the world, in metres."),
    ]
}

/// World position (metres) set by [`position_params`].
pub fn position(ctx: &EvalContext) -> (f64, f64) {
    let c = ctx.world.centre();
    (c[0] + ctx.f64("pos_x_m"), c[1] + ctx.f64("pos_y_m"))
}

/// A direction parameter in degrees. 0° points along +X (right in the 2D
/// view and exported images), 90° along +Y (down).
pub fn angle_param(key: &str, label: &str, default: f64, description: &str) -> ParamDef {
    ParamDef::float(key, label, default, 0.0, 360.0)
        .unit("°")
        .describe(description)
}

/// Unit vector for an angle in degrees (see [`angle_param`]).
pub fn direction(deg: f64) -> (f64, f64) {
    let r = deg.to_radians();
    (libm::cos(r), libm::sin(r))
}

/// Seed parameter.
pub fn seed_param() -> ParamDef {
    ParamDef::int("seed", "Seed", 0, 0, 999_999).describe("Change for a different variation.")
}

/// Heights in metres: "Height" (drivable) and "Base".
pub fn height_params(height: f64, base: f64) -> Vec<ParamDef> {
    vec![
        ParamDef::metres("height_m", "Height", height, 0.0, 20_000.0)
            .describe("Height of the feature above its base, in metres.")
            .drivable(),
        ParamDef::metres("base_m", "Base", base, -10_000.0, 10_000.0)
            .describe("Height of the surrounding ground, in metres."),
    ]
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A "Strength" parameter (0..1, drivable): how much of the effect to apply.
pub fn strength_param() -> ParamDef {
    ParamDef::float("strength", "Strength", 1.0, 0.0, 1.0)
        .describe("How much of the effect to apply. 0 = input unchanged.")
        .drivable()
}

/// Kernel parameters `f[12..16]`: grid origin and cell size in metres, so a
/// kernel can compute world positions (`origin + cell × index`).
pub fn grid_positions(p: Params, spec: GridSpec) -> Params {
    let cell = spec.cell_size_m();
    p.f(12, spec.origin_m[0] as f32)
        .f(13, cell[0] as f32)
        .f(14, spec.origin_m[1] as f32)
        .f(15, cell[1] as f32)
}
