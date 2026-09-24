# OpenTerrainStudio — Roadmap

This document tracks milestones, risks and decisions. The technical design lives in [ARCHITECTURE.md](ARCHITECTURE.md). Each milestone here matches a GitLab milestone in the `EllisonDigital/open-terrain-studio` subgroup.

---

## 1. Milestones

Ten milestones take the project from an empty repo to a stable v1.0; each one ends with a downloadable app that does something useful on its own.

| Version | Name | Usable result at the end | Relative size |
| --- | --- | --- | --- |
| v0.1 | Foundation | Noise terrain in a node graph, 3D preview, save/load, heightmap export | L |
| v0.2 | Shaping toolkit | Build real landforms from primitives, noise and masks; undo, caching | M |
| v0.3 | Erosion | Eroded, natural-looking terrain with flow/wear/deposition maps | L |
| v0.4 | GPU acceleration | Same results, interactive speed at 1K–2K preview | L |
| v0.5 | Water and hydrology | Rivers, lakes, sea, snow, and their masks | M |
| v0.6 | Colour and texturing | Colour maps, splat/weight maps, normal maps for engines | M |
| v0.7 | Vegetation | Trees/Shrubs/Grass populations, density masks and point export | L |
| v0.8 | Production scale | Tiled 16K builds, mesh export, portals, groups, presets | L |
| v0.9 | Interop and polish | Engine presets, import helpers, docs, UX polish | M |
| v1.0 | Stable release | Frozen file format, installers, full docs | M |

Erosion comes early (v0.3) because it is what makes terrain look like Gaea output, and it stress-tests the architecture on a real workload.

### v0.1 — Foundation

**Status (24 Sep 2026):** complete in the repository; release pending. All deliverables are built and tested, including packaged apps: export presets for Windows, Linux and macOS (universal), `scripts/package.sh` / `package.ps1`, the Godot version pinned in `.godot-version`, and a CI pipeline that publishes a GitLab Release on `v*` tags. Still to do on GitLab: protect `main`, create the milestones and labels, and push the `v0.1.0` tag. The release pipeline hasn't run on GitLab yet.

Exit criteria, as checked:

- *Download, build, save, reopen, export:* the packaged Linux app was launched, previewed the starter graph and exported a 1009² EXR and PNG through the export dialog. The Windows and macOS exports were produced with placeholder libraries only, so they are untested on those systems until the release pipeline builds the real ones.
- *EXR imports correctly into Blender at the right scale:* checked in Blender 5.2 LTS, where every vertex matched its EXR pixel ([export guides](export-guides.md)).
- *Byte-identical exports:* two exports from two separate builds gave identical EXR and PNG files, but on one machine. A second machine hasn't been compared yet.
- *1,024² preview under 1 s:* 37 ms (fBm → Levels) on a 28-thread i7-14700HX, and 47 ms limited to 4 threads.

**Goal:** prove the whole pipeline end to end with the smallest possible feature set: node graph → Rust core → heightmap → Godot viewport → exported file.

**Deliverables**

- Cargo workspace with `terrain-core`, `terrain-nodes`, `terrain-godot`; Godot 4 project in `app/`; GitLab CI building all three OSes.
- `LICENSE-MIT`, `LICENSE-APACHE`, `CONTRIBUTING.md` (with DCO sign-off), protected `main` branch.
- `Heightfield` / `Mask` types with world extent, cell size, metres (ARCHITECTURE.md §4).
- `NodeKind` trait, `NodeSchema`, graph model, simple evaluator (no caching yet), seeding.
- Nodes: Constant, Perlin, Simplex, fBm (octaves, lacunarity, gain, feature size in metres), Combine (add, multiply, max, min, blend), Levels.
- Godot UI: main window with graph panel (GraphEdit), inspector auto-built from the schema, 3D viewport, top menu.
- Viewport: shader-displaced grid from a height texture, sun, sky, orbit camera.
- Evaluation on a background thread with a progress bar; UI never freezes.
- Project save/load (`.otstudio` JSON, format version 1).
- Export of the viewed node as 32-bit EXR and 16-bit PNG, plus `build.json`.

**Exit criteria**

- A new user can download the app, create Perlin → Levels, see it in 3D, save, reopen, and export an EXR that imports correctly into Blender at the right scale.
- Same project + seed gives byte-identical exports on two machines.
- 1,024² preview updates in under 1 second on a mid-range CPU.

**Out of scope:** undo, caching, erosion, GPU, masks, colour.

### v0.2 — Shaping toolkit

**Goal:** enough nodes and editor comfort to build believable large-scale landforms by hand.

**Deliverables**

- Primitives: Gradient, Cone, Hemisphere, Shape, File (import a heightmap).
- Noise: Voronoi/Cellular, Ridged, Billow, Domain warp, Value.
- Terrain: Mountain, Ridge, Canyon, Crater, Plateau, Dunes (first versions).
- Adjust: Curve, Clamp, Invert, Terrace, Blur, Sharpen, Transform (move/rotate/scale), Warp.
- Data/masks: Height mask, Slope, Curvature, Aspect, Select range, Distance.
- Masks can drive parameters (parameter → input port).
- Cache with dirty propagation; only changed branches recompute.
- Undo/redo for all graph and parameter edits.
- 2D map view with value readout; mask-overlay view mode in 3D.
- Mark any output for export; Build tab listing all marked outputs; build resolution setting.
- Resolution-independence regression tests running in CI.

**Exit criteria:** recreate three reference landforms (alpine range, canyon, dune field) using only built-in nodes; editing a downstream node never recomputes upstream nodes; undo works for 100+ steps.

### v0.3 — Erosion

**Goal:** natural-looking eroded terrain, the core of the Gaea look, on the CPU.

**Deliverables**

- Hydraulic erosion node (grid-based water/sediment simulation), parameters in physical terms: duration, rock softness, sediment capacity, deposition, evaporation, downcutting.
- Multi-output: Height, Flow, Wear, Deposition, Sediment.
- Thermal erosion node (talus angle in degrees, duration), outputs Height and Debris/Talus mask.
- Mask input for spatially varying erosion strength.
- Stratification/Rock hardness as a simple input map that erosion respects.
- Progress reporting and cancellation mid-simulation.
- Multithreaded CPU implementation (Rayon) with deterministic results.
- Erosion results at 512² and 2,048² look the same (tested).

**Exit criteria:** 1,024² hydraulic erosion completes in under 30 seconds on an 8-core CPU; flow and deposition masks are usable as texture masks in Unreal; results are deterministic across thread counts.

### v0.4 — GPU acceleration

**Goal:** the v0.3 feature set at interactive speed, with results matching the CPU versions.

**Deliverables**

- RenderingDevice compute infrastructure in `terrain-godot`: shader loading, buffer/texture pools, dispatch helper, readback.
- GPU-resident cache entries; consecutive GPU nodes never round-trip through the CPU.
- GLSL kernels for: all noise nodes, Blur/Sharpen/Warp/Transform, Slope/Curvature, hydraulic erosion, thermal erosion.
- Long simulations split into short dispatches (no driver timeouts), with progress and cancel.
- CPU fallback and "Force CPU" setting; automatic CPU-only mode on the Compatibility renderer; per-node GPU/CPU indicator in the graph.
- GPU-vs-CPU tolerance tests for every kernel.
- Preview quality setting (512 / 1,024 / 2,048) and "auto-update vs manual update" toggle.

**Exit criteria:** 2,048² erosion in under 5 seconds on a mid-range GPU (e.g. RTX 3060 class); dragging a noise slider updates the 1,024² viewport at 10+ fps; all GPU tests within tolerance on NVIDIA, AMD and Intel.

### v0.5 — Water and hydrology

**Goal:** terrain-aware water features and the masks that describe them.

**Deliverables**

- Flow accumulation and flow direction (D8/D-infinity) nodes; watershed/basin mask.
- Rivers node: carves river channels from flow accumulation, width/depth in metres; outputs Height, River mask, Riverbank mask.
- Lakes node: fills depressions to a spill level; outputs Height, Water surface, Lake mask, Shore mask.
- Sea node: sea level in metres, coastline and shallow-water masks, simple coastal erosion.
- Snow node: accumulation by altitude, slope and aspect with melt; outputs Snow mask and snow-covered height.
- Water distance / wetness masks for later vegetation use.
- Viewport water surfaces for sea, lakes and rivers.

**Exit criteria:** a mountain → erosion → rivers → lakes → sea graph produces connected rivers that reach the sea, with river and lake masks exporting cleanly; snow sits convincingly on north-facing high slopes.

### v0.6 — Colour and texturing

**Goal:** produce everything an engine needs to texture the terrain.

All colour work lives in the dedicated **Colour** tab (ARCHITECTURE.md §5). Terrain outputs arrive there as portals, so colour changes never recompute the terrain.

**Deliverables**

- Colour graph tab, with portals from the Terrain tab.
- `ColorMap` type through the graph; colour view mode in the viewport.
- Colourise node: gradient mapped from height or any mask; a library of natural gradients (rock, sand, grass, snow) authored in-house.
- Blend colours by mask; layer stack node for quick material layering.
- Normal map node (OpenGL/DirectX convention), Occlusion/cavity mask, Angle/Aspect-based colouring.
- Splat/weight-map node: up to N material masks, normalised to sum to 1, packed RGBA or one PNG per layer.
- Satellite-style colour from a user-supplied image (import and remap).

**Exit criteria:** export height + 4-layer weight maps + normal map + colour map and set up a textured landscape in Unreal and Godot in under 10 minutes using the written guide.

### v0.7 — Vegetation

**Goal:** Gaea-style ecosystem populations with density masks and point export (ARCHITECTURE.md §8).

**Deliverables**

- Trees, Shrubs and Grass population nodes with growth/health, inhibitors and dead zones.
- `Occupied` input for chaining populations; populations avoid or intermingle.
- Outputs: Density, Points, Dead zones, Occupied.
- Poisson-disk point sampling with minimum spacing in metres, random rotation and scale ranges.
- Debris/Rocks node driven by thermal erosion talus and dead zones.
- Species preset files (YAML) with a starter set of ~15 temperate, alpine, desert and tropical presets, hosted in the `examples` project.
- Viewport vegetation preview with MultiMesh placeholders and a density cap; ecosystem data view.
- Export: per-population greyscale masks, RGBA packing, CSV/JSON points.

**Exit criteria:** three chained populations (pine, birch, shrubs) with visibly different habitats; 1M+ points generated in under 10 seconds; exported points reproduce the same placement in Godot and Unreal via a simple import script.

### v0.8 — Production scale

**Goal:** handle real production sizes and larger, more organised graphs.

**Deliverables**

- Tiled builds: world split into overlapping tiles, each evaluated and blended; builds up to 16,384² and beyond on 16 GB RAM machines.
- Tiled export with configurable tile size and naming patterns.
- Disk-spilling LRU cache with a size limit in settings.
- Mesh export: GLB and OBJ, full-res or decimated, optional tiling and LODs.
- Graph organisation: portals, groups/sub-graphs, frames with comments, node bypass, search-to-add node menu.
- Graph presets and templates ("Alpine valley", "Desert canyon", "Volcanic island") that open as editable graphs.
- Exposed parameters and variation builds (re-run a build with N different seeds).
- Background builds: keep editing while a full-resolution build runs.

**Exit criteria:** an 8,192² build with erosion, hydrology, colour and three vegetation populations finishes without running out of memory on a 16 GB machine; a 16,384² tiled build completes and tiles align seamlessly in Unreal World Partition.

### v0.9 — Interop and polish

**Goal:** make the app pleasant for new users and effortless to get data into engines.

**Deliverables**

- One-click engine presets for Unreal, Godot, Blender and generic (ARCHITECTURE.md §9).
- Import helpers: Unreal editor Python script, Godot editor plugin, Blender add-on, all reading `build.json`.
- UX pass: keyboard shortcuts, node thumbnails, tooltips from the schema, consistent icons, dark/light themes, recent projects, crash-safe autosave.
- In-app help panel per node (generated from node docs).
- User documentation site in the `docs` project, published with GitLab Pages: getting started, every node, export guides per engine, example projects.
- Performance pass guided by benchmarks; startup under 3 seconds.
- Opt-in crash reporting (local log + "copy report" button; no telemetry by default).

**Exit criteria:** a new user follows the getting-started guide and has an eroded, textured, vegetated terrain inside Unreal in under 30 minutes; outside testers complete the tutorial without help.

### v1.0 — Stable release

**Goal:** a dependable 1.0 people can build production work on.

**Deliverables**

- Project format frozen at a stable version, with migrations from every 0.x format tested.
- Node parameter names and behaviour frozen; future changes go through `type_version` migrations.
- Public beta (release candidates) with a bug-fix-only period.
- Signed installers for Windows and macOS, AppImage/Flatpak for Linux, published through GitLab Releases.
- Complete docs, example project library, and a contributor guide for writing new nodes.
- Release notes and a public roadmap for 1.x (candidates: CLI, wgpu backend, plugins, more simulations, AI-assisted presets).

**Exit criteria:** no known crash or data-loss bugs; all golden, determinism and GPU tests green on three OSes and three GPU vendors; projects from v0.5 onwards open correctly.

---

## 2. Risks and decisions

| Risk | Impact | Mitigation |
| --- | --- | --- |
| Erosion quality falls short of Gaea's look | Users see it as a toy | Start erosion in v0.3, iterate against reference images, study published erosion research, invite artist feedback early |
| gdext breaking changes | Upgrade work | Pin the version; keep all gdext code inside `terrain-godot` |
| GPU driver differences | Inconsistent results, crashes | CPU reference, tolerance tests on three vendors, short dispatches |
| Memory at 8K+ | Crashes on large builds | Tile-aware data model from v0.1, LRU cache, tiling in v0.8 |
| Godot GraphEdit limits at 200+ nodes | Sluggish editor | Profile early; fall back to a custom graph control if needed |
| Windows/macOS CI runners unavailable on the GitLab plan | No automated builds for those OSes | Self-hosted GitLab runner on a local Windows/Mac machine for release builds |
| Scope creep | Never reaching 1.0 | Every milestone has exit criteria; new ideas go to the 1.x roadmap |
| Solo-developer bandwidth | Slow progress | Clear node API and "add a node" guide to attract contributors; AI agents for well-specified, tested tasks |

**Decided (24 Sep 2026)**

- Name: **OpenTerrainStudio** (written "Open Terrain Studio" in prose), project extension `.otstudio`.
- Owner: EllisonDigital, which holds the copyright and the "OpenTerrainStudio" trademark.
- Hosting: GitLab subgroup `EllisonDigital/open-terrain-studio`, main repo `open-terrain-studio`; `examples` (v0.7) and `docs` (v0.9) added later. Optional read-only GitHub mirror.
- Crates: internal workspace crates are `terrain-core`, `terrain-nodes` and `terrain-godot` (not published). Any crate published to crates.io uses the `openterrainstudio-` prefix (`ots-core` is already taken).
- Licence: dual MIT / Apache-2.0.
- Contributions: Developer Certificate of Origin (DCO) sign-off, no CLA.
- Old hardware: supported via the Compatibility renderer with CPU-only compute.
- Colour: a separate Colour graph tab, used only for applying colour.
- Minimum GPU and RAM: set later from benchmark results.
