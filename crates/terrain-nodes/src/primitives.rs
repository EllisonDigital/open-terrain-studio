//! Primitive shapes: Gradient, Cone, Hemisphere, Shape and File (import a
//! heightmap). Sizes are metres; positions are offsets from the world centre.

use std::collections::BTreeMap;
use std::path::Path;

use terrain_core::error::{CoreError, Result};
use terrain_core::import::{ImageValues, read_height_image};
use terrain_core::node::resolve_path;
use terrain_core::ops::smoothstep64;
use terrain_core::{
    EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, ParamValue, PortDef, PortType,
};

use crate::common::{angle_param, direction, height_params, heightfield_out, position, position_params};

fn primitive_schema(type_id: &str, label: &str, description: &str, params: Vec<ParamDef>) -> NodeSchema {
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: "Primitives".into(),
        description: description.into(),
        inputs: vec![],
        outputs: vec![PortDef::new("out", "Out", PortType::Heightfield)],
        params,
        gpu: false,
    }
}

/// `base + height × profile(x, y)`, with a drivable height.
fn profile_heightfield(ctx: &EvalContext, profile: impl Fn(f64, f64) -> f64 + Sync) -> Result<Outputs> {
    let height = ctx.field("height_m");
    let base = ctx.f64("base_m");
    heightfield_out(Grid::from_fn_indexed(ctx.spec, |idx, x, y| {
        (base + height.at(idx) as f64 * profile(x, y)) as f32
    }))
}

// ---- Gradient ---------------------------------------------------------------

/// A ramp: linear across the world, or radial from a point.
pub struct Gradient {
    schema: NodeSchema,
}

impl Default for Gradient {
    fn default() -> Self {
        let mut params = vec![
            ParamDef::choice(
                "mode",
                "Mode",
                "linear",
                &[("linear", "Linear"), ("radial", "Radial (bowl)")],
            ),
            angle_param(
                "angle_deg",
                "Direction",
                0.0,
                "Linear only: the direction the ramp rises towards. 0° = +X (right), 90° = +Y (down).",
            ),
            ParamDef::metres("length_m", "Length", 8192.0, 1.0, 1_000_000.0).describe(
                "Distance over which the ramp rises from Base to Base + Height (the radius, for Radial).",
            ),
            ParamDef::choice(
                "profile",
                "Profile",
                "linear",
                &[("linear", "Straight"), ("smooth", "Smooth (eased ends)")],
            ),
        ];
        params.extend(height_params(1000.0, 0.0));
        params.extend(position_params());
        Self {
            schema: primitive_schema(
                "primitive.gradient",
                "Gradient",
                "A ramp: a straight slope across the terrain, or a radial bowl around a point.",
                params,
            ),
        }
    }
}

impl NodeKind for Gradient {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let (dx, dy) = direction(ctx.f64("angle_deg"));
        let len = ctx.f64("length_m");
        let radial = ctx.choice("mode") == "radial";
        let smooth = ctx.choice("profile") == "smooth";
        profile_heightfield(ctx, |x, y| {
            let (px, py) = (x - cx, y - cy);
            let t = if radial {
                (px * px + py * py).sqrt() / len
            } else {
                (px * dx + py * dy) / len + 0.5
            };
            let t = t.clamp(0.0, 1.0);
            if smooth { smoothstep64(0.0, 1.0, t) } else { t }
        })
    }
}

// ---- Cone and Hemisphere ----------------------------------------------------

fn round_params(radius: f64, height: f64) -> Vec<ParamDef> {
    let mut params = vec![
        ParamDef::metres("radius_m", "Radius", radius, 1.0, 1_000_000.0)
            .describe("Radius of the base, in metres."),
    ];
    params.extend(height_params(height, 0.0));
    params.extend(position_params());
    params
}

/// A cone with straight sides.
pub struct Cone {
    schema: NodeSchema,
}

impl Default for Cone {
    fn default() -> Self {
        Self {
            schema: primitive_schema(
                "primitive.cone",
                "Cone",
                "A cone with straight sides, like a simple volcano.",
                round_params(2500.0, 1500.0),
            ),
        }
    }
}

impl NodeKind for Cone {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let r = ctx.f64("radius_m");
        profile_heightfield(ctx, |x, y| {
            let d = ((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt();
            (1.0 - d / r).max(0.0)
        })
    }
}

/// A dome (half an ellipsoid).
pub struct Hemisphere {
    schema: NodeSchema,
}

impl Default for Hemisphere {
    fn default() -> Self {
        Self {
            schema: primitive_schema(
                "primitive.hemisphere",
                "Hemisphere",
                "A rounded dome. With Height equal to Radius it is a true half sphere.",
                round_params(2500.0, 1500.0),
            ),
        }
    }
}

impl NodeKind for Hemisphere {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let r = ctx.f64("radius_m");
        profile_heightfield(ctx, |x, y| {
            let t = ((x - cx) * (x - cx) + (y - cy) * (y - cy)) / (r * r);
            (1.0 - t).max(0.0).sqrt()
        })
    }
}

// ---- Shape ------------------------------------------------------------------

/// Signed distance to a shape of "radius" `r` centred at the origin
/// (negative inside), after Quilez's 2D distance functions.
fn shape_sdf(shape: &str, px: f64, py: f64, r: f64) -> f64 {
    const SQRT3: f64 = 1.732_050_807_568_877_2;
    match shape {
        "square" => {
            let (dx, dy) = (px.abs() - r, py.abs() - r);
            let outside = (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt();
            outside + dx.max(dy).min(0.0)
        }
        "diamond" => (px.abs() + py.abs() - r) * std::f64::consts::FRAC_1_SQRT_2,
        "hexagon" => {
            // Flat-topped hexagon with inradius r.
            let (kx, ky, kz) = (-SQRT3 * 0.5, 0.5, 1.0 / SQRT3);
            let (mut x, mut y) = (px.abs(), py.abs());
            let d = 2.0 * (kx * x + ky * y).min(0.0);
            x -= d * kx;
            y -= d * ky;
            let cx = x.clamp(-kz * r, kz * r);
            let (x, y) = (x - cx, y - r);
            (x * x + y * y).sqrt() * y.signum()
        }
        "triangle" => {
            // Equilateral triangle, point up (towards -Y), inradius r / 2.
            let k = SQRT3;
            let (mut x, mut y) = (px.abs() - r, -py + r / k);
            if x + k * y > 0.0 {
                (x, y) = ((x - k * y) * 0.5, (-k * x - y) * 0.5);
            }
            x -= x.clamp(-2.0 * r, 0.0);
            -(x * x + y * y).sqrt() * y.signum()
        }
        _ => (px * px + py * py).sqrt() - r,
    }
}

/// A flat-topped shape with a soft edge.
pub struct Shape {
    schema: NodeSchema,
}

impl Default for Shape {
    fn default() -> Self {
        let mut params = vec![
            ParamDef::choice(
                "shape",
                "Shape",
                "circle",
                &[
                    ("circle", "Circle"),
                    ("square", "Square"),
                    ("diamond", "Diamond"),
                    ("hexagon", "Hexagon"),
                    ("triangle", "Triangle"),
                ],
            ),
            ParamDef::metres("size_m", "Size", 2500.0, 1.0, 1_000_000.0)
                .describe("Distance from the centre to the edge, in metres."),
            ParamDef::metres("falloff_m", "Falloff", 800.0, 0.0, 100_000.0)
                .describe("Width of the soft edge outside the shape, in metres. 0 = a hard edge."),
            ParamDef::float("rotation_deg", "Rotation", 0.0, -360.0, 360.0).unit("°"),
        ];
        params.extend(height_params(1000.0, 0.0));
        params.extend(position_params());
        Self {
            schema: primitive_schema(
                "primitive.shape",
                "Shape",
                "A flat-topped circle, square, diamond, hexagon or triangle with a soft edge. \
                 Useful as a mask or a plateau.",
                params,
            ),
        }
    }
}

impl NodeKind for Shape {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (cx, cy) = position(ctx);
        let size = ctx.f64("size_m");
        let falloff = ctx.f64("falloff_m");
        let shape = ctx.choice("shape");
        let (c, s) = direction(-ctx.f64("rotation_deg"));
        profile_heightfield(ctx, |x, y| {
            let (px, py) = (x - cx, y - cy);
            let (rx, ry) = (px * c - py * s, px * s + py * c);
            let d = shape_sdf(&shape, rx, ry, size);
            if falloff <= 0.0 {
                if d <= 0.0 { 1.0 } else { 0.0 }
            } else {
                1.0 - smoothstep64(0.0, falloff, d)
            }
        })
    }
}

// ---- File -------------------------------------------------------------------

/// Import a heightmap image, stretched over the whole world.
pub struct File {
    schema: NodeSchema,
}

impl Default for File {
    fn default() -> Self {
        Self {
            schema: primitive_schema(
                "primitive.file",
                "File",
                "Import a heightmap image (EXR or 8/16-bit PNG), stretched over the whole world. \
                 Pixel (0, 0) is the world origin, as in exported files.",
                vec![
                    ParamDef::file(
                        "path",
                        "File",
                        &["*.exr ; OpenEXR heightmap", "*.png ; PNG heightmap"],
                    )
                    .describe("Relative paths are resolved from the project file's folder."),
                    ParamDef::choice(
                        "values",
                        "Values",
                        "auto",
                        &[
                            ("auto", "Auto (EXR = metres, PNG = 0..1)"),
                            ("metres", "Metres"),
                            ("normalised", "0..1 (mapped to Low..High)"),
                        ],
                    )
                    .describe("What the image's values mean."),
                    ParamDef::bool("world_range", "0..1 = world height range", true).describe(
                        "For 0..1 values: map black and white to the world's lowest and highest heights.",
                    ),
                    ParamDef::metres("low_m", "Low", 0.0, -10_000.0, 20_000.0)
                        .describe("For 0..1 values: height of black, when not using the world range."),
                    ParamDef::metres("high_m", "High", 1000.0, -10_000.0, 20_000.0)
                        .describe("For 0..1 values: height of white, when not using the world range."),
                    ParamDef::bool("flip_y", "Flip vertically", false)
                        .describe("For images whose first row is the far (Y = max) edge of the terrain."),
                ],
            ),
        }
    }
}

impl NodeKind for File {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }

    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let path = ctx
            .path("path")
            .ok_or_else(|| CoreError::Image("no file chosen: pick a heightmap in the File setting".into()))?;
        let img = read_height_image(&path)?;
        let metres = match ctx.choice("values").as_str() {
            "metres" => true,
            "normalised" => false,
            _ => img.values == ImageValues::Metres,
        };
        let (lo, hi) = if ctx.bool("world_range") {
            (ctx.world.height_range_m[0], ctx.world.height_range_m[1])
        } else {
            (ctx.f32("low_m"), ctx.f32("high_m"))
        };
        let flip = ctx.bool("flip_y");
        let (sx, sy) = (ctx.world.size_m[0], ctx.world.size_m[1]);
        let (w1, h1) = ((img.width - 1) as f64, (img.height - 1) as f64);
        heightfield_out(Grid::from_fn(ctx.spec, |x, y| {
            let px = x / sx * w1;
            let py = if flip { (1.0 - y / sy) * h1 } else { y / sy * h1 };
            let v = img.sample(px, py);
            if metres { v } else { lo + v * (hi - lo) }
        }))
    }

    /// Re-read the file when it changes on disk.
    fn cache_salt(&self, params: &BTreeMap<String, ParamValue>, base_dir: Option<&Path>) -> String {
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        let Some(p) = resolve_path(path, base_dir) else {
            return String::new();
        };
        match std::fs::metadata(&p) {
            Ok(m) => {
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                format!("{}|{}|{modified}", p.display(), m.len())
            }
            Err(_) => format!("{}|missing", p.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::shape_sdf;

    #[test]
    fn shape_distances() {
        for shape in ["circle", "square", "diamond", "hexagon", "triangle"] {
            assert!(
                shape_sdf(shape, 0.0, 0.0, 100.0) < 0.0,
                "{shape} centre is inside"
            );
            assert!(
                shape_sdf(shape, 1000.0, 1000.0, 100.0) > 0.0,
                "{shape} far is outside"
            );
        }
        assert!((shape_sdf("circle", 150.0, 0.0, 100.0) - 50.0).abs() < 1e-9);
        assert!((shape_sdf("square", 150.0, 0.0, 100.0) - 50.0).abs() < 1e-9);
        assert!((shape_sdf("square", 0.0, -130.0, 100.0) - 30.0).abs() < 1e-9);
        assert!(
            shape_sdf("hexagon", 0.0, 99.0, 100.0) < 0.0 && shape_sdf("hexagon", 0.0, 101.0, 100.0) > 0.0
        );
    }
}
