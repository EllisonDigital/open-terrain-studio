//! Colour and texturing nodes: colour maps from gradients and images,
//! blending and layering, normal maps, occlusion and splat (weight) maps.
//! Colours are sRGB-encoded 0..1 with alpha; see `docs/colour.md`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use terrain_core::error::{CoreError, Result};
use terrain_core::import::read_color_image;
use terrain_core::ops::{gaussian_blur, gradient};
use terrain_core::{
    ColorGrid, EvalContext, Gradient, Grid, NodeKind, NodeSchema, Outputs, ParamDef, ParamValue, PortDef,
    PortType, Value,
};

fn color_out(key: &str, color: ColorGrid) -> (String, Value) {
    (key.into(), Value::ColorMap(Arc::new(color)))
}

fn schema(
    type_id: &str,
    label: &str,
    category: &str,
    description: &str,
    inputs: Vec<PortDef>,
    outputs: Vec<PortDef>,
    params: Vec<ParamDef>,
) -> NodeSchema {
    NodeSchema {
        type_id: type_id.into(),
        type_version: 1,
        label: label.into(),
        category: category.into(),
        description: description.into(),
        inputs,
        outputs,
        params,
        gpu: false,
    }
}

// ---- Gradients ----------------------------------------------------------------

/// Built-in natural gradients: `(key, label, stops [t, r, g, b])`, sRGB.
/// Authored for this project from typical ground colours.
pub const GRADIENTS: &[(&str, &str, &[[f64; 4]])] = &[
    (
        "terrain",
        "Terrain (lowland to snow)",
        &[
            [0.00, 0.20, 0.27, 0.13],
            [0.30, 0.36, 0.40, 0.20],
            [0.55, 0.47, 0.41, 0.31],
            [0.78, 0.55, 0.53, 0.50],
            [0.90, 0.88, 0.89, 0.91],
            [1.00, 0.97, 0.98, 1.00],
        ],
    ),
    (
        "grass",
        "Grass",
        &[
            [0.00, 0.13, 0.22, 0.08],
            [0.50, 0.28, 0.40, 0.14],
            [1.00, 0.55, 0.58, 0.28],
        ],
    ),
    (
        "rock",
        "Rock",
        &[
            [0.00, 0.22, 0.20, 0.18],
            [0.50, 0.42, 0.39, 0.35],
            [1.00, 0.66, 0.64, 0.60],
        ],
    ),
    (
        "sand",
        "Sand",
        &[
            [0.00, 0.55, 0.45, 0.31],
            [0.50, 0.76, 0.66, 0.48],
            [1.00, 0.91, 0.85, 0.70],
        ],
    ),
    (
        "snow",
        "Snow",
        &[
            [0.00, 0.62, 0.68, 0.78],
            [0.60, 0.88, 0.90, 0.94],
            [1.00, 0.98, 0.99, 1.00],
        ],
    ),
    (
        "desert",
        "Desert",
        &[
            [0.00, 0.42, 0.22, 0.13],
            [0.40, 0.70, 0.40, 0.22],
            [0.75, 0.84, 0.60, 0.38],
            [1.00, 0.93, 0.80, 0.62],
        ],
    ),
    (
        "water",
        "Water depth",
        &[
            [0.00, 0.02, 0.10, 0.20],
            [0.60, 0.08, 0.30, 0.40],
            [1.00, 0.35, 0.60, 0.58],
        ],
    ),
];

fn gradient_params() -> Vec<ParamDef> {
    let mut options = vec![("custom", "Custom (below)")];
    options.extend(GRADIENTS.iter().map(|(k, l, _)| (*k, *l)));
    vec![
        ParamDef::choice("preset", "Gradient", "terrain", &options)
            .describe("A built-in natural gradient, or Custom to use your own below."),
        ParamDef::gradient("gradient", "Custom gradient", GRADIENTS[0].2)
            .describe("Colours from black (left) to white (right) of the input, when Gradient is Custom."),
    ]
}

/// The gradient chosen by `preset` / `gradient`.
fn chosen_gradient(ctx: &EvalContext) -> Gradient {
    let preset = ctx.choice("preset");
    match GRADIENTS.iter().find(|(k, _, _)| *k == preset) {
        Some((_, _, stops)) => Gradient::from_stops(stops.to_vec()),
        None => ctx.gradient("gradient"),
    }
}

// ---- Colourise ----------------------------------------------------------------

/// Colour a height or mask through a gradient.
pub struct Colourise {
    schema: NodeSchema,
}

impl Default for Colourise {
    fn default() -> Self {
        let mut params = gradient_params();
        params.extend([
            ParamDef::float("low", "Input low", 0.0, 0.0, 1.0).describe(
                "Input value (0..1; heights over the world height range) at the gradient's left end.",
            ),
            ParamDef::float("high", "Input high", 1.0, 0.0, 1.0)
                .describe("Input value at the gradient's right end."),
        ]);
        Self {
            schema: schema(
                "colour.colourise",
                "Colourise",
                "Colour",
                "Colours a mask or height through a gradient: black (or the lowest height) takes the \
                 left end's colour, white the right end's. Use it on Slope, Aspect or Height Mask \
                 for colour by angle, facing or altitude.",
                vec![PortDef::new("in", "In", PortType::Mask)],
                vec![PortDef::new("out", "Colour", PortType::ColorMap)],
                params,
            ),
        }
    }
}

impl NodeKind for Colourise {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let input = ctx.input_grid("in")?;
        let g = chosen_gradient(ctx);
        let (lo, hi) = (ctx.f64("low"), ctx.f64("high"));
        let span = hi - lo;
        let color = ColorGrid::from_fn_indexed(ctx.spec, |i, _, _| {
            let v = input.data[i] as f64;
            let t = if span.abs() > 1.0e-9 { (v - lo) / span } else { v };
            let [r, g, b] = g.eval(t);
            [r, g, b, 1.0]
        });
        Ok(Outputs::from([color_out("out", color)]))
    }
}

// ---- Blend ------------------------------------------------------------------

/// Combine two colour maps.
pub struct Blend {
    schema: NodeSchema,
}

impl Default for Blend {
    fn default() -> Self {
        Self {
            schema: schema(
                "colour.blend",
                "Blend Colours",
                "Colour",
                "Puts Top over Bottom where Mask is white (everywhere without a mask), with the chosen \
                 blend mode and opacity.",
                vec![
                    PortDef::new("a", "Bottom", PortType::ColorMap),
                    PortDef::new("b", "Top", PortType::ColorMap),
                    PortDef::new("mask", "Mask", PortType::Mask).optional(),
                ],
                vec![PortDef::new("out", "Colour", PortType::ColorMap)],
                vec![
                    ParamDef::choice(
                        "mode",
                        "Mode",
                        "normal",
                        &[
                            ("normal", "Normal"),
                            ("multiply", "Multiply"),
                            ("screen", "Screen"),
                            ("overlay", "Overlay"),
                        ],
                    ),
                    ParamDef::float("opacity", "Opacity", 1.0, 0.0, 1.0)
                        .describe("How much of Top to apply.")
                        .drivable(),
                ],
            ),
        }
    }
}

fn blend_channel(mode: &str, a: f32, b: f32) -> f32 {
    match mode {
        "multiply" => a * b,
        "screen" => 1.0 - (1.0 - a) * (1.0 - b),
        "overlay" => {
            if a < 0.5 {
                2.0 * a * b
            } else {
                1.0 - 2.0 * (1.0 - a) * (1.0 - b)
            }
        }
        _ => b,
    }
}

impl NodeKind for Blend {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let (a, b) = (ctx.input_color("a")?, ctx.input_color("b")?);
        let mask = ctx.input("mask").map(|v| v.grid().clone());
        let opacity = ctx.field("opacity");
        let mode = ctx.choice("mode");
        let color = ColorGrid::from_fn_indexed(ctx.spec, |i, _, _| {
            let (ca, cb) = (a.at(i), b.at(i));
            let m = mask.as_ref().map_or(1.0, |g| g.data[i].clamp(0.0, 1.0));
            let k = (m * opacity.at(i) * cb[3]).clamp(0.0, 1.0);
            let mut out = [0.0; 4];
            for c in 0..3 {
                let top = blend_channel(&mode, ca[c], cb[c]);
                out[c] = ca[c] + (top - ca[c]) * k;
            }
            out[3] = ca[3] + (1.0 - ca[3]) * k;
            out
        });
        Ok(Outputs::from([color_out("out", color)]))
    }
}

// ---- Layers -----------------------------------------------------------------

const LAYERS: usize = 4;

/// Stack up to four colour layers over a base, each where its mask is white.
pub struct Layers {
    schema: NodeSchema,
}

impl Default for Layers {
    fn default() -> Self {
        let mut inputs = vec![PortDef::new("base", "Base", PortType::ColorMap)];
        for k in 1..=LAYERS {
            inputs.push(
                PortDef::new(&format!("color_{k}"), &format!("Layer {k}"), PortType::ColorMap).optional(),
            );
            inputs.push(PortDef::new(&format!("mask_{k}"), &format!("Mask {k}"), PortType::Mask).optional());
        }
        Self {
            schema: schema(
                "colour.layers",
                "Colour Layers",
                "Colour",
                "Quick material layering: each connected layer is painted over the ones below it where its \
                 mask is white (everywhere without a mask), in order 1 to 4.",
                inputs,
                vec![PortDef::new("out", "Colour", PortType::ColorMap)],
                vec![],
            ),
        }
    }
}

impl NodeKind for Layers {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let base = ctx.input_color("base")?;
        let layers: Vec<(Arc<ColorGrid>, Option<Arc<Grid>>)> = (1..=LAYERS)
            .filter_map(|k| {
                let color = ctx.input(&format!("color_{k}"))?.color()?.clone();
                let mask = ctx.input(&format!("mask_{k}")).map(|v| v.grid().clone());
                Some((color, mask))
            })
            .collect();
        let color = ColorGrid::from_fn_indexed(ctx.spec, |i, _, _| {
            let mut out = base.at(i);
            for (layer, mask) in &layers {
                let c = layer.at(i);
                let k = (mask.as_ref().map_or(1.0, |m| m.data[i].clamp(0.0, 1.0)) * c[3]).clamp(0.0, 1.0);
                for ch in 0..3 {
                    out[ch] += (c[ch] - out[ch]) * k;
                }
                out[3] += (1.0 - out[3]) * k;
            }
            out
        });
        Ok(Outputs::from([color_out("out", color)]))
    }
}

// ---- Image ------------------------------------------------------------------

/// Import a colour image (e.g. satellite imagery), stretched over the world.
pub struct Image {
    schema: NodeSchema,
}

impl Default for Image {
    fn default() -> Self {
        Self {
            schema: schema(
                "colour.image",
                "Colour Image",
                "Colour",
                "Imports a colour image such as satellite or aerial imagery (PNG, JPEG or EXR), stretched \
                 over the whole world. Pixel (0, 0) is the world origin, as in exported files.",
                vec![],
                vec![PortDef::new("out", "Colour", PortType::ColorMap)],
                vec![
                    ParamDef::file(
                        "path",
                        "File",
                        &[
                            "*.png ; PNG image",
                            "*.jpg, *.jpeg ; JPEG image",
                            "*.exr ; OpenEXR image",
                        ],
                    )
                    .describe("Relative paths are resolved from the project file's folder."),
                    ParamDef::bool("flip_y", "Flip vertically", false)
                        .describe("For images whose first row is the far (Y = max) edge of the terrain."),
                    ParamDef::float("brightness", "Brightness", 1.0, 0.0, 4.0)
                        .describe("Multiplies every colour, e.g. to match imagery to the rest of the map."),
                    ParamDef::float("saturation", "Saturation", 1.0, 0.0, 4.0)
                        .describe("0 = grey, 1 = as imported, more = more colourful."),
                ],
            ),
        }
    }
}

impl NodeKind for Image {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let path = ctx
            .path("path")
            .ok_or_else(|| CoreError::Image("no file chosen: pick an image in the File setting".into()))?;
        let img = read_color_image(&path)?;
        let flip = ctx.bool("flip_y");
        let (bright, sat) = (ctx.f32("brightness"), ctx.f32("saturation"));
        let (sx, sy) = (ctx.world.size_m[0], ctx.world.size_m[1]);
        let (w1, h1) = ((img.width - 1) as f64, (img.height - 1) as f64);
        let color = ColorGrid::from_fn_indexed(ctx.spec, |_, x, y| {
            let px = x / sx * w1;
            let py = if flip { (1.0 - y / sy) * h1 } else { y / sy * h1 };
            let [r, g, b, a] = img.sample(px, py);
            let grey = (r + g + b) / 3.0;
            let adjust = |v: f32| ((grey + (v - grey) * sat) * bright).clamp(0.0, 1.0);
            [adjust(r), adjust(g), adjust(b), a.clamp(0.0, 1.0)]
        });
        Ok(Outputs::from([color_out("out", color)]))
    }
    fn cache_salt(&self, params: &BTreeMap<String, ParamValue>, base_dir: Option<&Path>) -> String {
        crate::common::file_salt(params, "path", base_dir)
    }
}

// ---- Normal map ---------------------------------------------------------------

/// Tangent-space normal map of a heightfield.
pub struct NormalMap {
    schema: NodeSchema,
}

impl Default for NormalMap {
    fn default() -> Self {
        Self {
            schema: schema(
                "output.normal_map",
                "Normal Map",
                "Output",
                "A normal map of the terrain for texturing: red = east (+X), green = up the image \
                 (OpenGL: Godot, Blender, Unity) or down (DirectX: Unreal), blue = out of the ground.",
                vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
                vec![PortDef::new("out", "Normals", PortType::ColorMap)],
                vec![
                    ParamDef::choice(
                        "convention",
                        "Convention",
                        "opengl",
                        &[
                            ("opengl", "OpenGL (Godot, Blender, Unity)"),
                            ("directx", "DirectX (Unreal)"),
                        ],
                    )
                    .describe("Which way the green channel points."),
                    ParamDef::float("strength", "Strength", 1.0, 0.0, 20.0)
                        .describe("1 = true slopes; more exaggerates the relief."),
                ],
            ),
        }
    }
}

impl NodeKind for NormalMap {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        terrain_core::Reach::Local(crate::common::cell_m(ctx))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let h = ctx.input_grid("in")?;
        let (gx, gy) = gradient(h);
        let s = ctx.f32("strength");
        // Image rows run along world +Y, so "up the image" is world -Y.
        let up = if ctx.choice("convention") == "directx" {
            1.0
        } else {
            -1.0
        };
        let color = ColorGrid::from_fn_indexed(ctx.spec, |i, _, _| {
            let (nx, ny, nz) = (-gx.data[i] * s, -gy.data[i] * s, 1.0f32);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            [
                0.5 + 0.5 * nx / len,
                0.5 + 0.5 * up * ny / len,
                0.5 + 0.5 * nz / len,
                1.0,
            ]
        });
        Ok(Outputs::from([color_out("out", color)]))
    }
}

// ---- Occlusion ----------------------------------------------------------------

/// Cavity (an ambient-occlusion approximation): dark in hollows, creases and
/// valley floors. It measures how far each point lies below the mean height
/// around it at three scales, not the visible sky; for true sky visibility see
/// the sky-view factor (Zakšek, Oštir & Kokalj 2011) or horizon-based ambient
/// occlusion (Bavoil, Sainz & Dimitrov 2008).
pub struct Occlusion {
    schema: NodeSchema,
}

impl Default for Occlusion {
    fn default() -> Self {
        Self {
            schema: schema(
                "data.occlusion",
                "Occlusion",
                "Data",
                "An approximation of how open the sky is: white on ridges and open ground, dark in \
                 hollows, creases and valley floors, by how far each point lies below its surroundings \
                 (not a traced sky view). Radius sets the size of the features that shade; small radii \
                 give a cavity map.",
                vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
                vec![PortDef::new("out", "Occlusion", PortType::Mask)],
                vec![
                    ParamDef::metres("radius_m", "Radius", 100.0, 1.0, 100_000.0)
                        .describe("Size of the hollows that shade, in metres."),
                    ParamDef::float("strength", "Strength", 1.0, 0.0, 10.0).describe("How dark hollows get."),
                    ParamDef::bool("invert", "Invert", false).describe("White in hollows instead."),
                ],
            ),
        }
    }
}

impl NodeKind for Occlusion {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, ctx: &EvalContext) -> terrain_core::Reach {
        terrain_core::Reach::Local(crate::common::blur_reach(ctx.f64("radius_m")))
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let h = ctx.input_grid("in")?;
        let radius = ctx.f64("radius_m");
        let strength = ctx.f32("strength");
        // How far each point lies below its surroundings at three scales,
        // relative to the scale: a hollow as deep as it is wide is fully dark.
        let depths: Vec<(Grid, f32)> = [0.25, 0.5, 1.0]
            .iter()
            .map(|k| (gaussian_blur(h, radius * k), (radius * k) as f32))
            .collect();
        let invert = ctx.bool("invert");
        let out = h.map_indexed(|i, v| {
            let below: f32 = depths
                .iter()
                .map(|(g, r)| (g.data[i] - v).max(0.0) / r)
                .sum::<f32>()
                / 3.0;
            let open = (1.0 - strength * 4.0 * below).clamp(0.0, 1.0);
            if invert { 1.0 - open } else { open }
        });
        Ok(Outputs::from([("out".to_string(), Value::Mask(Arc::new(out)))]))
    }
}

// ---- Splat ------------------------------------------------------------------

const SPLAT_LAYERS: usize = 8;

/// Material weights that sum to 1, packed for engines.
pub struct Splat {
    schema: NodeSchema,
}

impl Default for Splat {
    fn default() -> Self {
        let mut inputs = Vec::new();
        for k in 1..=SPLAT_LAYERS {
            let label = if k == 1 {
                "Layer 1 (base)".to_string()
            } else {
                format!("Layer {k}")
            };
            inputs.push(PortDef::new(&format!("layer_{k}"), &label, PortType::Mask).optional());
        }
        let mut outputs = vec![
            PortDef::new("weights_1_4", "Weights 1-4 (RGBA)", PortType::ColorMap),
            PortDef::new("weights_5_8", "Weights 5-8 (RGBA)", PortType::ColorMap),
        ];
        for k in 1..=SPLAT_LAYERS {
            outputs.push(PortDef::new(
                &format!("weight_{k}"),
                &format!("Weight {k}"),
                PortType::Mask,
            ));
        }
        Self {
            schema: schema(
                "output.splat",
                "Splat Map",
                "Output",
                "Turns up to 8 material masks into weights that add up to 1 at every point, for terrain \
                 texturing in engines. Layer 1 is the base: if it isn't connected it fills whatever the \
                 other layers leave. Weights 1-4 and 5-8 pack four layers into R, G, B and A; each weight \
                 is also available on its own.",
                inputs,
                outputs,
                vec![],
            ),
        }
    }
}

impl NodeKind for Splat {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let layers: Vec<Option<Arc<Grid>>> = (1..=SPLAT_LAYERS)
            .map(|k| ctx.input(&format!("layer_{k}")).map(|v| v.grid().clone()))
            .collect();
        let n = ctx.spec.len();
        let mut weights = vec![[0.0f32; SPLAT_LAYERS]; n];
        weights.par_iter_mut().enumerate().for_each(|(i, w)| {
            let raw = |k: usize| layers[k].as_ref().map_or(0.0, |g| g.data[i].clamp(0.0, 1.0));
            let others: f32 = (1..SPLAT_LAYERS).map(raw).sum();
            w[0] = if layers[0].is_some() {
                raw(0)
            } else {
                (1.0 - others).max(0.0)
            };
            for (k, v) in w.iter_mut().enumerate().skip(1) {
                *v = raw(k);
            }
            let sum: f32 = w.iter().sum();
            if sum > 1.0e-6 {
                w.iter_mut().for_each(|v| *v /= sum);
            } else {
                *w = [0.0; SPLAT_LAYERS];
                w[0] = 1.0;
            }
        });
        let pack = |first: usize| ColorGrid {
            spec: ctx.spec,
            data: weights
                .iter()
                .flat_map(|w| [w[first], w[first + 1], w[first + 2], w[first + 3]])
                .collect(),
        };
        let mut outputs = Outputs::from([
            color_out("weights_1_4", pack(0)),
            color_out("weights_5_8", pack(4)),
        ]);
        for k in 0..SPLAT_LAYERS {
            let grid = Grid {
                spec: ctx.spec,
                data: weights.iter().map(|w| w[k]).collect(),
            };
            outputs.insert(format!("weight_{}", k + 1), Value::Mask(Arc::new(grid)));
        }
        Ok(outputs)
    }
}

// ---- Portals ------------------------------------------------------------------

/// A Terrain-tab output brought into the Colour tab (ARCHITECTURE.md §5).
/// Passes its input through unchanged.
pub struct Portal {
    schema: NodeSchema,
}

impl Portal {
    /// A portal for heightfields (`portal.height`) or masks (`portal.mask`).
    pub fn new(ty: PortType) -> Self {
        let (id, label, what) = match ty {
            PortType::Heightfield => ("portal.height", "Height Portal", "a heightfield"),
            _ => ("portal.mask", "Mask Portal", "a mask"),
        };
        Self {
            schema: schema(
                id,
                label,
                "Portal",
                &format!(
                    "Brings {what} from another tab (e.g. Terrain into Vegetation or Colour), so work \
                     in this tab never recomputes the other. Create one with \"Send to … tab\" on a \
                     node's output."
                ),
                vec![PortDef::new("in", "Source", ty)],
                vec![PortDef::new("out", "Out", ty)],
                vec![],
            ),
        }
    }
}

impl NodeKind for Portal {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let v = ctx.input("in").cloned().ok_or_else(|| CoreError::MissingInput {
            node: ctx.node_id.into(),
            port: "in".into(),
        })?;
        Ok(Outputs::from([("out".to_string(), v)]))
    }
}
