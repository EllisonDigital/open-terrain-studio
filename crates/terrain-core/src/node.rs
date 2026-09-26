use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::gpu::{Gpu, GpuGrid};
use crate::grid::{ColorGrid, Grid, GridSpec};
use crate::params::{Curve, Gradient, ParamDef, ParamKind, ParamValue};
use crate::points::PointSet;
use crate::world::World;

/// The type of data flowing through a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// Heights in metres.
    Heightfield,
    /// Values in 0..1 (masks, densities, weights).
    Mask,
    /// An sRGB colour with alpha per sample (colour maps, packed weights,
    /// normal maps).
    ColorMap,
    /// Vegetation or debris instances ([`PointSet`]).
    PointSet,
}

impl PortType {
    /// Can data of type `self` feed an input of type `input`?
    /// Heightfield and Mask convert into each other automatically; ColorMap
    /// only feeds ColorMap and PointSet only PointSet.
    pub fn can_feed(self, input: PortType) -> bool {
        matches!(
            (self, input),
            (
                PortType::Heightfield | PortType::Mask,
                PortType::Heightfield | PortType::Mask
            ) | (PortType::ColorMap, PortType::ColorMap)
                | (PortType::PointSet, PortType::PointSet)
        )
    }

    /// The key used in schemas and by the editor, e.g. `color_map`.
    pub fn key(self) -> &'static str {
        match self {
            PortType::Heightfield => "heightfield",
            PortType::Mask => "mask",
            PortType::ColorMap => "color_map",
            PortType::PointSet => "point_set",
        }
    }
}

/// Data produced by a node output.
#[derive(Clone, Debug)]
pub enum Value {
    Heightfield(Arc<Grid>),
    Mask(Arc<Grid>),
    ColorMap(Arc<ColorGrid>),
    Points(Arc<PointSet>),
    /// A result still on the GPU; read back on first [`Value::grid`].
    Gpu(PortType, Arc<GpuGrid>),
}

impl Value {
    pub fn port_type(&self) -> PortType {
        match self {
            Value::Heightfield(_) => PortType::Heightfield,
            Value::Mask(_) => PortType::Mask,
            Value::ColorMap(_) => PortType::ColorMap,
            Value::Points(_) => PortType::PointSet,
            Value::Gpu(ty, _) => *ty,
        }
    }

    /// The data on the CPU (a GPU result is downloaded the first time).
    ///
    /// # Panics
    /// For a colour map or point set; use [`Value::color`],
    /// [`Value::points`] or [`Value::samples`].
    pub fn grid(&self) -> &Arc<Grid> {
        match self {
            Value::Heightfield(g) | Value::Mask(g) => g,
            Value::Gpu(_, g) => g.cpu(),
            Value::ColorMap(_) => panic!("a colour map has no single-channel grid"),
            Value::Points(_) => panic!("a point set has no grid"),
        }
    }

    /// The colour map, if this is one.
    pub fn color(&self) -> Option<&Arc<ColorGrid>> {
        match self {
            Value::ColorMap(c) => Some(c),
            _ => None,
        }
    }

    /// The point set, if this is one.
    pub fn points(&self) -> Option<&Arc<PointSet>> {
        match self {
            Value::Points(p) => Some(p),
            _ => None,
        }
    }

    /// Every sample value (every channel of every sample for a colour map;
    /// every stored value of every point for a point set), e.g. for hashing
    /// or range checks.
    pub fn samples(&self) -> &[f32] {
        match self {
            Value::ColorMap(c) => &c.data,
            Value::Points(p) => &p.data,
            _ => &self.grid().data,
        }
    }

    /// One grid per channel: the grid itself, or red, green, blue and alpha.
    /// None for a point set.
    pub fn channels(&self) -> Vec<Grid> {
        match self {
            Value::ColorMap(c) => (0..4).map(|i| c.channel(i)).collect(),
            Value::Points(_) => Vec::new(),
            _ => vec![self.grid().as_ref().clone()],
        }
    }

    /// Memory used on the CPU, in bytes.
    pub fn bytes(&self) -> usize {
        match self {
            Value::ColorMap(c) => c.data.len() * 4,
            Value::Points(p) => p.data.len() * 4 + p.species_index.len() * 2,
            _ => self.spec().len() * 4,
        }
    }

    /// Resolution and world region, without reading GPU data back.
    pub fn spec(&self) -> GridSpec {
        match self {
            Value::Heightfield(g) | Value::Mask(g) => g.spec,
            Value::ColorMap(c) => c.spec,
            Value::Points(p) => p.spec,
            Value::Gpu(_, g) => g.spec,
        }
    }

    /// The part of this value inside `to`, a window of the same whole grid
    /// inside this value's grid. Points outside it are dropped.
    pub fn crop(&self, to: GridSpec) -> Value {
        if self.spec() == to {
            return self.clone();
        }
        match self {
            Value::Heightfield(g) => Value::Heightfield(Arc::new(g.crop(to))),
            Value::Mask(g) => Value::Mask(Arc::new(g.crop(to))),
            Value::ColorMap(c) => Value::ColorMap(Arc::new(c.crop(to))),
            Value::Points(p) => Value::Points(Arc::new(p.within(to))),
            Value::Gpu(ty, g) => match ty {
                PortType::Mask => Value::Mask(Arc::new(g.cpu().crop(to))),
                _ => Value::Heightfield(Arc::new(g.cpu().crop(to))),
            },
        }
    }

    /// Convert to another port type using the world height range. A GPU
    /// value is converted on the GPU (or, if that fails, on the CPU).
    pub fn convert(&self, to: PortType, world: &World) -> Value {
        match (self, to) {
            (Value::Heightfield(g), PortType::Heightfield) => Value::Heightfield(g.clone()),
            (Value::Mask(g), PortType::Mask) => Value::Mask(g.clone()),
            (Value::Heightfield(g), PortType::Mask) => Value::Mask(Arc::new(g.map(|h| world.normalise(h)))),
            (Value::Mask(g), PortType::Heightfield) => {
                Value::Heightfield(Arc::new(g.map(|m| world.denormalise(m))))
            }
            // Colour maps and point sets only feed inputs of their own type
            // (see `PortType::can_feed`).
            (Value::ColorMap(_) | Value::Points(_), _) | (_, PortType::ColorMap | PortType::PointSet) => {
                self.clone()
            }
            (Value::Gpu(from, _), to) if *from == to => self.clone(),
            (Value::Gpu(from, g), to) => {
                let span = world.height_span();
                let min = world.height_range_m[0];
                let (scale, offset) = match to {
                    PortType::Mask => (1.0 / span, -min / span),
                    PortType::Heightfield => (span, min),
                    PortType::ColorMap | PortType::PointSet => unreachable!(),
                };
                let gpu = Gpu::new(g.buffer.device().clone());
                match gpu.affine(g.spec, &g.buffer, scale, offset) {
                    Ok(buffer) => gpu.value(to, g.spec, buffer),
                    Err(_) => {
                        let cpu = match from {
                            PortType::Heightfield => Value::Heightfield(g.cpu().clone()),
                            PortType::Mask => Value::Mask(g.cpu().clone()),
                            PortType::ColorMap | PortType::PointSet => {
                                unreachable!("no GPU colour maps or points")
                            }
                        };
                        cpu.convert(to, world)
                    }
                }
            }
        }
    }
}

/// Declaration of one input or output port.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: PortType,
    /// Inputs only: may be left unconnected.
    pub optional: bool,
    /// For a parameter port (a drivable parameter exposed as an input), the
    /// parameter key. Empty for ordinary ports.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub param: String,
}

/// Input port keys for exposed parameters start with this, e.g. `p:height_m`.
pub const PARAM_PORT_PREFIX: &str = "p:";

/// The input port key for a drivable parameter.
pub fn param_port_key(param: &str) -> String {
    format!("{PARAM_PORT_PREFIX}{param}")
}

impl PortDef {
    pub fn new(key: &str, label: &str, ty: PortType) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            ty,
            optional: false,
            param: String::new(),
        }
    }

    /// The optional Mask input that drives parameter `def`.
    pub fn for_param(def: &ParamDef) -> Self {
        Self {
            key: param_port_key(&def.key),
            label: def.label.clone(),
            ty: PortType::Mask,
            optional: true,
            param: def.key.clone(),
        }
    }
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

/// Everything the editor needs to know about a node type.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NodeSchema {
    /// Stable identifier stored in project files, e.g. `noise.perlin`. Never rename.
    pub type_id: String,
    /// Bumped when parameters change meaning; see [`NodeKind::migrate`].
    pub type_version: u32,
    pub label: String,
    pub category: String,
    pub description: String,
    pub inputs: Vec<PortDef>,
    pub outputs: Vec<PortDef>,
    pub params: Vec<ParamDef>,
    /// Whether a GPU kernel exists ([`NodeKind::evaluate_gpu`]).
    pub gpu: bool,
}

impl NodeSchema {
    pub fn input(&self, key: &str) -> Option<&PortDef> {
        self.inputs.iter().find(|p| p.key == key)
    }
    pub fn output(&self, key: &str) -> Option<&PortDef> {
        self.outputs.iter().find(|p| p.key == key)
    }
    pub fn param(&self, key: &str) -> Option<&ParamDef> {
        self.params.iter().find(|p| p.key == key)
    }

    /// Input ports of a node instance: the schema's inputs, then a port for
    /// each exposed drivable parameter (in parameter order).
    pub fn input_ports(&self, exposed: &std::collections::BTreeSet<String>) -> Vec<PortDef> {
        let mut ports = self.inputs.clone();
        ports.extend(
            self.params
                .iter()
                .filter(|p| p.drivable && exposed.contains(&p.key))
                .map(PortDef::for_param),
        );
        ports
    }
}

/// Results of one node evaluation, by output port key.
pub type Outputs = BTreeMap<String, Value>;

/// A node type. Implement this (in `terrain-nodes`) to add a node to the app.
pub trait NodeKind: Send + Sync + 'static {
    fn schema(&self) -> &NodeSchema;

    /// Compute every output. Must be deterministic: same context in, same bits out.
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs>;

    /// Compute every output on the GPU, for nodes whose schema sets `gpu`.
    /// Must match [`NodeKind::evaluate`] within the node's GPU tolerance.
    /// Return [`CoreError::GpuUnsupported`] for settings the kernel doesn't
    /// cover; the evaluator then uses the CPU, as it does for any other error.
    fn evaluate_gpu(&self, _ctx: &EvalContext, _gpu: &Gpu) -> Result<Outputs> {
        Err(CoreError::GpuUnsupported)
    }

    /// Upgrade parameters saved by an older `type_version`. Default: no change.
    fn migrate(&self, _from_version: u32, _params: &mut BTreeMap<String, ParamValue>) -> Result<()> {
        Ok(())
    }

    /// Extra text mixed into this node's cache key, for results that depend on
    /// something outside the graph (e.g. an imported file's size and date).
    fn cache_salt(&self, _params: &BTreeMap<String, ParamValue>, _base_dir: Option<&Path>) -> String {
        String::new()
    }

    /// How far around each sample this node reads its inputs, for tiled
    /// builds. `ctx` has the parameters and the whole build grid, but no
    /// inputs. The default, [`Reach::Global`], is always correct but gives
    /// the node a world pass; point-wise and neighbourhood nodes should say
    /// [`Reach::Local`].
    fn reach(&self, _ctx: &EvalContext) -> Reach {
        Reach::Global
    }

    /// Global nodes in a tiled build: the whole-world grid the world pass
    /// runs on, for a build over `ctx.spec`. Its inputs are brought to this
    /// grid the way [`EvalContext::spec`] is filtered and resampled for
    /// simulations. `None` if the node only needs each input's range
    /// ([`WorldPass::ranges`]). Default: the build grid, capped at
    /// [`WORLD_PASS_RESOLUTION`].
    fn world_spec(&self, ctx: &EvalContext) -> Option<GridSpec> {
        Some(default_world_spec(ctx.spec))
    }

    /// Global nodes in a tiled build: how far [`NodeKind::finish_tile`]
    /// reads its tile inputs around each sample, in metres (e.g. a band
    /// along a shoreline). Default: 0.
    fn finish_reach(&self, _ctx: &EvalContext) -> f64 {
        0.0
    }

    /// Global nodes in a tiled build: how an output of the world pass is
    /// brought to a tile's grid by [`NodeKind::finish_tile`]'s default.
    /// Default: a `height` output keeps the fine detail of the `in` input
    /// ([`Upsample::Detail`]); everything else is bilinear.
    fn upsample(&self, output: &PortDef) -> Upsample {
        let schema = self.schema();
        let detail = output.ty == PortType::Heightfield
            && output.key == "height"
            && schema.input("in").is_some_and(|i| i.ty == PortType::Heightfield);
        if detail {
            Upsample::Detail("in".into())
        } else {
            Upsample::Bilinear
        }
    }

    /// Global nodes in a tiled build: this node's outputs over one tile
    /// (`ctx.spec`, with the tile's inputs), from its world pass.
    fn finish_tile(&self, ctx: &EvalContext, world: &WorldPass) -> Result<Outputs> {
        let mut outputs = Outputs::new();
        for port in &self.schema().outputs {
            let Some(value) = world.outputs.get(&port.key) else {
                continue;
            };
            let upsample = self.upsample(port);
            outputs.insert(port.key.clone(), upsample_value(ctx, world, value, &upsample)?);
        }
        Ok(outputs)
    }
}

/// How far a node reads around each sample (see [`NodeKind::reach`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    /// Each output sample depends only on input samples within this many
    /// metres of it (0 = only the sample at the same position).
    Local(f64),
    /// Outputs depend on the whole world (water routing, auto ranges…).
    Global,
}

/// How a world-pass output becomes a tile's output (see [`NodeKind::upsample`]).
#[derive(Clone, Debug, PartialEq)]
pub enum Upsample {
    Bilinear,
    /// For categories and angles, which must not be blended.
    Nearest,
    /// The tile's own input on this port plus the change the node made to
    /// it in the world pass: heights keep detail finer than the world pass.
    Detail(String),
}

/// Largest world-pass grid per axis by default ([`NodeKind::world_spec`]).
pub const WORLD_PASS_RESOLUTION: u32 = 4097;

/// The whole build grid `full`, capped at [`WORLD_PASS_RESOLUTION`].
pub fn default_world_spec(full: GridSpec) -> GridSpec {
    let full = full.whole();
    let cap = |n: u32| n.min(WORLD_PASS_RESOLUTION);
    GridSpec::new(cap(full.width), cap(full.height), full.origin_m, full.extent_m)
}

/// A global node's whole-world result, for finishing its tiles.
pub struct WorldPass {
    /// The world-pass grid (`None` if the node only asked for ranges).
    pub spec: Option<GridSpec>,
    /// Inputs on the world-pass grid.
    pub inputs: BTreeMap<String, Value>,
    /// Outputs on the world-pass grid.
    pub outputs: Outputs,
    /// Exact lowest and highest value of each grid input over the whole
    /// build, at build resolution.
    pub ranges: BTreeMap<String, (f32, f32)>,
}

/// `value` (a world-pass output) brought to `ctx.spec` by `how`.
pub fn upsample_value(ctx: &EvalContext, world: &WorldPass, value: &Value, how: &Upsample) -> Result<Value> {
    let spec = ctx.spec;
    Ok(match value {
        Value::Points(p) => Value::Points(Arc::new(p.within(spec))),
        Value::ColorMap(c) => Value::ColorMap(Arc::new(ColorGrid::from_fn_indexed(spec, |_, x, y| {
            c.sample_bilinear_m(x, y)
        }))),
        _ => {
            let ty = value.port_type();
            let g = value.grid();
            let grid = match how {
                Upsample::Bilinear => Grid::from_fn(spec, |x, y| g.sample_bilinear_m(x, y)),
                Upsample::Nearest => Grid::from_fn(spec, |x, y| g.sample_nearest_m(x, y)),
                Upsample::Detail(port) => {
                    let fine = ctx.input_grid(port)?;
                    let coarse = world.inputs.get(port).ok_or_else(|| CoreError::MissingInput {
                        node: ctx.node_id.into(),
                        port: port.clone(),
                    })?;
                    let coarse = coarse.grid();
                    Grid::from_fn_indexed(spec, |i, x, y| {
                        fine.data[i] + (g.sample_bilinear_m(x, y) - coarse.sample_bilinear_m(x, y))
                    })
                }
            };
            match ty {
                PortType::Mask => Value::Mask(Arc::new(grid)),
                _ => Value::Heightfield(Arc::new(grid)),
            }
        }
    })
}

/// A float parameter that may vary per cell (see [`ParamDef::drivable`]).
#[derive(Clone, Copy)]
pub enum Field<'a> {
    Const(f32),
    /// `value × mask`, clamped to `min..max`.
    Driven {
        value: f32,
        min: f32,
        max: f32,
        mask: &'a Grid,
    },
}

impl Field<'_> {
    /// The value at flat cell index `idx` (row-major, like `Grid::data`).
    #[inline]
    pub fn at(&self, idx: usize) -> f32 {
        match *self {
            Field::Const(v) => v,
            Field::Driven {
                value,
                min,
                max,
                mask,
            } => (value * mask.data[idx].clamp(0.0, 1.0)).clamp(min, max),
        }
    }

    /// True if the value is the same everywhere.
    pub fn is_const(&self) -> bool {
        matches!(self, Field::Const(_))
    }
}

/// What a node sees while it evaluates.
pub struct EvalContext<'a> {
    pub world: &'a World,
    /// The grid (resolution + world region) being computed.
    pub spec: GridSpec,
    /// Node seed, derived from project seed, node id and the node's `seed` param.
    pub seed: u64,
    pub node_id: &'a str,
    pub(crate) schema: &'a NodeSchema,
    pub(crate) params: &'a BTreeMap<String, ParamValue>,
    pub(crate) inputs: BTreeMap<String, Value>,
    pub(crate) cancel: Option<&'a AtomicBool>,
    /// Node-local progress, see [`EvalContext::report_progress`].
    pub(crate) progress: Option<&'a (dyn Fn(f32) + Sync)>,
    /// Folder relative file paths are resolved against (the project's folder).
    pub base_dir: Option<&'a Path>,
}

impl<'a> EvalContext<'a> {
    /// Build a context directly (used by tests and tools; the evaluator builds its own).
    pub fn new(
        world: &'a World,
        spec: GridSpec,
        seed: u64,
        node_id: &'a str,
        schema: &'a NodeSchema,
        params: &'a BTreeMap<String, ParamValue>,
        inputs: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            world,
            spec,
            seed,
            node_id,
            schema,
            params,
            inputs,
            cancel: None,
            progress: None,
            base_dir: None,
        }
    }

    /// Attach controls for direct evaluations (tests, tools and long simulations).
    pub fn with_controls(
        mut self,
        cancel: Option<&'a AtomicBool>,
        progress: Option<&'a (dyn Fn(f32) + Sync)>,
    ) -> Self {
        self.cancel = cancel;
        self.progress = progress;
        self
    }

    /// Report node-local progress 0..1. Call monotonically from the
    /// coordinating thread; the evaluator maps it to overall progress.
    pub fn report_progress(&self, fraction: f32) {
        if fraction.is_finite()
            && let Some(progress) = self.progress
        {
            progress(fraction.clamp(0.0, 1.0));
        }
    }

    fn param(&self, key: &str) -> ParamValue {
        let def = self
            .schema
            .param(key)
            .unwrap_or_else(|| panic!("node '{}' has no parameter '{key}'", self.schema.type_id));
        self.params
            .get(key)
            .and_then(|v| def.validate(v).ok())
            .unwrap_or_else(|| def.default.clone())
    }

    pub fn f64(&self, key: &str) -> f64 {
        self.param(key).as_f64().unwrap_or(0.0)
    }
    pub fn f32(&self, key: &str) -> f32 {
        self.f64(key) as f32
    }
    pub fn i64(&self, key: &str) -> i64 {
        self.param(key).as_i64().unwrap_or(0)
    }
    pub fn bool(&self, key: &str) -> bool {
        self.param(key).as_bool().unwrap_or(false)
    }
    pub fn choice(&self, key: &str) -> String {
        self.param(key).as_str().unwrap_or_default().to_string()
    }
    pub fn text(&self, key: &str) -> String {
        self.choice(key)
    }
    pub fn curve(&self, key: &str) -> Curve {
        Curve::parse(&self.param(key))
            .or_else(|_| Curve::parse(&self.schema.param(key).expect("curve param").default))
            .expect("curve default is valid")
    }

    pub fn gradient(&self, key: &str) -> Gradient {
        Gradient::parse(&self.param(key))
            .or_else(|_| Gradient::parse(&self.schema.param(key).expect("gradient param").default))
            .expect("gradient default is valid")
    }

    /// A float parameter that a mask may drive per cell (see [`ParamDef::drivable`]).
    pub fn field(&self, key: &str) -> Field<'_> {
        let value = self.f32(key);
        match self.inputs.get(&param_port_key(key)) {
            Some(v) => {
                let (min, max) = self.param_range(key);
                Field::Driven {
                    value,
                    min,
                    max,
                    mask: v.grid(),
                }
            }
            None => Field::Const(value),
        }
    }

    /// The range a driven float parameter is clamped to.
    pub(crate) fn param_range(&self, key: &str) -> (f32, f32) {
        match self.schema.param(key).map(|d| &d.kind) {
            Some(ParamKind::Float { min, max, .. }) => (*min as f32, *max as f32),
            _ => (f32::MIN, f32::MAX),
        }
    }

    /// Resolve a file path parameter against the project folder.
    pub fn path(&self, key: &str) -> Option<PathBuf> {
        resolve_path(&self.text(key), self.base_dir)
    }

    /// An input, already converted to the port's declared type. `None` if unconnected.
    pub fn input(&self, key: &str) -> Option<&Value> {
        self.inputs.get(key)
    }

    /// An input grid; errors if a required input is missing.
    pub fn input_grid(&self, key: &str) -> Result<&Arc<Grid>> {
        self.inputs
            .get(key)
            .map(|v| v.grid())
            .ok_or_else(|| CoreError::MissingInput {
                node: self.node_id.into(),
                port: key.into(),
            })
    }

    /// An input colour map; errors if a required input is missing.
    pub fn input_color(&self, key: &str) -> Result<&Arc<ColorGrid>> {
        self.inputs
            .get(key)
            .and_then(|v| v.color())
            .ok_or_else(|| CoreError::MissingInput {
                node: self.node_id.into(),
                port: key.into(),
            })
    }

    /// An input point set; errors if a required input is missing.
    pub fn input_points(&self, key: &str) -> Result<&Arc<PointSet>> {
        self.inputs
            .get(key)
            .and_then(|v| v.points())
            .ok_or_else(|| CoreError::MissingInput {
                node: self.node_id.into(),
                port: key.into(),
            })
    }

    /// True once the user has cancelled; long nodes should check this and stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }
}

/// Resolve a (possibly relative) path against `base_dir`. `None` if empty.
pub fn resolve_path(path: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let p = Path::new(path);
    Some(match base_dir {
        Some(dir) if p.is_relative() => dir.join(p),
        _ => p.to_path_buf(),
    })
}

/// All node types known to the app, keyed by `type_id`.
#[derive(Default, Clone)]
pub struct NodeRegistry {
    kinds: BTreeMap<String, Arc<dyn NodeKind>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, kind: impl NodeKind) {
        let id = kind.schema().type_id.clone();
        assert!(!self.kinds.contains_key(&id), "node type '{id}' registered twice");
        self.kinds.insert(id, Arc::new(kind));
    }

    pub fn get(&self, type_id: &str) -> Option<&Arc<dyn NodeKind>> {
        self.kinds.get(type_id)
    }

    pub fn schema(&self, type_id: &str) -> Option<&NodeSchema> {
        self.kinds.get(type_id).map(|k| k.schema())
    }

    /// All schemas, sorted by type id.
    pub fn schemas(&self) -> impl Iterator<Item = &NodeSchema> {
        self.kinds.values().map(|k| k.schema())
    }
}
