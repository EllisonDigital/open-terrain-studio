// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 EllisonDigital
//! Real export-writer fixture, with independent raw-f32 reference files.
use std::{path::Path, sync::Arc};
use terrain_core::export::{BuildInfo, ExportFormat, write_value};
use terrain_core::{Grid, GridSpec, Value, World};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let folder = Path::new(args.get(1).expect("output folder"));
    let resolution: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(129);
    std::fs::create_dir_all(folder).unwrap();
    let world = World {
        size_m: [2048.0, 1024.0],
        height_range_m: [-120.0, 2280.0],
        ..World::default()
    };
    let spec = GridSpec::full_world(&world, resolution).unwrap();
    let mut files = Vec::new();
    for (node, port, value) in [
        (
            "terrain_a",
            "out",
            Value::Heightfield(Arc::new(Grid::from_fn(spec, |x, y| {
                (-80.0 + x * 0.73 + y * 0.31 + x * y * 0.00003) as f32
            }))),
        ),
        (
            "terrain_b",
            "height",
            Value::Heightfield(Arc::new(Grid::from_fn(spec, |x, y| {
                (100.0 + x * 0.21 - y * 0.13) as f32
            }))),
        ),
        (
            "mask_a",
            "slope",
            Value::Mask(Arc::new(Grid::from_fn(spec, |x, y| {
                (0.1 + 0.4 * x / 2048.0 + 0.3 * y / 1024.0) as f32
            }))),
        ),
        (
            "mask_a",
            "aspect",
            Value::Mask(Arc::new(Grid::from_fn(spec, |x, y| {
                (0.9 - 0.3 * x / 2048.0 - 0.4 * y / 1024.0) as f32
            }))),
        ),
    ] {
        let stem = format!("{node}_{port}");
        let raw: Vec<u8> = value.grid().data.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(folder.join(format!("{stem}.f32")), raw).unwrap();
        for format in [ExportFormat::Exr32, ExportFormat::Png16] {
            files.push(
                write_value(
                    &value,
                    &world,
                    format,
                    &folder.join(format!("{stem}.{}", format.extension())),
                    node,
                    port,
                )
                .unwrap(),
            );
        }
    }
    let info = BuildInfo::new(&world, &spec, files);
    std::fs::write(
        folder.join("build.json"),
        serde_json::to_string_pretty(&info).unwrap(),
    )
    .unwrap();
    println!(
        "Exported {resolution}² real EXR/PNG heightfields + masks to {}",
        folder.display()
    );
}
