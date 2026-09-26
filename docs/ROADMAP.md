# OpenTerrainStudio — Roadmap

This document tracks milestones, risks and decisions. The technical design lives in [ARCHITECTURE.md](ARCHITECTURE.md). Milestones are tracked in the `EllisonDigital/open-terrain-studio` GitHub repository.

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

**Status:** complete in the repository; release pending. All deliverables are built and tested, including packaged apps: export presets for Windows, Linux and macOS (universal), `scripts/package.sh` / `package.ps1`, the Godot version pinned in `.godot-version`, and a GitHub Actions workflow that publishes a GitHub Release on version tags. Still to do on GitHub: protect `main`, create the milestones and labels, and push a release tag. The release workflow has not been verified on a version tag yet.

Exit criteria, as checked:

- *Download, build, save, reopen, export:* the packaged Linux app was launched, previewed the starter graph and exported a 1009² EXR and PNG through the export dialog. The Windows and macOS exports were produced with placeholder libraries only, so they are untested on those systems until the release pipeline builds the real ones.
- *EXR imports correctly into Blender at the right scale:* checked in Blender 5.2 LTS, where every vertex matched its EXR pixel ([export guides](export-guides.md)).
- *Byte-identical exports:* two exports from two separate builds gave identical EXR and PNG files, but on one machine. A second machine hasn't been compared yet.
- *1,024² preview under 1 s:* 37 ms (fBm → Levels) on a 28-thread i7-14700HX, and 47 ms limited to 4 threads.

**Goal:** prove the whole pipeline end to end with the smallest possible feature set: node graph → Rust core → heightmap → Godot viewport → exported file.

**Deliverables**

- Cargo workspace with `terrain-core`, `terrain-nodes`, `terrain-godot`; Godot 4 project in `app/`; GitHub Actions building all three OSes for releases.
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

**Status (25 Sep 2026):** complete on the `v0.2-shaping` branch; not merged or released yet. All deliverables are built and tested with the Linux app on Godot 4.7.2 and 4.6.2. Windows and macOS have not been run, as for v0.1.

Exit criteria, as checked:

- *Three reference landforms from built-in nodes only:* the alpine range, canyon and dune field in `app/examples/` (*File → Open Example*, included in the packaged app). A test loads each one without warnings and builds its marked outputs. They were judged by eye, not compared with reference photos. The terrain nodes are first versions, and erosion (v0.3) should make them look far more natural.
- *Editing a downstream node never recomputes upstream nodes:* `editing_downstream_never_recomputes_upstream` counts computed nodes after edits at the end, the middle and the start of a chain. In the alpine example at 1,024², a cold preview takes 160 ms; after editing its last node it takes 6.7 ms, with 1 of 13 nodes recomputed (28-thread i7-14700HX; 514 ms and 17.8 ms on 4 threads).
- *Undo works for 100+ steps:* a unit test undoes and redoes 150 steps; the UI test undoes and redoes 120 parameter edits in the running app.

Differences from the plan:

- Golden tests pin a hash of every node's output (`tests/golden/node_hashes.json`) instead of storing reference EXRs in Git LFS.
- Resolution tests compare 513² with 2,049², so every fourth high-resolution sample lands exactly on a low-resolution one.
- The cache is memory-only (1 GiB, least recently used first); spilling to disk comes with large builds (v0.8). Independent branches still run one after another, each node using all cores.

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

**Status (25 Sep 2026):** in progress on the `v0.3-integration` branch (the CPU erosion from `v0.3-erosion` on top of v0.2). Not merged or released.

- *Done and tested:* Hydraulic Erosion (Height, Flow, Wear, Deposition, Sediment), Thermal Erosion (Height, Debris/Talus), Rock Hardness, strength and hardness inputs, progress and cancellation inside a simulation (a cancelled result is never cached), Rayon with bit-identical results for 1 and 8 workers, and 512²/2,048² comparisons of the erosion change and every mask. The editor gained an Output picker for nodes with several outputs, and masks drape over their node's own eroded Height. There is an *Eroded strata* example.
- *Exit criterion, speed:* 1,024² hydraulic, 60 s simulated, 8 workers: 2.4 s (i7-14700HX), far below 30 s.
- *Exit criterion, determinism:* met across thread counts on one machine; not yet compared on a second machine or OS.
- *Exit criterion, Unreal masks:* PNG/EXR mask values round-trip exactly, but nothing has been imported into Unreal.
- **Not met: the goal, natural-looking erosion.** On the alpine example, hydraulic erosion planes slopes smooth and fills valleys instead of cutting branching channels, at every setting tried (rain 0.002–0.05 m/s, 60–600 s, capacity 2–8, deposition 0.1–0.5), with some grid-aligned artefacts along crests. Uniform rain over a grid water sheet makes erosion act like diffusion. Thermal erosion behaves as intended but its defaults remove hundreds of metres from steep peaks in 60 s. The solver needs erosion that concentrates with drained area before v0.3 can be called done.

**Update (25 Sep 2026, later):** Hydraulic Erosion now uses a stream-power landscape-evolution model (drainage-area routing, implicit incision, sediment transport and hillslope creep; see `docs/erosion.md`). On the alpine example it cuts branching V-shaped valleys between sharp ridges and fills basins with sediment. Flow is a river network and Deposition marks valley fills, so the "natural-looking" goal is met by eye; there is still no artist review or reference-photo comparison. It simulates on a fixed 8 m grid, so previews and builds get the same rivers. 1,024² over the default 8 km world takes 10.3 s (still under 30 s). Still open for the milestone: comparing on a second machine or OS, and importing the masks into Unreal.

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

**Status (25 Sep 2026):** in progress on the `v0.4-gpu` branch. Not merged or released. Details and measurements are in [gpu.md](gpu.md).

- *Done and tested:* GPU compute through a local RenderingDevice on its own thread; GPU-resident results that the next GPU node reads without a round trip; kernels for every noise node, Blur, Sharpen, Transform, Warp, Slope, Aspect (not in the plan, same kernel as Slope), Curvature and Thermal Erosion; batched submissions for long simulations, with progress and cancellation between batches; CPU fallback on any GPU error; Force CPU; automatic CPU-only mode without a device (Compatibility renderer, headless); GPU/CPU badge on each node; auto-update or manual update (F5). Preview quality already offered 256–2,048².
- *Tolerance tests:* `app/tests/gpu_test.gd` compares 36 node cases with the CPU. All are within 100 ppm (0.01%) of the value range on Intel UHD (Mesa) and Mesa llvmpipe, on Godot 4.7.2 and 4.6.2. GitHub Actions runs it on llvmpipe (`godot-gpu` job).
- *Builds stay on the CPU by default*, so exports remain bit-identical on every machine; the Build tab can switch them to the GPU, and `build.json` records which was used.
- *Exit criterion, 10+ fps slider at 1,024²:* met on the development laptop: 36 fps on its integrated GPU, 48 fps with Force CPU (fBm → Levels, request to finished preview).
- *Exit criterion, 2,048² erosion under 5 s:* met on the development laptop. Thermal Erosion takes 0.76 s on the integrated GPU; the existing CPU Hydraulic Erosion solver now takes 4.4 s on 8 workers (was 10.2 s), with bit-identical outputs. Hydraulic remains CPU-only; no parallel GPU routing was introduced.
- **Not met: three GPU vendors.** Only Intel and a software driver were available; no NVIDIA or AMD GPU, and no RTX 3060-class GPU for the speed criteria.

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

**Status (25 Sep 2026):** built on the `v0.5-water` branch; not merged or released. Details, methods and limits are in [water.md](water.md).

- *Done and tested:* Flow (D8 or D-infinity accumulation, direction, basins), Rivers (carved, meandering channels with Water surface, River and Riverbank), Lakes (spill-level filling with sediment infill, Water surface, Lakes, Shore), Sea (edge-connected flooding, beaches, Shallows, Shoreline), Snow (altitude, shaded side, slope, melt, snow depth) and Wetness (wetness index, Water distance). The 3D view draws the water of Rivers, Lakes and Sea over any terrain made with them. Drainage moved to a module shared with Hydraulic Erosion, whose output is unchanged (same golden hash).
- *Exit criteria:* the *River coast* example (mountain → erosion → crater → rivers → lakes → sea → snow) has rivers running to the sea at the world's edges, a crater lake, and exports the River, Lakes, Sea, Shoreline and Snow masks. Snow lies on the high ground and reaches lower on slopes facing the shaded side (up in the 2D view). Checked with rendered masks and the app's 3D view on Windows, Godot 4.7.2; Linux and macOS are left to CI.
- *Limits:* rivers follow D8 routing, so on smooth uneroded slopes they keep some 45° runs; all water nodes are CPU-only.

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

**Status:** implemented on the `v0.6-colour` branch; not merged or released. Colour tab, portals,
ColorMap preview and exports, gradients, blending/layers, image import, normal/cavity and splat
nodes are covered by focused Rust and Godot tests. The *River coast* example has four-layer
weights, a normal map and a colour map marked for export. The under-10-minute Unreal/Godot import
criterion still needs a timed hands-on check; see [colour.md](colour.md).

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

**Status (26 Sep 2026):** built on the `v0.7-vegetation` branch; not merged or released. Details, methods and
limits are in [vegetation.md](vegetation.md).

- *Done and tested:*
  - Trees, Shrubs and Grass populations: growth/health, inhibitors and dead zones, with Water, Snow,
    Allowed-area and Occupied inputs. They output Density, Points, Dead zones, Occupied and Water influence.
  - Occupied chaining in three modes: avoid, intermingle and grow near.
  - Poisson-disk points on a world-fixed cell grid, with random rotation and scale.
  - Debris / Rocks, driven by talus (Thermal Erosion's Debris output, or steep ground) and dead zones.
  - Pack Masks (RGBA).
  - A `PointSet` port type; CSV and JSON point export, listed in `build.json`.
  - 17 YAML species presets (temperate, alpine, desert, tropical) with apply, load and save in the inspector.
  - MultiMesh placeholder plants in the 3D view, capped at 150,000, and the ecosystem data view.
  - The *Forest valley* example; *Asterfall Crown* gained the same chain.
- *Exit criteria:*
  - *Forest valley* chains pine, birch and shrubs. In the tests, pine points average over 300 m above birch
    points, and under 12% of birch cover overlaps pine.
  - 2.5 million points generate in 0.41 s.
  - `app/tests/vegetation_test.gd` exports the points as CSV, places them with the Godot import convention in
    [vegetation.md](vegetation.md), and every point lands within 1 mm of the app's preview.
  - The Unreal placement is documented from the landscape conventions in the export guides but hasn't been
    checked in Unreal. Mesh-scattering importers for Blender, Godot and Unreal are part of the import helpers.
- *Limits:* vegetation runs on the CPU only. Points are sampled per build grid, so tiled builds (v0.8) will need
  a margin. Point counts depend a little on the resolution, because slopes do.

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
- User documentation site, published with GitHub Pages: getting started, every node, export guides per engine, example projects.
- Performance pass guided by benchmarks; startup under 3 seconds.
- Opt-in crash reporting (local log + "copy report" button; no telemetry by default).

**Exit criteria:** a new user follows the getting-started guide and has an eroded, textured, vegetated terrain inside Unreal in under 30 minutes; outside testers complete the tutorial without help.

### v1.0 — Stable release

**Goal:** a dependable 1.0 people can build production work on.

**Deliverables**

- Project format frozen at a stable version, with migrations from every 0.x format tested.
- Node parameter names and behaviour frozen; future changes go through `type_version` migrations.
- Public beta (release candidates) with a bug-fix-only period.
- Signed installers for Windows and macOS, AppImage/Flatpak for Linux, published through GitHub Releases.
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
| Windows/macOS CI runner limits on GitHub | Release builds may be delayed | Monitor Actions quotas; use self-hosted runners if needed |
| Scope creep | Never reaching 1.0 | Every milestone has exit criteria; new ideas go to the 1.x roadmap |
| Solo-developer bandwidth | Slow progress | Clear node API and "add a node" guide to attract contributors; AI agents for well-specified, tested tasks |

**Decided (24 Sep 2026)**

- Name: **OpenTerrainStudio** (written "Open Terrain Studio" in prose), project extension `.otstudio`.
- Owner: EllisonDigital, which holds the copyright and the "OpenTerrainStudio" trademark.
- Hosting: GitHub at `EllisonDigital/open-terrain-studio`; issues, pull requests and releases live there.
- Crates: internal workspace crates are `terrain-core`, `terrain-nodes` and `terrain-godot` (not published). Any crate published to crates.io uses the `openterrainstudio-` prefix (`ots-core` is already taken).
- Licence: dual MIT / Apache-2.0.
- Contributions: Developer Certificate of Origin (DCO) sign-off, no CLA.
- Old hardware: supported via the Compatibility renderer with CPU-only compute.
- Colour: a separate Colour graph tab, used only for applying colour.
- Minimum GPU and RAM: set later from benchmark results.

**Decided (25 Sep 2026)**

- Mask ports scale a parameter: the value used at each cell is the value set × the mask (black = 0, white = the value set), clamped to the parameter's range.
- Directions are degrees with 0° along +X and 90° along +Y, which is clockwise in the 2D view and in exported images.
- Undo covers the graph, parameters, world settings and export marks. Build settings and editor state (camera, viewed node) are saved with the project but not undoable.
