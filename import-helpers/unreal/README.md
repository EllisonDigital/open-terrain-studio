<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Unreal Editor import — UNTESTED IN UNREAL

The maths and manifest handling have plain-Python tests. **Neither the editor script nor the C++ bridge was compiled/run in Unreal.** The native code targets the documented **UE 5.6** API. Review/build it for your engine version before relying on it. World Partition/tiled import is not implemented: use a basic non-partitioned editor level, not PIE.

## Install

1. Keep the complete `import-helpers` folder together. The Python script uses the dependency-free validator in `blender/openterrainstudio_import/ots_manifest.py`; it does not load Blender or require `bpy`.
2. Copy `OpenTerrainStudioImport/` into `<YourProject>/Plugins/`.
3. Build the project's **Editor** target with your Unreal C++ toolchain, then restart the editor and enable **OpenTerrainStudio Landscape Import Bridge**, **Python Editor Script Plugin**, and **Editor Scripting Utilities**. A Blueprint-only project may need a C++ class added to establish its build target.
4. Open a non-World-Partition level. Export heightfields as **16-bit PNG** at a legal Landscape resolution, such as 1009, 2017, 4033 or 8129.

The small native bridge is necessary because the [documented Landscape `Import` method](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Landscape/ALandscapeProxy/Import?application_version=5.6) creates the components from uint16 samples, while [stock Python's Landscape API](https://dev.epicgames.com/documentation/en-us/unreal-engine/python-api/class/LandscapeProxy?application_version=5.6) exposes render-target updates to existing landscapes. Spawning an empty actor alone would not create terrain. The bridge uses the engine's PNG Landscape importer and the native creation method; no speculative `unreal.LandscapeEditorSubsystem` call is used.

## Run

In the Unreal **Output Log**, switch its command mode to **Python**:

```python
import sys
sys.path.insert(0, r"/absolute/path/to/import-helpers/unreal")
import ots_import_build
result = ots_import_build.import_build(
    r"/absolute/path/to/export/build.json",
    "/Game/OpenTerrainStudio/Build001",
)
```

`result["landscapes"]` contains one actor per heightfield, keyed by output. `result["layer_weight_textures"]` contains every mask as a linear Texture2D asset with the Terrain Weightmap LOD group and node/port/build metadata. Alternative encodings are deduplicated, preferring PNG for Unreal. An EXR-only **heightfield** is rejected with a request to export PNG16; EXR-only masks are accepted by the texture import path.

Actors start at X/Y = 0, Z = `unreal.z_location`, with `unreal.z_scale`. X uses `unreal.xy_scale`; Y uses the same value for square cells, or `cell_size_m[1]×100` for rectangular worlds because the exporter writes only the X spacing into the single `xy_scale` hint. Each sample's world height is `(z_location + (v−32768)/128 × z_scale)/100` metres. The planner checks the hints against the metadata, including Rust's float32 range calculation, and selects a component layout fitting the pixels exactly. It never pads or resamples.

These are **layer-weight texture assets**, not automatically painted Landscape layers or a generated Landscape material. Connect their red channels in your material. PNG masks use grayscale compression and EXR masks use HDR compression, with sRGB off and no generated mipmaps. Unreal texture/cooking precision has not been measured; no claim of byte-identical GPU masks is made. The imported source files remain unchanged.

Several heightfields occupy the same world space; hide the alternatives you are not using. The script refuses existing destination assets rather than replacing them. It removes actors/assets created by this invocation if import fails, saves successful texture assets, and leaves the level unsaved for inspection.

## Validate without Unreal

```sh
python3 import-helpers/unreal/ots_import_build.py --dry-run /path/to/build.json
python3 -m unittest discover -s import-helpers/tests -p 'test_*.py' -v
```

The six boundary values from Rust's `unreal_hints_reproduce_png_heights` test are checked against float32 arithmetic, and every uint16 value is also checked against the analytic mapping. Tests cover legal/illegal component layouts, rectangular spacing, narrow elevated height ranges, missing files, wrong generators and multiple outputs. These checks **do not establish that the editor integration works**. Validate native compilation, actor/component creation, textures, undo and saving in a real UE 5.6 project before distributing this as a tested Unreal importer.
