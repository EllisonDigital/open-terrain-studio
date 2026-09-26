//! Export writers: EXR 32-bit float, PNG 16 and 8-bit (grey, or RGBA for colour
//! maps), CSV and JSON for point sets, and the `build.json` sidecar.
//!
//! Image orientation: row 0 of every exported image is world Y = 0 and column 0
//! is world X = 0 (top-left of the image = world origin). Unreal, Godot and
//! Blender all read heightmaps this way.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::assemble::{RawImage, Rect, file_tiles, tiles_even, write_exr_streamed, write_png_streamed};
use crate::error::{CoreError, Result};
use crate::eval::{EvalOptions, evaluate_node};
use crate::grid::{ColorGrid, Grid, GridSpec};
use crate::node::{NodeRegistry, PortType, Value};
use crate::points::PointSet;
use crate::project::{FileTiles, Project};
use crate::tiled::{TILED_ABOVE, evaluate_tiled};
use crate::world::World;

/// Supported export formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    /// OpenEXR, one 32-bit float channel `Y`. Heightfields are written in metres.
    Exr32,
    /// Greyscale PNG, 16 bits. Heightfields are remapped: 0 = world min height,
    /// 65535 = world max height. Masks: 0..1 -> 0..65535.
    Png16,
    /// Greyscale PNG, 8 bits, remapped like [`ExportFormat::Png16`] to 0..255.
    /// For masks and colour maps engines load directly.
    Png8,
    /// Point sets as CSV: `x,y,z,rotation_deg,scale,species` per row.
    Csv,
    /// Point sets as JSON (same columns as CSV).
    Json,
}

impl ExportFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exr32" | "exr" => Some(Self::Exr32),
            "png16" | "png" => Some(Self::Png16),
            "png8" => Some(Self::Png8),
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Exr32 => "exr32",
            Self::Png16 => "png16",
            Self::Png8 => "png8",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }

    /// Whether this format can hold data of type `ty`: images for grids,
    /// CSV and JSON for point sets.
    pub fn supports(self, ty: PortType) -> bool {
        matches!(self, Self::Csv | Self::Json) == (ty == PortType::PointSet)
    }
    /// File name ending, e.g. .png; 8-bit PNGs end _8bit.png so they
    /// don't overwrite a 16-bit PNG of the same output.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Exr32 => "exr",
            Self::Png16 => "png",
            Self::Png8 => "8bit.png",
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

/// One written file, as listed in `build.json`.
#[derive(Clone, Debug, Serialize)]
pub struct ExportedFile {
    pub file: String,
    pub node: String,
    pub port: String,
    pub data: PortType,
    pub format: ExportFormat,
    /// What stored values mean: e.g. "metres" or "normalised to height_range_m".
    pub encoding: String,
    /// Actual min/max of the exported data, in its natural unit (metres or
    /// 0..1; point heights for point sets).
    pub data_min: f32,
    pub data_max: f32,
    /// Point sets only: how many points the file lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points: Option<usize>,
    /// Point sets only: the species names used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub species: Option<Vec<String>>,
    /// Tile files only: the tile's column and row, and where its first
    /// sample lies in the whole build, in samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_origin: Option<[u32; 2]>,
    /// Tile files only: the file's size in samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tile_size: Option<[u32; 2]>,
}

/// How a build's images were split into tile files, in `build.json`.
#[derive(Clone, Debug, Serialize)]
pub struct TileFilesInfo {
    /// Samples per tile side; neighbouring tiles share their edge samples.
    pub size: u32,
    pub columns: u32,
    pub rows: u32,
    pub pattern: String,
    /// Every tile is `size` samples (what Unreal World Partition expects).
    pub even: bool,
}

/// What point files store, as written to `build.json`.
pub const POINTS_ENCODING: &str = "x,y metres from the world origin (row 0 / column 0 corner); z metres; rotation_deg about vertical (0 = +X, 90 = +Y); scale; species";

/// Contents of the `build.json` sidecar written next to exported files.
#[derive(Clone, Debug, Serialize)]
pub struct BuildInfo {
    pub generator: String,
    pub app_version: String,
    pub world_size_m: [f64; 2],
    pub height_range_m: [f32; 2],
    pub resolution: [u32; 2],
    pub cell_size_m: [f64; 2],
    pub seed: u64,
    pub orientation: String,
    /// "cpu" (bit-exact on every machine) or "gpu: <device>" (matches the
    /// CPU within tolerance).
    pub compute: String,
    pub files: Vec<ExportedFile>,
    pub unreal: UnrealHints,
    /// Set when images were written as tile files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_tiles: Option<TileFilesInfo>,
    /// Set when the build was computed in tiles (large builds): their size
    /// in samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_in_tiles: Option<u32>,
}

/// Values to type into Unreal's landscape import dialog for 16-bit heightmaps.
///
/// Unreal reads a 16-bit value `v` as a height of `(v - 32768) / 128 * z_scale`
/// centimetres above the landscape's Z location, so Z scale 100 spans
/// -256 m to 255.992 m. Our PNG stores `v = (h - min) / span * 65535`, which
/// gives `z_scale = span_m * 12800 / 65535` and puts 32768 at
/// `min + span * 32768 / 65535`.
#[derive(Clone, Debug, Serialize)]
pub struct UnrealHints {
    /// Landscape X/Y scale in Unreal units (cm) per heightmap pixel.
    pub xy_scale: f64,
    /// Landscape Z scale (100 = the 16-bit range covers 512 m).
    pub z_scale: f64,
    /// Landscape Z location (cm) so heights line up with world metres.
    pub z_location: f64,
}

impl UnrealHints {
    pub fn new(world: &World, cell_size_m: f64) -> Self {
        let min = world.height_range_m[0] as f64;
        let span = world.height_span() as f64;
        Self {
            xy_scale: cell_size_m * 100.0,
            z_scale: span * 12800.0 / 65535.0,
            z_location: (min + span * 32768.0 / 65535.0) * 100.0,
        }
    }
}

impl BuildInfo {
    pub fn new(world: &World, spec: &GridSpec, files: Vec<ExportedFile>) -> Self {
        let cell = spec.cell_size_m();
        Self {
            generator: "OpenTerrainStudio".into(),
            app_version: crate::APP_VERSION.into(),
            world_size_m: world.size_m,
            height_range_m: world.height_range_m,
            resolution: [spec.width, spec.height],
            cell_size_m: cell,
            seed: world.seed,
            orientation: "row 0 = world Y 0, column 0 = world X 0".into(),
            compute: "cpu".into(),
            files,
            unreal: UnrealHints::new(world, cell[0]),
            file_tiles: None,
            computed_in_tiles: None,
        }
    }
}

/// Write one value as the requested format. Returns the file entry.
pub fn write_value(
    value: &Value,
    world: &World,
    format: ExportFormat,
    path: &Path,
    node: &str,
    port: &str,
) -> Result<ExportedFile> {
    let ty = value.port_type();
    if !format.supports(ty) {
        return Err(CoreError::Project(format!(
            "{node}.{port}: {} can't be written as {}",
            ty.key(),
            format.key()
        )));
    }
    if let Some(points) = value.points() {
        let text = match format {
            ExportFormat::Csv => points.to_csv(),
            _ => points.to_json(),
        };
        std::fs::write(path, text)?;
        let (lo, hi) = points.z_range();
        return Ok(ExportedFile {
            file: file_name(path),
            node: node.into(),
            port: port.into(),
            data: ty,
            format,
            encoding: POINTS_ENCODING.into(),
            data_min: lo,
            data_max: hi,
            points: Some(points.len()),
            species: Some(points.species.clone()),
            tile: None,
            tile_origin: None,
            tile_size: None,
        });
    }
    let (lo, hi) = value
        .samples()
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let encoding = encoding(format, ty);
    if let Some(color) = value.color() {
        match format {
            ExportFormat::Exr32 => write_exr32_rgba(color, path)?,
            ExportFormat::Png16 => write_png_rgba(color, true, path)?,
            ExportFormat::Png8 => write_png_rgba(color, false, path)?,
            ExportFormat::Csv | ExportFormat::Json => unreachable!(),
        }
    } else {
        let grid = value.grid();
        let normalised = || match ty {
            PortType::Heightfield => grid.map(|h| world.normalise(h)),
            _ => (**grid).clone(),
        };
        match format {
            ExportFormat::Exr32 => write_exr32(grid, path)?,
            ExportFormat::Png16 => write_png16(&normalised(), path)?,
            ExportFormat::Png8 => write_png8(&normalised(), path)?,
            ExportFormat::Csv | ExportFormat::Json => unreachable!(),
        }
    }
    Ok(ExportedFile {
        file: file_name(path),
        node: node.into(),
        port: port.into(),
        data: ty,
        format,
        encoding,
        data_min: lo,
        data_max: hi,
        points: None,
        species: None,
        tile: None,
        tile_origin: None,
        tile_size: None,
    })
}

/// What stored values mean in `format`, for `build.json`.
fn encoding(format: ExportFormat, ty: PortType) -> String {
    match (format, ty) {
        (ExportFormat::Exr32, PortType::Heightfield) => "metres",
        (ExportFormat::Exr32, PortType::Mask) => "0..1",
        (ExportFormat::Exr32, PortType::ColorMap) => "RGBA 0..1 as stored (sRGB for colours)",
        (ExportFormat::Png16, PortType::Heightfield) => "0..65535 = height_range_m min..max",
        (ExportFormat::Png16, PortType::Mask) => "0..65535 = 0..1",
        (ExportFormat::Png16, PortType::ColorMap) => "RGBA, sRGB, 0..65535 = 0..1",
        (ExportFormat::Png8, PortType::Heightfield) => "0..255 = height_range_m min..max",
        (ExportFormat::Png8, PortType::Mask) => "0..255 = 0..1",
        (ExportFormat::Png8, PortType::ColorMap) => "RGBA, sRGB, 0..255 = 0..1",
        (_, PortType::PointSet) => POINTS_ENCODING,
        _ => unreachable!("checked by ExportFormat::supports"),
    }
    .to_string()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Write a single-channel 32-bit float EXR (channel `Y`).
pub fn write_exr32(grid: &Grid, path: &Path) -> Result<()> {
    use exr::prelude::*;
    let size = (grid.spec.width as usize, grid.spec.height as usize);
    let channel = AnyChannel::new("Y", FlatSamples::F32(grid.data.clone()));
    let layer = Layer::new(
        size,
        LayerAttributes::named("height"),
        // Fixed line order + single-threaded writing = byte-identical files.
        Encoding {
            compression: Compression::ZIP16,
            blocks: Blocks::ScanLines,
            line_order: LineOrder::Increasing,
        },
        AnyChannels::sort(vec![channel].into()),
    );
    Image::from_layer(layer)
        .write()
        .non_parallel()
        .to_file(path)
        .map_err(|e| CoreError::Image(e.to_string()))
}

/// Write a 16-bit greyscale PNG from 0..1 values (clamped).
pub fn write_png16(normalised: &Grid, path: &Path) -> Result<()> {
    let pixels: Vec<u16> = normalised
        .data
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)
        .collect();
    let img = image::ImageBuffer::<image::Luma<u16>, _>::from_raw(
        normalised.spec.width,
        normalised.spec.height,
        pixels,
    )
    .ok_or_else(|| CoreError::Image("pixel buffer size mismatch".into()))?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| CoreError::Image(e.to_string()))
}

/// Write an 8-bit greyscale PNG from 0..1 values (clamped).
pub fn write_png8(normalised: &Grid, path: &Path) -> Result<()> {
    let pixels: Vec<u8> = normalised
        .data
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect();
    let img = image::ImageBuffer::<image::Luma<u8>, _>::from_raw(
        normalised.spec.width,
        normalised.spec.height,
        pixels,
    )
    .ok_or_else(|| CoreError::Image("pixel buffer size mismatch".into()))?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| CoreError::Image(e.to_string()))
}

/// Write an RGBA PNG of a colour map, sRGB as stored: 16 bits if deep, else 8.
pub fn write_png_rgba(color: &ColorGrid, deep: bool, path: &Path) -> Result<()> {
    let (w, h) = (color.spec.width, color.spec.height);
    let mismatch = || CoreError::Image("pixel buffer size mismatch".into());
    let saved = if deep {
        let px: Vec<u16> = color
            .data
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)
            .collect();
        image::ImageBuffer::<image::Rgba<u16>, _>::from_raw(w, h, px)
            .ok_or_else(mismatch)?
            .save_with_format(path, image::ImageFormat::Png)
    } else {
        let px: Vec<u8> = color
            .data
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect();
        image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(w, h, px)
            .ok_or_else(mismatch)?
            .save_with_format(path, image::ImageFormat::Png)
    };
    saved.map_err(|e| CoreError::Image(e.to_string()))
}

/// Write an RGBA 32-bit float EXR of a colour map, values as stored: colours
/// stay sRGB-encoded, so packed weights and normal maps keep their exact values.
pub fn write_exr32_rgba(color: &ColorGrid, path: &Path) -> Result<()> {
    use exr::prelude::*;
    let size = (color.spec.width as usize, color.spec.height as usize);
    let channels: SmallVec<[AnyChannel<FlatSamples>; 4]> = ["R", "G", "B", "A"]
        .iter()
        .enumerate()
        .map(|(c, name)| {
            let data = color.data.chunks(4).map(|px| px[c]).collect();
            AnyChannel::new(*name, FlatSamples::F32(data))
        })
        .collect();
    let layer = Layer::new(
        size,
        LayerAttributes::named("colour"),
        Encoding {
            compression: Compression::ZIP16,
            blocks: Blocks::ScanLines,
            line_order: LineOrder::Increasing,
        },
        AnyChannels::sort(channels),
    );
    Image::from_layer(layer)
        .write()
        .non_parallel()
        .to_file(path)
        .map_err(|e| CoreError::Image(e.to_string()))
}

/// Make a string safe for use in a file name.
pub fn sanitise(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    s.trim_matches('_').to_string()
}

/// Request to export one node output.
pub struct ExportRequest<'a> {
    pub node: &'a str,
    pub port: &'a str,
    pub resolution: u32,
    /// Output folder; relative paths are resolved against `EvalOptions::base_dir`.
    pub folder: &'a Path,
    pub formats: &'a [ExportFormat],
}

/// File name without extension: `{node}_{output}_{res}`, e.g. `fbm-n_0003_out_2048`.
fn output_basename(
    project: &Project,
    registry: &NodeRegistry,
    node: &str,
    port: &str,
    resolution: u32,
) -> String {
    let label = project
        .graph
        .node(node)
        .and_then(|n| registry.schema(&n.type_id))
        .map(|s| s.label.clone())
        .unwrap_or_default();
    format!("{}-{}_{}_{}", sanitise(&label), node, sanitise(port), resolution)
}

fn resolve_folder(folder: &Path, base_dir: Option<&Path>) -> Result<PathBuf> {
    if folder.as_os_str().is_empty() {
        return Err(CoreError::Project("no output folder chosen".into()));
    }
    match base_dir {
        _ if folder.is_absolute() => Ok(folder.to_path_buf()),
        Some(dir) => Ok(dir.join(folder)),
        None => Err(CoreError::Project(format!(
            "the output folder '{}' is relative: save the project first, or choose a full path",
            folder.display()
        ))),
    }
}

/// Evaluate a node output at build resolution and write it in every requested
/// format, plus `build.json`. Returns the paths written.
pub fn export_node(
    project: &Project,
    registry: &NodeRegistry,
    req: &ExportRequest,
    opts: &EvalOptions,
) -> Result<Vec<PathBuf>> {
    let outputs = [(req.node.to_string(), req.port.to_string(), req.formats.to_vec())];
    write_outputs(
        project,
        registry,
        &outputs,
        req.resolution,
        req.folder,
        None,
        opts,
    )
}

/// Build every output marked for export (`project.exports`) at `resolution`
/// into `folder`, with one `build.json` listing them all. Shared upstream
/// nodes are computed once when `opts.cache` is set. Images are written as
/// tile files if the project's build settings ask for them.
pub fn build_marked(
    project: &Project,
    registry: &NodeRegistry,
    resolution: u32,
    folder: &Path,
    opts: &EvalOptions,
) -> Result<Vec<PathBuf>> {
    // Group formats by output, keeping the (sorted) mark order.
    let mut outputs: Vec<(String, String, Vec<ExportFormat>)> = Vec::new();
    for e in &project.exports {
        let Some(format) = ExportFormat::parse(&e.format) else {
            return Err(CoreError::Project(format!(
                "unknown export format '{}'",
                e.format
            )));
        };
        match outputs.iter_mut().find(|o| o.0 == e.node && o.1 == e.port) {
            Some(o) => o.2.push(format),
            None => outputs.push((e.node.clone(), e.port.clone(), vec![format])),
        }
    }
    if outputs.is_empty() {
        return Err(CoreError::Project(
            "nothing is marked for export: mark a node's output in its settings first".into(),
        ));
    }
    let tiles = project.build.file_tiles.as_ref();
    write_outputs(project, registry, &outputs, resolution, folder, tiles, opts)
}

/// Images of at most this many values are written from memory, as untiled
/// builds write them; bigger ones are streamed from disk.
const IN_MEMORY_VALUES: u64 = 1 << 26;

/// One built output, before it is written.
enum Built {
    /// Held in memory: untiled builds, and point sets.
    Value(Value),
    /// A tiled build's image, put back together on disk.
    Raw {
        image: RawImage,
        ty: PortType,
        range: (f32, f32),
    },
}

impl Built {
    fn is_points(&self) -> bool {
        matches!(self, Built::Value(Value::Points(_)))
    }

    /// Write `rect` of the build (all of it if `None`) as `format`.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        full: GridSpec,
        rect: Option<Rect>,
        world: &World,
        format: ExportFormat,
        path: &Path,
        node: &str,
        port: &str,
    ) -> Result<ExportedFile> {
        let window = rect.map(|r| full.window(r.x0, r.y0, r.w, r.h));
        let mut file = match self {
            Built::Value(v) => match window {
                Some(w) => write_value(&v.crop(w), world, format, path, node, port)?,
                None => write_value(v, world, format, path, node, port)?,
            },
            Built::Raw { image, ty, range } => {
                let r = rect.unwrap_or(Rect {
                    x0: 0,
                    y0: 0,
                    w: full.width,
                    h: full.height,
                });
                let spec = window.unwrap_or(full);
                let values = r.w as u64 * r.h as u64 * image.channels as u64;
                if values <= IN_MEMORY_VALUES {
                    let data = image.read(r)?;
                    let value = match ty {
                        PortType::ColorMap => Value::ColorMap(Arc::new(ColorGrid { spec, data })),
                        PortType::Mask => Value::Mask(Arc::new(Grid { spec, data })),
                        _ => Value::Heightfield(Arc::new(Grid { spec, data })),
                    };
                    write_value(&value, world, format, path, node, port)?
                } else {
                    let (lo, hi) = match rect {
                        Some(r) => min_max_of(image, r)?,
                        None => *range,
                    };
                    let heights = *ty == PortType::Heightfield;
                    let map = |v: f32| if heights { world.normalise(v) } else { v };
                    match format {
                        ExportFormat::Exr32 => write_exr_streamed(image, r, path)?,
                        ExportFormat::Png16 => write_png_streamed(image, r, true, &map, path)?,
                        ExportFormat::Png8 => write_png_streamed(image, r, false, &map, path)?,
                        ExportFormat::Csv | ExportFormat::Json => {
                            return Err(CoreError::Project(format!(
                                "{node}.{port}: {} can't be written as {}",
                                ty.key(),
                                format.key()
                            )));
                        }
                    }
                    ExportedFile {
                        file: file_name(path),
                        node: node.into(),
                        port: port.into(),
                        data: *ty,
                        format,
                        encoding: encoding(format, *ty),
                        data_min: lo,
                        data_max: hi,
                        points: None,
                        species: None,
                        tile: None,
                        tile_origin: None,
                        tile_size: None,
                    }
                }
            }
        };
        if let Some(r) = rect {
            file.tile_origin = Some([r.x0, r.y0]);
            file.tile_size = Some([r.w, r.h]);
        }
        Ok(file)
    }
}

/// Lowest and highest value in `rect` of `image`, read a band at a time.
fn min_max_of(image: &RawImage, rect: Rect) -> Result<(f32, f32)> {
    let mut range = (f32::INFINITY, f32::NEG_INFINITY);
    let mut y = 0;
    while y < rect.h {
        let h = 256.min(rect.h - y);
        for v in image.read(Rect {
            y0: rect.y0 + y,
            h,
            ..rect
        })? {
            range = (range.0.min(v), range.1.max(v));
        }
        y += h;
    }
    Ok(range)
}

/// Evaluate every output over the whole world: at once for build grids up
/// to [`TILED_ABOVE`], in tiles (put back together in `folder`) above.
fn build_values(
    project: &Project,
    registry: &NodeRegistry,
    outputs: &[(String, String, Vec<ExportFormat>)],
    full: GridSpec,
    folder: &Path,
    opts: &EvalOptions,
) -> Result<Vec<Built>> {
    if full.width <= TILED_ABOVE && full.height <= TILED_ABOVE {
        let n = outputs.len() as f32;
        let mut built = Vec::with_capacity(outputs.len());
        for (k, (node, port, _)) in outputs.iter().enumerate() {
            let scaled = |f: f32| {
                if let Some(p) = opts.progress {
                    p((k as f32 + f) / n);
                }
            };
            let evaluated = evaluate_node(
                &project.graph,
                registry,
                &project.world,
                full,
                node,
                &EvalOptions {
                    progress: Some(&scaled),
                    ..*opts
                },
            )?;
            let value = evaluated
                .get(port)
                .cloned()
                .ok_or_else(|| CoreError::PortNotFound {
                    node: node.clone(),
                    port: port.clone(),
                })?;
            built.push(Built::Value(value));
        }
        return Ok(built);
    }

    let targets: Vec<(String, String)> = outputs.iter().map(|(n, p, _)| (n.clone(), p.clone())).collect();
    std::fs::create_dir_all(folder)?;
    type Assembly = (RawImage, PortType, (f32, f32));
    let mut images: Vec<Option<Assembly>> = (0..outputs.len()).map(|_| None).collect();
    let mut points: Vec<Option<PointSet>> = vec![None; outputs.len()];
    evaluate_tiled(
        &project.graph,
        registry,
        &project.world,
        full,
        &targets,
        project.build.tile_size.max(MIN_TILE_SIZE),
        opts,
        &mut |tile, node, port, value| {
            let k = targets
                .iter()
                .position(|(n, p)| n == node && p == port)
                .expect("a requested output");
            if let Value::Points(p) = &value {
                let all = points[k].get_or_insert_with(|| {
                    let mut all = PointSet::new(full, "");
                    all.species = p.species.clone();
                    all
                });
                for q in p.iter() {
                    all.push(q);
                }
                return Ok(());
            }
            if images[k].is_none() {
                let ty = value.port_type();
                let channels = if ty == PortType::ColorMap { 4 } else { 1 };
                let path = folder.join(format!(".ots-build-{}-{k}.raw", std::process::id()));
                let image = RawImage::create(path, full.width, full.height, channels)?;
                images[k] = Some((image, ty, (f32::INFINITY, f32::NEG_INFINITY)));
            }
            let (image, _, range) = images[k].as_mut().expect("made above");
            let samples = value.samples();
            for &v in samples {
                *range = (range.0.min(v), range.1.max(v));
            }
            image.write_window(tile.core, samples)
        },
    )?;
    Ok(images
        .into_iter()
        .zip(points)
        .map(|(image, points)| match (image, points) {
            (Some((image, ty, range)), _) => Built::Raw { image, ty, range },
            (None, Some(p)) => Built::Value(Value::Points(Arc::new(p))),
            (None, None) => unreachable!("every tile hands over every output"),
        })
        .collect())
}

/// Smallest tile size a build uses, whatever the settings say.
const MIN_TILE_SIZE: u32 = 64;

fn write_outputs(
    project: &Project,
    registry: &NodeRegistry,
    outputs: &[(String, String, Vec<ExportFormat>)],
    resolution: u32,
    folder: &Path,
    tiles: Option<&FileTiles>,
    opts: &EvalOptions,
) -> Result<Vec<PathBuf>> {
    let full = GridSpec::build_world(&project.world, resolution)?;
    let folder = resolve_folder(folder, opts.base_dir)?;
    // Refuse mismatched formats (e.g. a heightfield as CSV) before computing anything.
    for (node, port, formats) in outputs {
        let ty = project
            .graph
            .node(node)
            .and_then(|n| registry.schema(&n.type_id))
            .and_then(|s| s.output(port))
            .map(|o| o.ty);
        if let Some(ty) = ty
            && let Some(f) = formats.iter().find(|f| !f.supports(ty))
        {
            return Err(CoreError::Project(format!(
                "{node}.{port} is a {} and can't be exported as {}",
                ty.key().replace('_', " "),
                f.key()
            )));
        }
    }
    if let Some(t) = tiles
        && (t.size < 2 || !t.pattern.contains("{x}") || !t.pattern.contains("{y}"))
    {
        return Err(CoreError::Project(
            "tile files need a size of at least 2 and a name pattern with {x} and {y}".into(),
        ));
    }

    // Evaluate everything before writing anything, so a failure leaves no
    // half-finished build behind.
    let built = build_values(project, registry, outputs, full, &folder, opts)?;

    std::fs::create_dir_all(&folder)?;
    let mut written = Vec::new();
    let mut files = Vec::new();
    let tile_list = tiles.map(|t| file_tiles(full.width, full.height, t.size));
    for ((node, port, formats), built) in outputs.iter().zip(&built) {
        let base = output_basename(project, registry, node, port, resolution);
        for &format in formats {
            match (&tile_list, tiles) {
                (Some(list), Some(t)) if !built.is_points() => {
                    for &(index, rect) in list {
                        let name = t
                            .pattern
                            .replace("{name}", &base)
                            .replace("{x}", &index[0].to_string())
                            .replace("{y}", &index[1].to_string());
                        let path = folder.join(format!("{name}.{}", format.extension()));
                        let mut file =
                            built.write(full, Some(rect), &project.world, format, &path, node, port)?;
                        file.tile = Some(index);
                        files.push(file);
                        written.push(path);
                    }
                }
                _ => {
                    let path = folder.join(format!("{base}.{}", format.extension()));
                    files.push(built.write(full, None, &project.world, format, &path, node, port)?);
                    written.push(path);
                }
            }
        }
    }
    let mut info = BuildInfo::new(&project.world, &full, files);
    if let Some(gpu) = opts.gpu {
        info.compute = format!("gpu: {}", gpu.name());
    }
    if let (Some(t), Some(list)) = (tiles, &tile_list) {
        let last = list.last().map_or([0, 0], |(i, _)| *i);
        info.file_tiles = Some(TileFilesInfo {
            size: t.size,
            columns: last[0] + 1,
            rows: last[1] + 1,
            pattern: t.pattern.clone(),
            even: tiles_even(full.width, t.size) && tiles_even(full.height, t.size),
        });
    }
    if full.width > TILED_ABOVE || full.height > TILED_ABOVE {
        info.computed_in_tiles = Some(project.build.tile_size.max(MIN_TILE_SIZE));
    }
    let info_path = folder.join("build.json");
    std::fs::write(&info_path, serde_json::to_string_pretty(&info)? + "\n")?;
    written.push(info_path);
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Height in metres that Unreal gives a 16-bit value, per its landscape docs.
    fn unreal_height_m(hints: &UnrealHints, v: u16) -> f64 {
        (hints.z_location + (v as f64 - 32768.0) / 128.0 * hints.z_scale) / 100.0
    }

    #[test]
    fn unreal_hints_reproduce_png_heights() {
        let world = World {
            height_range_m: [-120.0, 2280.0],
            ..World::default()
        };
        let hints = UnrealHints::new(&world, 8.0);
        assert_eq!(hints.xy_scale, 800.0);
        for v in [0u16, 1, 32767, 32768, 65534, 65535] {
            let expected = world.denormalise(v as f32 / 65535.0) as f64;
            let got = unreal_height_m(&hints, v);
            assert!(
                (got - expected).abs() < 1e-3,
                "v={v}: unreal {got} m, png {expected} m"
            );
        }
    }

    #[test]
    fn unreal_z_scale_100_is_512_metres() {
        let world = World {
            height_range_m: [0.0, 512.0],
            ..World::default()
        };
        let hints = UnrealHints::new(&world, 1.0);
        // 512 m over 65535 steps rather than Unreal's nominal 65536.
        assert!((hints.z_scale - 100.0).abs() < 0.002, "{}", hints.z_scale);
    }
}
