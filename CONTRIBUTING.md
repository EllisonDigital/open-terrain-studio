# Contributing to OpenTerrainStudio

Thanks for helping. OpenTerrainStudio is developed by EllisonDigital and the community on GitHub:
`github.com/EllisonDigital/open-terrain-studio`

Before a larger change, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and check the milestone in
[docs/ROADMAP.md](docs/ROADMAP.md). Opening an issue first avoids duplicated work.

## Sign your commits (DCO)

Every commit must carry a `Signed-off-by:` line, certifying the
[Developer Certificate of Origin](https://developercertificate.org/): you wrote the change, or have the
right to submit it under the project licence. You keep your copyright.

```sh
git commit -s -m "Add ridged noise node"
```

Forgot? `git commit --amend -s` (last commit) or `git rebase --signoff main` (whole branch).

Contributions are dual-licensed MIT / Apache-2.0, like the rest of the project.

## Set up

You need:

- **Rust** (stable, 1.94 or newer): <https://rustup.rs>
- **Godot 4.6 or newer** (standard build, not .NET): <https://godotengine.org/download>

```sh
cargo build                      # builds the engine and the Godot extension
godot --path app                 # runs the app (or open app/project.godot in the Godot editor)
```

The extension is loaded from `target/debug`, so after changing Rust code just run `cargo build` again and
restart the app.

## Before you open a pull request

CI runs all of these; running them locally saves a round trip.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build && godot --headless --path app --script res://tests/smoke_test.gd
```

## Adding a node

Nodes are plain Rust: one struct, one schema, one `evaluate` function. The inspector and graph editor are
generated from the schema, so **no Godot code is needed**.

1. Add the node to `crates/terrain-nodes/src/` in the module for its category (`primitives.rs`, `noise/`,
   `terrain.rs`, `adjust.rs`, `data.rs`); `basic.rs` has small examples, `common.rs` shared parameters.
2. Give it a permanent `type_id` like `noise.ridged`. Never rename it later: project files store it.
3. Declare parameters with `ParamDef`. **All sizes are metres** (`ParamDef::metres`), never pixels; directions
   are degrees with 0° along +X and 90° along +Y.
4. In `evaluate`, compute values from world positions (`Grid::from_fn` gives `x_m`, `y_m`), so the node
   looks the same at every resolution. Use `ctx.seed` for randomness, never time or thread order. Shared
   algorithms (blur, gradients, distance transforms) are in `terrain_core::ops`.
5. If a float parameter could sensibly vary across the terrain, mark it `.drivable()` and read it with
   `ctx.field("key").at(index)` instead of `ctx.f32("key")`. Users can then drive it with a mask.
6. Register it in `terrain_nodes::registry()`.
7. Run `cargo test`. Every registered node is checked automatically for finite output, determinism across
   thread counts and resolution independence; add a tolerance in `tests/nodes.rs` only if the node uses
   neighbouring samples. Then record its golden hash:
   `OTS_BLESS=1 cargo test -p terrain-nodes --test nodes golden`.

To see a node without starting the app, render it to a shaded PNG:

```sh
cargo run --release -p terrain-nodes --example render -- terrain.mountain mountain.png 1025
```

To give a node a GPU version, see *Adding a kernel* in [docs/gpu.md](docs/gpu.md); the GPU test then
checks it against the CPU automatically:

```sh
cargo build && godot --path app --rendering-driver vulkan --script res://tests/gpu_test.gd
```

If you later change what a parameter means, bump `type_version` and implement `NodeKind::migrate` so old
projects still open correctly. If you change a node's output on purpose, re-bless the golden hashes and say
why in the pull request: it changes existing users' terrains.

## Ground rules

- **Deterministic:** same project and seed must give byte-identical output on every machine. Avoid
  platform maths like `f64::powf`/`powi`/`sin`/`exp` in node results (use `libm`), and hash-map iteration
  order.
- **Clean-room:** implement from published papers and public algorithm descriptions. Comparing results with
  Gaea visually is fine; decompiling it or copying its presets or assets is not.
- **Small pull requests** with a clear description are reviewed fastest.

## Reporting bugs

Use the Bug issue template and attach the `.otstudio` file if you can. It's small, human-readable JSON.
