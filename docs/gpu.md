# GPU compute

Nodes with a GPU kernel run on the GPU in previews; everything else runs on the CPU as before. This page
describes how that works, how it's tested and what was measured. Design background is in
[ARCHITECTURE.md §6](ARCHITECTURE.md#6-gpu-compute-strategy); kernel conventions are in
[shaders/README.md](../shaders/README.md).

## What runs on the GPU

| Node | Kernel | Largest difference from the CPU (identical inputs) |
| --- | --- | --- |
| Perlin, Simplex, Value, Voronoi | `noise.comp` | under 1 ppm of the value range |
| fBm, Ridged, Billow, Domain Warp | `noise.comp` | 15–90 ppm (see *Limits*) |
| Blur, Sharpen | `core/blur_*.comp`, `adjust.comp` | under 1 ppm |
| Transform, Warp | `adjust.comp` | under 4 ppm |
| Slope, Aspect, Curvature | `data.comp` | under 20 ppm |
| Thermal Erosion (Height, Debris) | `thermal.comp` | under 0.1 ppm |

ppm = millionths of the output's value range; the required tolerance is 100 ppm (0.01%). Measured at
257² on Intel UHD (RPL-S) and Mesa llvmpipe, Godot 4.7.2 and 4.6.2.

Hydraulic Erosion has **no** GPU kernel yet: see *Limits*.

## How it works

- `terrain-core` has no Godot dependency, so it talks to the GPU through the small `GpuDevice` trait
  (`crates/terrain-core/src/gpu.rs`): upload, allocate, download, dispatch a GLSL kernel, flush.
  `terrain-godot` implements it with a local RenderingDevice owned by one thread (`src/gpu.rs`),
  because a local device only works on the thread that created it.
- A node with a kernel sets `NodeSchema::gpu` and implements `NodeKind::evaluate_gpu`. The evaluator
  uses it when a device is given (`EvalOptions::gpu`) and falls back to `evaluate` on any GPU error.
  Fallbacks are counted and shown in the status bar's tooltip.
- GPU outputs are `Value::Gpu`: they stay on the GPU, so the next GPU node reads them directly. They are
  downloaded once, the first time a CPU node, the preview or an export asks for the data.
- GPU results are cached under their own keys (`|gpu` is added to the key), so Force CPU and CPU builds
  never reuse a GPU result.
- Every kernel is compiled when the device starts, in the background; a kernel that fails to compile
  makes the whole device unavailable, so problems show at once instead of mid-preview.
- Long simulations submit work in batches of 64 steps, checking for cancellation and reporting
  progress between batches, so no single submission runs long enough for the driver to be reset.

## Settings

- **Settings → Force CPU (debugging):** every node on the CPU. Saved in `user://settings.cfg`.
- **Settings → Auto-update Preview:** off = edits only mark the preview out of date; **Update (F5)**
  recomputes it. Saved in `user://settings.cfg`.
- **Build tab → Compute on the GPU:** off by default, so builds and exports are bit-identical CPU
  results on every machine. `build.json` records which was used (`"compute": "cpu"` or
  `"gpu: <device>"`).
- There is no device on the Compatibility renderer or in `--headless` runs; the status bar then shows
  "CPU only" and everything runs on the CPU, with the same results.
- Each node in the graph shows GPU or CPU while a GPU is in use.

## Tests

```sh
cargo build -p terrain-godot
godot --path app --rendering-driver vulkan --script res://tests/gpu_test.gd     # needs a GPU or lavapipe
godot --path app --rendering-driver vulkan --gpu-index 1 --script res://tests/gpu_test.gd   # another device
OTS_GPU_TEST_RES=1025 godot --path app --rendering-driver vulkan --script res://tests/gpu_test.gd
godot --path app --rendering-driver vulkan --script res://tests/gpu_bench.gd    # timings
```

`gpu_test.gd` calls `TerrainBuilder.run_gpu_check`, which runs every GPU node (and variants: each
Voronoi mode, each fractal basis, driven parameters, large offsets, rotation) on the CPU and on the GPU
and compares every output. The input terrain is a Mountain, which has no kernel, so both runs see
identical inputs and the check measures the kernel alone. The test also checks that a preview uses the
GPU, that Force CPU works and is cached separately, and that a whole GPU chain (fBm → Blur) agrees with
the CPU. CI runs it on Mesa's software Vulkan driver under `xvfb-run` (job `godot-gpu`), which checks
the kernels and plumbing but not speed or real drivers.

## Measurements (25 Sep 2026)

Release build, i7-14700HX (28 threads) with its integrated Intel UHD graphics (RPL-S, 32 EUs; about
1/15 of an RTX 3060's compute). Times are from request to finished preview image, median of repeated
edits.

| | GPU (Intel UHD) | CPU (28 threads) |
| --- | --- | --- |
| Drag an fBm slider, fBm → Levels, 1,024² | 27.5 ms (36 fps) | 20.9 ms (48 fps) |
| fBm → Warp → Blur → Slope, 1,024² | 41.3 ms | 41.4 ms |
| fBm → Levels, 2,048² | 69.4 ms | 42.9 ms |
| Thermal Erosion, 2,048², 60 s | 0.76 s | 1.10 s |
| Hydraulic Erosion, 2,048², 1,000 kyr | CPU solver | 4.4 s (8 workers) |

On this machine the integrated GPU is about as fast as the 28-thread CPU. The kernels are memory-bound
and simple, so a discrete GPU should be several times faster, but that hasn't been measured: no
NVIDIA or AMD GPU was available.

Hydraulic Erosion at 2,048², 1,000 kyr takes 4.4 s in the release benchmark (8 workers), down from
10.2 s before the v0.4 CPU optimization. It remains entirely on the CPU. The optimization keeps
Priority-Flood's exact `(filled height, cell index)` ordering while avoiding a heap operation for
most unraised terrain cells: sort cells by original height and merge that sequence with a heap of
raised pit/flat cells. It also hoists invariant per-cell jitter and per-link distance factors, and
computes discharge powers once for both incision and sediment capacity. A routing unit test compares
the filled surface and pop order with the original binary-heap algorithm on plateaus, pits and varied
grid aspect ratios; EXR outputs at 1,024² and 2,048² are byte-identical to the pre-optimization
baseline. Timing is machine- and load-dependent; the ~4.4 s run meets the 5 s target here, but with
limited headroom on other hardware.

## Limits

- **Hydraulic Erosion stays on the CPU.** Its Priority-Flood routing and subsequent passes along the
  drainage tree are sequential. The existing solver was optimized without changing outputs; a GPU
  solver would need different, parallel routing and would carve rivers in slightly different places.
- **Fractal noise precision.** Kernels compute lattice positions in f32. Each octave multiplies them
  by the lacunarity, so beyond about 8 octaves the finest octaves lose precision: a 12-octave Ridged
  differs by up to 160 ppm in 0.02% of samples. These nodes may exceed 100 ppm in up to 0.1% of
  samples.
- **Voronoi Cells** is discontinuous at cell borders; a sample within f32 rounding of a border may take
  the neighbouring cell's value (allowed in 0.1% of samples; none seen so far).
- **Previews read results back.** The viewport still receives an image (a 1,024² read-back takes a few
  milliseconds). Handing it the GPU buffer directly would need the renderer's own RenderingDevice.
- **Nodes without kernels read back their GPU inputs** (for example Levels after fBm). Cheap per-cell
  nodes (Levels, Combine, Clamp, Curve…) are the next candidates for kernels.
- **Only tested on Intel (Mesa ANV) and Mesa llvmpipe**, on Linux. The exit criterion asks for NVIDIA,
  AMD and Intel.

## Adding a kernel

1. Write the kernel in `shaders/` (conventions in [shaders/README.md](../shaders/README.md)) and add a
   `Kernel` for it in `crates/terrain-nodes/src/kernels.rs` (and to `kernels::all`).
2. Set `gpu: true` in the node's schema and implement `evaluate_gpu`: get inputs with `gpu.input`,
   drivable parameters with `gpu.field`, allocate outputs with `gpu.alloc`, then `gpu.dispatch_grid`
   and return `gpu.value(...)`. Return `CoreError::GpuUnsupported` for settings the kernel doesn't cover.
3. Run `gpu_test.gd`: every GPU node is checked automatically. Add variants for its modes to `cases`
   in `gpu_check.rs`, and a tolerance only if it genuinely can't meet 100 ppm.
