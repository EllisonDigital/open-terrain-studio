# Contributing to OpenTerrainStudio

Thanks for helping. OpenTerrainStudio is developed by EllisonDigital and the community on GitLab:
`gitlab.com/ellison-digital/open-terrain-studio/open-terrain-studio`

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

## Before you open a merge request

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

1. Add the node to `crates/terrain-nodes/src/` (see `basic.rs` for small examples).
2. Give it a permanent `type_id` like `noise.ridged`. Never rename it later: project files store it.
3. Declare parameters with `ParamDef`. **All sizes are metres** (`ParamDef::metres`), never pixels.
4. In `evaluate`, compute values from world positions (`Grid::from_fn` gives `x_m`, `y_m`), so the node
   looks the same at every resolution. Use `ctx.seed` for randomness, never time or thread order.
5. Register it in `terrain_nodes::registry()`.
6. Add a test. The pipeline test already checks that every registered node evaluates without NaNs.

If you later change what a parameter means, bump `type_version` and implement `NodeKind::migrate` so old
projects still open correctly.

## Ground rules

- **Deterministic:** same project and seed must give byte-identical output on every machine. Avoid
  platform maths like `f64::powf`/`sin` in node results (use `libm`), and hash-map iteration order.
- **Clean-room:** implement from published papers and public algorithm descriptions. Comparing results with
  Gaea visually is fine; decompiling it or copying its presets or assets is not.
- **Small merge requests** with a clear description are reviewed fastest.

## Reporting bugs

Use the Bug issue template and attach the `.otstudio` file if you can. It's small, human-readable JSON.
