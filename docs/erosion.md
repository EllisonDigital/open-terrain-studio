# CPU erosion

The v0.3 node package adds **Hydraulic erosion**, **Thermal erosion** and **Rock hardness**. Open `presets/erosion-strata.otstudio` for an editable fBm → hydraulic → thermal example with alternating hard and soft beds. The project marks Height, Flow, Wear, Deposition, Sediment and Debris for export through v0.2's Build tab.

## Nodes and units

| Type ID | Inputs | Outputs |
| --- | --- | --- |
| `simulate.hydraulic` | `in`: terrain; optional `mask`: strength; optional `hardness` | `height`, `flow`, `wear`, `deposition`, `sediment` |
| `simulate.thermal` | Same inputs | `height`, `debris` |
| `data.rock_hardness` | `in`: terrain | `out`: hardness mask |

All nodes are type version 1. `height` is deliberately the first schema output, so the existing editor previews the terrain by default. All other outputs are `PortType::Mask`, bounded to 0–1. The architectural illustration's placeholder `simulate.erosion` is not an alias for hydraulic erosion; its old unitless parameters have no defined migration.

Hydraulic controls:

| Key | Default | Meaning |
| --- | --- | --- |
| `duration_s` | 60 s | Simulated duration; zero preserves input |
| `rainfall_m_s` | 0.05 m/s | Uniform rain depth per second, intentionally accelerated for authoring |
| `rock_softness` | 0.5 | Erosion relaxation rate in inverse seconds, before hardness and strength |
| `sediment_capacity` | 2 s/m | Coefficient multiplying speed, slope and water depth |
| `deposition_rate` | 0.5 /s | Relaxation rate for sediment exceeding capacity |
| `evaporation_rate` | 0.02 /s | Exponential water loss rate |
| `downcutting` | 1 | Multiplier on bed erosion; zero prevents new wear |

Thermal controls: `duration_s` (60 s), `talus_angle_deg` (35°), and `diffusivity_m2_s` (10 m²/s). Steeper slopes shed material into neighbouring cells. Transport is proportional to slope excess and diffusivity; increasing resolution reduces the stable timestep rather than changing the physical duration.

Rock hardness alternates horizontal beds using terrain elevation. `layer_thickness_m` (50 m) is each bed's thickness; a complete soft/hard repeat is twice that. `offset_m` shifts the beds vertically. `soft_hardness` (0.1) and `hard_hardness` (0.9) set their strengths with smooth transitions. Hardness is sampled from the supplied map throughout a simulation: it does **not** discover new geological layers as the ground erodes. Chain another hardness/erosion pair for that workflow.

## Strength and hardness

A disconnected strength map means full strength; a disconnected hardness map means soft rock. Finite mask values outside 0–1 are clamped after input validation.

- Hydraulic strength scales both local wear and deposition. A zero-strength cell's ground remains exactly unchanged, but water and suspended sediment still cross it. Flow can therefore remain nonzero in protected areas.
- Thermal strength controls transfer across each face using the lower of the two neighbouring strengths. Zero-strength terrain neither sheds nor receives material.
- Hardness 1 prevents bedrock shedding/wear; it still permits deposition onto hard ground. Hardness 0 adds no resistance.

## Output interpretation and export

Wear is cumulative material removed; deposition is cumulative material settled. Sediment is material **still suspended** at the end, not silently discarded or baked into Height. Their physical values are depths in metres before conversion to masks. Debris is the **positive net increase in terrain height** from thermal transport, not the number of times particles crossed a cell.

These four masks use `m = depth / (1 m + depth)`. Thus 0 means none, 0.5 means one metre and 0.9 means nine metres. Flow integrates outgoing water discharge over the simulation in m² and uses `m = discharge / (10 m² + discharge)`. Flow indicates moving water over the simulation; it is not the watershed/flow-accumulation node planned for v0.5. These fixed scales preserve contrast between resolutions and world regions, without image-dependent auto-normalisation. Adjust masks downstream when a material needs different contrast.

The existing exporter writes masks directly as linear 0–1 float EXR or 16-bit greyscale PNG (0–65535). It does not apply the terrain's height range to masks. Automated round-trip tests check both formats. Actual texture setup in Unreal remains an integration/artist check; no Unreal instance was used to validate this branch.

## Solver and determinism

The clean-room starting point is the water/sediment and talus models in [Musgrave, *Methods for Realistic Landscape Imaging*, §2.4](https://www.kenmusgrave.com/dissertation.pdf). This implementation adds physical spacing/time, bounded transport, explicit simultaneous updates, mask controls and deterministic parallel execution. It is a terrain-authoring approximation, not a calibrated hydrological solver or a full shallow-water momentum simulation.

Hydraulic steps:

1. Add uniform rain to water depth. For each of four neighbours, calculate the downhill slope `s` of the water surface.
2. Use face speed `v = 10 m/s × s / (1 + s)`. Transfer depth `q = water × v × dt / cell_spacing`, limited to one eighth of the positive surface-height difference to avoid overshooting pools.
3. Gather incoming/outgoing water and sediment from immutable state. Sediment moves with the donor's water concentration. Evaporate water exponentially.
4. Set sediment capacity from outgoing speed, bed slope and water depth. Erode deficits, or deposit excess, with exponential relaxation. Bed wear per step cannot exceed one quarter of local downhill relief.

`dt ≤ min(dx,dy)/(4×10 m/s)` and `dt ≤ 0.5 s` bound water transport. Thermal flux is `max(height_difference - tan(angle) × spacing, 0) × diffusivity × dt / spacing²`, scaled by strength and hardness. Thermal `dt ≤ 0.2×min(dx,dy)²/diffusivity` and `dt ≤ 1 s`. Step counts round upward, then `dt = duration / steps`, so the full requested duration is represented. Cells at the boundary have no external neighbours: no wraparound, clamping inflow or loss through the edges.

Rayon computes disjoint destination cells. Neighbours are gathered in a fixed west/east/north/south order; there are no floating-point atomic accumulations or thread-dependent reductions in outputs. Non-polynomial node mathematics uses `libm`. Tests compare every output bit with one and eight workers; cross-platform bit equality still needs release CI evidence.

The simulation reports node-local progress and checks cancellation between each parallel pass and during output assembly. A pass must finish before cancellation returns. Invalid/mismatched/nonfinite input grids are rejected. Cell spacings below 1 cm and jobs needing over 100,000 steps return actionable errors rather than running unboundedly.

## Validation and practical limits

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p terrain-nodes --example bench_erosion -- 1024 8 60
# Optional fourth argument writes diagnostic maps and a project:
cargo run --release -p terrain-nodes --example bench_erosion -- 1024 8 60 build/erosion-review
godot --headless --path app --script res://tests/erosion_test.gd
```

Measured on an Intel i7-14700HX with a fixed eight-worker Rayon pool, Linux, release build: 1,024² over 4,096×4,096 m with 60 s simulation duration, fBm input, hydraulic **4.666 s**, thermal **0.595 s**. Timings include upstream noise evaluation, but exclude diagnostic file writing. Eight workers are not a hardware-isolated eight-core benchmark. Default hydraulic mean absolute height change was 10.71 m, so the benchmark does real erosion.

The normal Rust test suite evaluates 512² and 2,048² over the same 4,096 m terrain for 12 s, resampling the high-resolution **erosion delta**, not just comparing the much larger input heights. There are two fixtures: a smooth ridge and a steeper ridge meeting a foothill, which must produce measurable deposition. Observed relative delta RMS errors:

| Fixture | Hydraulic | Thermal |
| --- | --- | --- |
| Smooth ridge | 1.37% | 0.74% |
| Foothill | 1.82% | 1.57% |

Directional similarities are above 0.999. Test limits are 10% relative delta RMS, cosine similarity >0.99, mask absolute RMS <0.02 (about five 8-bit levels) and relative mask RMS <15% where the signal is non-negligible. An eight-low-resolution-cell boundary margin is excluded. Minimum-effect assertions prevent a no-op solver or empty foothill deposition map from passing. The largest absolute mask RMS was Wear at 0.01297; the largest non-negligible relative mask RMS was foothill Deposition at 12.38%. The two fixtures take roughly 38 s in the development test profile with four workers, and run in the ordinary CI suite. These are bounded comparisons for specific fixtures, not a guarantee for every input or duration.

Closed boundaries pool water at edges. Four-neighbour stencils can introduce directional bias, and features near the grid spacing cannot match across resolutions. Long runs, very rough inputs, large world height offsets and aggressive parameters need visual review. The CPU implementation keeps several full-resolution buffers; it does not implement tiling or disk spilling. Hydraulic simulation storage is roughly 72 bytes/cell before inputs/output construction (about 72 MiB at 1,024²); output conversion temporarily adds more. These limits belong in v0.3 review and v0.4 GPU planning, not a silent promise of production-scale simulation.
