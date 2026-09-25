# GPU compute kernels

GLSL compute shaders for GPU-accelerated nodes (milestone v0.4, see `docs/ROADMAP.md` and
[docs/gpu.md](../docs/gpu.md)). Every kernel must match its node's CPU implementation in
`crates/terrain-nodes` within the tolerance in `crates/terrain-nodes/src/gpu_check.rs`.

| File | Used by |
| --- | --- |
| `common/common.glsl` | every kernel: the parameter block, grid indexing, `soft_range`, a precise `atan` |
| `common/hash.glsl` | the seed hashes of `terrain_core::seed`, bit for bit, with 64-bit integers as `uvec2` |
| `common/noise.glsl` | an f32 port of `noise/basis.rs` (Perlin, simplex, value, cellular, fractals) |
| `core/affine.comp` | Heightfield ↔ Mask conversion of GPU results |
| `core/blur_taps.comp`, `core/blur_box.comp` | `Gpu::gaussian_blur`, the same kernels as `ops::gaussian_blur` |
| `noise.comp` | every Noise node |
| `adjust.comp` | Blur, Sharpen, Transform, Warp |
| `data.comp` | Slope, Aspect, Curvature |
| `thermal.comp` | Thermal Erosion |

One file per node family rather than per node: nodes in a family share most of their code.

## Conventions

- The Rust side (`terrain_core::gpu::Kernel`) prepends `#version 450` and joins the parts it lists,
  shared files first. There is no `#include`.
- Grids are `std430` storage buffers of `float`, row-major like `Grid::data`, bound at `set = 0` in
  the order the Rust code passes them. Unused bindings get a one-float dummy buffer.
- Parameters are the 128-byte push-constant block in `common.glsl`: `pc.u[16]` then `pc.f[16]`.
  `u[0]`, `u[1]` are width and height. Document each shader's other slots at its top.
- 2D kernels use `local_size_x = 8, local_size_y = 8`; per-row kernels use `local_size_x = 64`.
- Follow the CPU code's order of operations. Where the CPU uses `f64`, precompute what you can on the
  CPU in `f64` and pass it in (e.g. noise lattice origins); positions in metres stay small enough for
  `f32`.
- Don't rely on built-in transcendental precision: Vulkan allows `atan` to be off by 4096 ulp. Use
  `atan_precise` where a threshold follows.
- Outputs of `Gpu::alloc` are uninitialised: write every element.

Viewport shaders (for display only) live in `app/shaders/`.
