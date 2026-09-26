<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Import helpers handoff

Roadmap milestone: **v0.9 — Import helpers (delivered early)**. This implements
the import-helper item ahead of the rest of v0.9, not the entire milestone.

Branch: `import-helpers`, rebased onto `origin/main` on 26 September 2026.
Worktree: `../open-terrain-studio-import-helpers`.

Changes are confined to `import-helpers/` and the “Import helpers” section in
`docs/export-guides.md`; no app/core/shader/roadmap/architecture edits. No PR opened.
Code is clean-room and MIT OR Apache-2.0.

- Blender: installable add-on ZIP, multi-terrain import, exact EXR and lossless PNG16,
  metre scaling, packed mask images and per-vertex mask attributes.
- Godot: editor plugin, saved terrain scenes with full-float embedded textures,
  custom PNG16 decoder and selectable mask preview.
- Unreal: Python importer plus required native editor bridge, exact Landscape
  scale/layout planning and mask texture assets. **Untested in Unreal**, including
  bridge compilation (UE 5.6 API target). Needs a C++ Editor build and a
  non-World-Partition level. Masks are texture assets, not painted Landscape layers.

Validated on Linux, 25 September 2026: Python 3.13.5 (10 tests), unchanged Rust
`unreal_hints_reproduce_png_heights`, Blender 5.2.0 LTS, Godot 4.7.2 and 4.6.2.
Real Rust exports cover multiple outputs/masks, PNG-only, old single-output,
mask-only and invalid/missing input. Every 129² EXR vertex/mask matched exactly;
PNG error stays within quantisation tolerance. Blender and Godot 4.7.2 also matched
all 1,018,081 heights of a 1009² EXR export. Godot scene reload and Blender packaged
operator/save/reload passed, including packed pixels. Unreal's real-export dry-run
passed; this does not validate its editor APIs.

See [test commands and detailed results](README.md#tests) and each tool's README
for installation and limits. Interactive dialogs, GPU rendering, packaged games,
Windows/macOS and other Blender versions were not tested. Full-resolution meshes
are limited to 4,194,304 vertices per image; no tiling/LOD or Godot collision.
Generated exports, ZIP, scenes and test build products are ignored in `.work/` and
can be regenerated; no fixtures or binaries need to be merged.

PointSet update (Windows, 26 September 2026): both CSV and JSON point formats are
validated; Blender uses Geometry Nodes on species point meshes, Godot MultiMeshes,
and Unreal a Python/native HISM bridge with 8,192-instance batches. Deterministic
5,000/1,000,000-point fixtures include corners, rotations and scale extremes.
Python 3.14.7 passed 11 tests; a million CSV points parsed in 2.635 s. A 1009²
real-export 5,000-point Unreal dry run passed. **Blender and Godot were not installed
here: neither engine importer, transform checks, save/reload nor the 30-second
million-point target was run. Unreal Editor/bridge compilation remains untested.**
