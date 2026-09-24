# GPU compute kernels

GLSL compute shaders for GPU-accelerated nodes live here, one per node, from
milestone v0.4 (see `docs/ROADMAP.md`). Every kernel must match its node's CPU
implementation in `crates/terrain-nodes` within the tolerance its tests define.

Viewport shaders (for display only) live in `app/shaders/`.
