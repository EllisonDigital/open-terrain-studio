<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# OpenTerrainStudio import helpers

Read an exported **build.json**, with its image files alongside it:

- [Blender add-on](blender/README.md): centred, metre-scaled terrain meshes, packed mask images and point attributes.
- [Godot editor plugin](godot/README.md): saved terrain scenes, float textures and selectable mask preview.
- [Unreal Python script](unreal/README.md): Landscape actors and linear layer-weight texture assets. **Untested in Unreal**, including its required native editor bridge.

The helpers also accept vegetation `PointSet` CSV/JSON alternatives described in [the exact file contract](points-format.md). Blender uses one point mesh and Geometry Nodes instancer per species, Godot one `MultiMeshInstance3D` per species, and Unreal one `HierarchicalInstancedStaticMeshComponent` per species with batched inserts. Replace the placeholder cone with your own nominal-size species asset. No per-point scene objects are created.

All helpers support multiple heightfields, multiple masks, alternative PNG/EXR encodings, and v0.1 single-output builds. Alternative encodings of the same `(node, port)` produce one terrain/texture. Different heightfields produce separate terrains at the **same** world position: hide the alternatives you are not using. Mask-only builds are also supported.

The generator must be exactly `OpenTerrainStudio`. Missing files (including unselected alternative encodings), unsupported encodings/orientations, duplicate output formats, or contradictory grid metadata are errors. Files are resolved relative to the manifest; do not move just the JSON. Unknown extra metadata is ignored. No source files are rewritten.

Blender and Godot prefer EXR. Their PNG16 decoder reads all sixteen bits directly, without gamma conversion or an eight-bit intermediate. Heights map through `height_range_m`, masks remain 0–1. The Blender package contains the shared pure-Python validator/PNG decoder also used by the Unreal script; keep the helper directory together when using Unreal.

Blender and Godot currently limit a build to 4,194,304 samples per image. Geometry is fully baked at one vertex per sample, not streamed, tiled, decimated or converted to a terrain-system-specific asset. Godot does not create collision. There are no installers, background update watchers or automatic reimport.

## Tests

Run from the repository root. Substitute the paths to your Blender and Godot executables. All generated fixtures and build products stay under `import-helpers/`; no app or engine source edits are required.

```sh
python3 -m unittest discover -s import-helpers/tests -p 'test_*.py' -v
CARGO_TARGET_DIR="$PWD/import-helpers/.work/target" cargo run \
  --manifest-path import-helpers/tests/fixture_export/Cargo.toml -- \
  "$PWD/import-helpers/.work/fixture" 129
python3 import-helpers/tests/make_variants.py import-helpers/.work/fixture
python3 import-helpers/tests/make_points.py import-helpers/.work/fixture
python3 import-helpers/tests/make_points.py import-helpers/.work/fixture --million
blender --background --factory-startup --python-exit-code 1 \
  --python import-helpers/tests/blender_check.py -- "$PWD/import-helpers/.work/fixture"
python3 import-helpers/blender/package.py
blender --background --factory-startup --python-exit-code 1 \
  --python import-helpers/tests/blender_package_check.py -- "$PWD/import-helpers/.work/fixture"
godot --headless --editor --path import-helpers/godot --import
godot --headless --path import-helpers/godot \
  --script res://../tests/godot_check.gd -- "$PWD/import-helpers/.work/fixture"
CARGO_TARGET_DIR="$PWD/import-helpers/.work/target" cargo test --locked \
  -p terrain-core unreal_hints_reproduce_png_heights
```

The fixture executable calls the real `terrain_core::export::write_value`/`BuildInfo` writer. It exports two distinct heightfields and two masks in both formats, with a 2,048×1,024 m world and −120…2,280 m range, plus independent raw float reference files. Tests compare **every vertex and mask sample**, not a visual screenshot or a handful of points. Godot checks the saved/reloaded scene as well as the float texture type. An independent PNG encoder exercises all five row filters, including values whose low byte matters.

`make_points.py` adds deterministic 5,000-point and million-point variants beside the export, leaving its original `build.json` intact. The engine checks compare every point's position/yaw/scale/species against CSV and its height against the imported terrain. The million-point check asserts import below 30 seconds (including terrain import, excluding fixture generation); run on a machine with the engines installed. The Unreal dry run requires a 1009² fixture: generate it with the command above substituting `fixture-large` and `1009`, then run `make_points.py` against that folder.

For the larger test, repeat fixture generation using `1009` and a new `fixture-large` folder, then run each headless test with `fixture-large single` as its final arguments. This checks all 1,018,081 vertices of one EXR heightfield without keeping several large meshes in memory.

## Results — 25 September 2026, Linux

| Check | Result |
| --- | --- |
| Python 3.13.5 | 10 unit tests passed; includes every uint16 height value, Rust float32 reference cases, legal Landscape layouts, PNG filters/checksums, multiple outputs and invalid manifests |
| Rust v0.2 `unreal_hints_reproduce_png_heights` | Passed unchanged |
| Blender 5.2.0 LTS | 129² multi-output, PNG-only, single-output, mask-only, malformed/missing files and existing scene-unit scale passed |
| Blender packaged ZIP | Registration and import operator passed; all mesh vertices, mask attributes and packed image pixels survived `.blend` save/reload for EXR and PNG-only builds |
| Godot 4.7.2 and 4.6.2 | Same 129² variants, all PNG filters, mask preview parameters, and save/reload precision passed |
| Blender 5.2.0 / Godot 4.7.2, 1009² single EXR | All 1,018,081 vertex heights exactly matched the Rust float samples; maximum horizontal coordinate error 0.0301 mm |
| 129² EXR heights and masks | Zero observed sample error in both tools |
| 129² PNG heights and masks | Max height error versus pre-quantisation source 0.018433 m across a 2,400 m range; mask error ≤0.000007630, consistent with 16-bit quantisation and float rounding |
| Unreal dry-run on the real 1009² export | Validated two Landscape import plans plus two masks, including different X/Y spacing |
| Unreal Editor/native bridge | **Not compiled or run.** No Unreal installation was available. Python maths and dry-run checks do not validate editor APIs, texture import or native Landscape creation. |

Interactive file-dialog clicks, GPU appearance, packaged game export, Windows/macOS and Blender versions other than 5.2.0 were not tested. These are explicit limits, not inferred successes from the headless runs.

## PointSet results — 26 September 2026, Windows

Python 3.14.7: **11 unit tests passed**. The deterministic million-point CSV parsed and validated in **2.635 s** (not an engine import). A real 1009² Rust-export fixture with 5,000 points passed Unreal's Python dry-run. Blender, Godot and Unreal executables were not available on this machine; **their new PointSet import, transform, save/reload and 30-second million-point targets have not been tested here**. Unreal's native bridge has not been compiled or run. Do not treat parser timing as Blender/Godot timing.

## Implementation references and licence

Clean-room code, dual MIT/Apache-2.0; see the repository's [MIT](../LICENSE-MIT) and [Apache](../LICENSE-APACHE) licences. No third-party terrain assets or presets are included.

API references: [Blender Image](https://docs.blender.org/api/5.2/bpy.types.Image.html), [Godot ImageTexture](https://docs.godotengine.org/en/4.7/classes/class_imagetexture.html), [Godot EditorPlugin](https://docs.godotengine.org/en/4.4/classes/class_editorplugin.html), [Unreal Landscape import](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Landscape/ALandscapeProxy/Import?application_version=5.6), and [Epic Landscape technical guide](https://dev.epicgames.com/documentation/unreal-engine/landscape-technical-guide-in-unreal-engine). PNG decoding implements the standard noninterlaced grayscale16 format; neither helper relies on colour-managed image conversion for PNG samples.
