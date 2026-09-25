//! Export writers: EXR 32-bit float, PNG 16-bit, and the `build.json` sidecar.
//!
//! Image orientation: row 0 of every exported image is world Y = 0 and column 0
//! is world X = 0 (top-left of the image = world origin). Unreal, Godot and
//! Blender all read heightmaps this way.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::eval::{EvalOptions, evaluate_node};
use crate::grid::{Grid, GridSpec};
use crate::node::{NodeRegistry, PortType, Value};
use crate::project::Project;
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
}

impl ExportFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "exr32" | "exr" => Some(Self::Exr32),
            "png16" | "png" => Some(Self::Png16),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Exr32 => "exr32",
            Self::Png16 => "png16",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Exr32 => "exr",
            Self::Png16 => "png",
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
    /// Actual min/max of the exported data, in its natural unit (metres or 0..1).
    pub data_min: f32,
    pub data_max: f32,
}

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
    pub files: Vec<ExportedFile>,
    pub unreal: UnrealHints,
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
            files,
            unreal: UnrealHints::new(world, cell[0]),
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
    let grid = value.grid();
    let (lo, hi) = grid.min_max();
    let ty = value.port_type();
    let encoding = match (format, ty) {
        (ExportFormat::Exr32, PortType::Heightfield) => "metres".to_string(),
        (ExportFormat::Exr32, PortType::Mask) => "0..1".to_string(),
        (ExportFormat::Png16, PortType::Heightfield) => "0..65535 = height_range_m min..max".to_string(),
        (ExportFormat::Png16, PortType::Mask) => "0..65535 = 0..1".to_string(),
    };
    match format {
        ExportFormat::Exr32 => write_exr32(grid, path)?,
        ExportFormat::Png16 => {
            let normalised = match ty {
                PortType::Heightfield => grid.map(|h| world.normalise(h)),
                PortType::Mask => (**grid).clone(),
            };
            write_png16(&normalised, path)?
        }
    }
    Ok(ExportedFile {
        file: path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default(),
        node: node.into(),
        port: port.into(),
        data: ty,
        format,
        encoding,
        data_min: lo,
        data_max: hi,
    })
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
    write_outputs(project, registry, &outputs, req.resolution, req.folder, opts)
}

/// Build every output marked for export (`project.exports`) at `resolution`
/// into `folder`, with one `build.json` listing them all. Shared upstream
/// nodes are computed once when `opts.cache` is set.
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
    write_outputs(project, registry, &outputs, resolution, folder, opts)
}

fn write_outputs(
    project: &Project,
    registry: &NodeRegistry,
    outputs: &[(String, String, Vec<ExportFormat>)],
    resolution: u32,
    folder: &Path,
    opts: &EvalOptions,
) -> Result<Vec<PathBuf>> {
    let spec = GridSpec::full_world(&project.world, resolution)?;
    let folder = resolve_folder(folder, opts.base_dir)?;
    // Evaluate everything before writing anything, so a failure leaves no
    // half-finished build behind.
    let n = outputs.len() as f32;
    let mut values = Vec::with_capacity(outputs.len());
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
            spec,
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
        values.push(value);
    }

    std::fs::create_dir_all(&folder)?;
    let mut written = Vec::new();
    let mut files = Vec::new();
    for ((node, port, formats), value) in outputs.iter().zip(&values) {
        let base = output_basename(project, registry, node, port, resolution);
        for &format in formats {
            let path = folder.join(format!("{base}.{}", format.extension()));
            files.push(write_value(value, &project.world, format, &path, node, port)?);
            written.push(path);
        }
    }
    let info = BuildInfo::new(&project.world, &spec, files);
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
