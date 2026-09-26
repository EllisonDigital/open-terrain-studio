# Production scale (v0.8)

Builds bigger than one grid can hold are computed in tiles, put back together on disk, and written
as whole images or as tile files. Results pushed out of the in-memory cache can be kept on disk.
The code is in `crates/terrain-core/src/tiled.rs` (planning and evaluation), `assemble.rs` (disk
assembly and streamed writers), `export.rs` (builds) and `cache.rs` (the cache).

## Tiled builds

Builds up to **4,097** samples per side are computed whole, exactly as before v0.8. Larger builds
(up to 65,537) are computed in tiles of 1,024, 2,048 (default) or 4,096 samples per side, set on the
Build tab. Previews are never tiled.

Each tile has a **core**: the block of build samples it is responsible for. The cores cover every
sample exactly once. A tile is a *window* of the build grid (`GridSpec::window`): its sample
positions and cell size are computed from the whole grid, so they are bit-identical to the same
samples of an untiled grid.

### How far each node reads

Every node says how far from a sample it reads its inputs (`NodeKind::reach`):

- **Local, 0 m** (point-wise): noise, primitives, terrain shapes, Combine, Curve, Clamp, Terrace,
  Height Mask, Select Range, the colour nodes, Splat, Pack Masks, Rock Hardness, portals, and Levels
  or Invert with a fixed range.
- **Local, some metres**: Blur and Sharpen (1.5 × radius), Slope, Aspect and Normal Map (one cell),
  Curvature, Occlusion, Distance (its distance), Warp (√2 × strength), Transform (how far its move,
  turn and scale shift any point of the world), Snow (its smoothing), and the populations and
  Debris (their blurs plus about 14 × spacing for the point sampler).
- **Global**: Hydraulic and Thermal Erosion, Flow, Wetness, Rivers, Lakes and Sea, and Levels or
  Invert using the input's own range. A local node whose reach is over a quarter of the world is
  treated as global too.

Nodes that don't say are treated as global, which is always correct, only slower.

Each node is computed over its tile's core grown by a **margin**: enough that everything downstream
is exact over the core. Working from the requested outputs upstream, a node's margin is the largest
over its consumers of (the consumer's margin + the consumer's reach). A tiled build of local nodes
is therefore **bit-identical** to an untiled one; `crates/terrain-nodes/tests/tiled.rs` checks this
for every local node type, points included.

The point sampler used by the populations and Debris now assigns its nine parallel phases by world
cell rather than by cell within the grid (the same thing for a whole-world grid, so earlier results
are unchanged); without this, a tile's points differed from the whole world's.

### Global nodes: world passes

A global node gets a **world pass**. Its inputs are computed tile by tile at build resolution and
streamed onto one whole-world grid of the node's choosing (`NodeKind::world_spec`):

- Hydraulic Erosion, Flow, Wetness, Rivers and Lakes: their simulation grid, set by *Detail size*
  (8 m cells by default: 1,025² for an 8 km world). The streamed input is filtered exactly as an
  untiled build filters it before resampling (a Gaussian of half a simulation cell), so the node sees
  the same terrain.
- Thermal Erosion and Sea: the build grid, capped at 4,097².
- Auto-range Levels and Invert: no grid, only the exact lowest and highest input value over the whole
  build, so their tiles match an untiled build exactly.

The node then runs once on that grid, and each tile's result is brought back to build resolution
(`NodeKind::finish_tile`): masks bilinearly, categories and angles (Flow's Direction and Basins) by
nearest sample, and heights as **the tile's own input plus the change the node made**, so detail finer
than the world pass survives. This is what Hydraulic Erosion already did at resolutions finer than its
simulation. Rivers and Lakes go further: the world pass finds the river courses and lakes once (shared
by every tile), and each tile carves channels, banks, lake beds and shores at full resolution, as an
untiled build does.

Global nodes run in **waves**: those with no global node upstream first, then those that depend only
on the first wave, and so on. Each wave is one pass over all tiles; a final pass computes the requested
outputs. With the cache, tiles computed in one pass are reused in the next.

Measured against untiled evaluation at 513² (16 m cells, drainage and erosion at 32 m, tiles of 128;
`tests/tiled.rs`), the mean difference is 0.0000% of the value range for every output of Hydraulic
Erosion, Flow, Rivers, Lakes, Sea and Thermal Erosion, and 0.17% for Wetness. Tiled Hydraulic
Erosion heights are within 5 cm everywhere.

### Putting builds back together

Each requested output is written into a temporary file in the output folder as tiles finish
(`.ots-build-*.raw`, 4 bytes per value: about 1 GB per image at 16,384², 4 GB for a colour map), and
deleted when the build ends. Point sets are kept in memory. Images are then written from the
temporary files a band of rows at a time, so no whole-world image is held in memory. Images up to 64
million values are written with the same writers as untiled builds.

`build.json` gains `computed_in_tiles` (the tile size) for tiled builds.

## Background builds

Builds and exports run on their own threads (all cores but two), so previews stay responsive, and
show their own progress bar on the Build tab and in the status bar. The graph stays editable: a build
uses the project as it was when it started. *Cancel* stops it and writes nothing.

## Tile files (Unreal World Partition)

*Write images as tiles* on the Build tab writes each image as a grid of files instead of one. Tiles
of `size` samples share their edge row and column, like landscape components, so a build of
`n × (size − 1) + 1` samples splits into `n × n` equal tiles: 16,129 = 8 × (2,017 − 1) + 1. Other
resolutions work too, with smaller tiles along the far edges; the Build tab says which. Point sets are
not split.

Files are named by the pattern, `{name}_x{x}_y{y}` by default, where `{name}` is the usual file name
and `{x}`, `{y}` count columns and rows from 0. This is the naming Unreal's tiled landscape import
expects. In `build.json`, `file_tiles` gives the size, columns, rows, pattern and whether every tile is
full size (`even`), and each file lists its `tile`, `tile_origin` (first sample in the whole build) and
`tile_size`. The Unreal import values in `build.json` → `unreal` apply to every tile.

## The cache

Computed results are kept so unchanged nodes aren't computed again (ARCHITECTURE.md §5). *Settings →
Cache…* sets how much memory results may use (1 GB by default) and whether results pushed out of memory
are kept on disk instead of dropped (on by default, up to 8 GB, in the system's temporary folder). A
result read back from disk counts as reused, not computed. Spill files are deleted when the app closes;
files over an hour old are cleared when a cache folder is taken into use, in case a session ended
without cleaning up. GPU results, and the tile results of large builds (many, and rarely asked for
again), are dropped rather than spilled.

## Limits

- Wetness and Thermal Erosion come close to, but don't match, untiled builds when the build is finer
  than their world pass. Thermal Erosion's world pass is capped at 4,097², so above that it simulates
  at 4,097² and its tiles keep their input's finer detail.
- Sea above 4,097² works the same way: coastlines and beaches are found at 4,097².
- A Transform that turns or scales the whole terrain reads far across the world, and is then treated
  as global: its tiles are bilinear from a 4,097² world pass.
- In tiled builds a global node's inputs must be heights or masks (true of every built-in global node).
- The build needs free disk space in the output folder for its temporary files.
