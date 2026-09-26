# Export guides: Blender, Unreal Engine and Godot

How to bring an OpenTerrainStudio heightmap into Blender, Unreal Engine and Godot at the right scale.

**File → Export Viewed Node** writes the node you are viewing. To export several outputs at once (say a
heightmap and a slope mask), tick a format under *Export* in each node's settings and press *Build* in the
**Build** tab (<kbd>Ctrl</kbd>+<kbd>B</kbd>): every marked output is written at the build resolution, with one
`build.json` listing them all. Masks are written as 0..1 (EXR) or 0..65535 (PNG). For each output you get:

| File | Contents |
| --- | --- |
| `<node>-<id>_<port>_<res>.exr` | 32-bit float, one channel (`Y`), **heights in metres** |
| `<node>-<id>_<port>_<res>.png` | 16-bit greyscale, 0 = lowest world height, 65535 = highest |
| `build.json` | World size, height range, resolution, cell size, seed, file list, Unreal import values |

The world spans `world_size_m`. Samples sit on the world's corners and edges, so the spacing is
`cell_size_m = world_size_m / (resolution − 1)`: a 1009² export of an 8,192 m world has 8.127 m cells.
Row 0 of each image is world Y = 0 and column 0 is world X = 0.

## What was tested

Tested on 24 Sep 2026 with the packaged Linux build (v0.1.0, Godot 4.7.2): starter graph (fBm → Levels),
8,192 m world, heights 0–2,000 m, seed 12345, exported at 1009² as EXR and PNG. That `build.json`:

```json
{
  "world_size_m": [8192.0, 8192.0],
  "height_range_m": [0.0, 2000.0],
  "resolution": [1009, 1009],
  "cell_size_m": [8.126984126984127, 8.126984126984127],
  "unreal": {
    "xy_scale": 812.6984126984127,
    "z_scale": 390.6309605554284,
    "z_location": 100001.52590218966
  }
}
```

Exporting again from a second build of the app gave byte-identical EXR and PNG files.

| Target | How it was checked | Result |
| --- | --- | --- |
| Blender 5.2 LTS | [`tests/import/blender_check.py`](../tests/import/blender_check.py): grid + Displace modifier as below, then every evaluated vertex compared with its EXR pixel | 1,018,081 vertices, X/Y −4,096…4,096 m, Z 0…2,000 m; max error 0.12 mm (float rounding) |
| Godot 4.7.2 | [`tests/import/godot_check.gd`](../tests/import/godot_check.gd) plus an editor import of both files | EXR exact at runtime and after the default editor import; **16-bit PNG read as 8-bit** (up to 3.9 m error) |
| Unreal Engine | **Not tested in Unreal Editor** (none available). The import values were checked against Epic's documentation and a unit test (`unreal_hints_reproduce_png_heights`) | See the Unreal section |

Re-run the checks on your own export:

```sh
blender --background --factory-startup --python tests/import/blender_check.py -- <export folder>
godot --headless --path app --script res://../tests/import/godot_check.gd -- <export folder>
```

---

## Blender (EXR displacement)

Blender units are metres, and the EXR stores metres, so a displacement strength of 1 gives the true height.

1. **Add → Mesh → Grid.** In the *Adjust Last Operation* panel set:
   - *X Subdivisions* and *Y Subdivisions* = `resolution − 1` (1008 for a 1009² export). This makes one vertex
     per pixel.
   - *Size* = `world_size_m` (8192 m).
2. **Add Modifier → Deform → Displace**, then click *New* to create a texture.
3. In the **Texture** properties: *Type* Image or Movie, *Open* the `.exr`, then:
   - *Color Space*: **Non-Color** (heights must not be colour-managed).
   - *Sampling → Interpolation*: **off**, so each vertex reads its own pixel.
   - *Mapping → Extension*: **Extend**.
4. Back in the **Displace** modifier: *Coordinates* **UV**, *Direction* Normal, *Strength* **1.0**, *Midlevel* **0.0**.
5. The viewport clips at 1 km by default: raise *View → Clip End* (N panel) to 20,000 m or more to see the whole
   terrain.

**Orientation:** with a centred grid, world X increases along Blender +X, and world Y = 0 (the top row of the
image) is at the **+Y** edge. So pixel (column *i*, row *j*) lands at
`x = i·cell − size/2`, `y = size/2 − j·cell`, `z = height in metres`.

**Interpolation matters.** With texture interpolation on (Blender's default), each vertex blends
neighbouring pixels. On the test terrain that put vertices a median 1.3 m (max 15 m) off their true height.
With it off, they matched to 0.12 mm.

For renders, a *Subdivision Surface* modifier before *Displace* adds detail between samples.

---

## Unreal Engine (16-bit PNG)

Use the **16-bit PNG** for landscapes. Unreal Engine also reads RAW/R16, which is planned for a later
version.

1. Export at an Unreal landscape size, marked *(Unreal landscape size)* in the export dialog:
   1009, 2017, 4033 or 8129.
2. **Landscape mode → Manage → New → Import from File**, then choose the `.png` as the heightmap.
3. Type the values from `build.json` → `unreal`:

| Import field | Value | Test export |
| --- | --- | --- |
| Location X / Y | 0 (or anywhere; the first vertex sits here) | 0 |
| Location Z | `z_location` | 100001.526 |
| Scale X / Y | `xy_scale` | 812.698 |
| Scale Z | `z_scale` | 390.631 |

**Where the numbers come from.** Unreal units are centimetres. Epic's
[Landscape Technical Guide](https://dev.epicgames.com/documentation/en-us/unreal-engine/landscape-technical-guide-in-unreal-engine)
says a 16-bit heightmap covers −256 m to 255.992 m at Z scale 100. In other words, value `v` becomes
`(v − 32768) / 128 × ZScale` centimetres above the landscape's Z location. OpenTerrainStudio writes
`v = (h − min) / span × 65535`, so matching the two gives:

- `xy_scale = cell_size_m × 100`: centimetres between samples. The landscape spans `(resolution − 1) × cell` =
  the world size.
- `z_scale = span_m × 12800 / 65535`: 390.631 for a 2,000 m range; Z scale 100 ≈ a 512 m range.
- `z_location = (min_m + span_m × 32768 / 65535) × 100`: Unreal puts value 32768 at the landscape's Z location,
  so Z location is the height that value represents.

With these values, every 16-bit value lands on the height OpenTerrainStudio meant, with no drift across the
range. Heights are quantised to `span / 65535` (3 cm for a 2,000 m range).

> **Fixed before v0.1.0:** development builds wrote `z_scale = span × 100 / 512` and
> `z_location = (min + span / 2) × 100`. That treated the PNG as if 65536 steps covered the range, so the top
> of the terrain came out one 16-bit step (≈ 3 cm per 2,000 m) too low.

**Orientation:** column 0 is Unreal X = 0 and row 0 is Unreal Y = 0, with rows running towards +Y.

---

## Godot 4 (EXR)

**Use the EXR, not the PNG.** Godot reads 16-bit PNGs as 8-bit (`Image.FORMAT_L8`): with a 2,000 m range,
that is 7.8 m steps, and heights on the test export were off by up to 3.9 m. The EXR loads as `FORMAT_RGBF`
with heights exact to the metre value in the file.

**Load at runtime** (for example in a tool script or a game):

```gdscript
var img := Image.load_from_file("res://terrain/levels-n_0002_out_1009.exr")  # RGBF, metres
img.convert(Image.FORMAT_RF)                                                  # one channel
```

**Or import in the editor.** The default import (*Texture2D*, *Compress → Mode: Lossless*) keeps the EXR exact.
Don't switch it to *VRAM Compressed*, which is lossy.

**Collision** with `HeightMapShape3D` (one sample per unit). Store heights divided by the cell size, then
scale the shape uniformly by the cell size, because non-uniform scaling of collision shapes isn't reliable:

```gdscript
var info: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(dir.path_join("build.json")))
var cell: float = info["cell_size_m"][0]
var shape := HeightMapShape3D.new()
shape.update_map_data_from_image(img, 0.0, 1.0 / cell)  # stores each height / cell
var body_shape := CollisionShape3D.new()
body_shape.shape = shape
body_shape.scale = Vector3.ONE * cell                   # 1009 samples -> 8,192 m, heights in metres
```

The shape is centred on its node, so it spans `±world_size_m / 2`. Its data is row-major like the image
(checked), so row 0 (world Y = 0) is the −Z edge and world X runs along +X.

**Rendering:** displace a subdivided `PlaneMesh` in a vertex shader that samples the EXR texture, as the
OpenTerrainStudio viewport does (`app/shaders/terrain.gdshader`), or use a terrain plugin that accepts a float
heightmap. Use `filter_nearest` when mesh vertices line up one-to-one with pixels, and `filter_linear` otherwise.

## Import helpers

The helpers also read vegetation `PointSet` outputs in CSV or JSON (alternatives
for the same node/port); see the [point contract](../import-helpers/points-format.md).
They place one instance per point using a replaceable cone placeholder: Blender
Geometry Nodes, Godot MultiMesh, and Unreal HISM batches (the latter untested in
Unreal). CSV/JSON validation rejects bad counts, bounds, species and non-finite
values. The point import and million-point performance checks for Blender/Godot
have not been run on this machine; Python parser tests and an Unreal dry run passed.

The optional [import helpers](../import-helpers/README.md) read `build.json` and its listed images,
including multi-output builds, masks and older single-output exports. They reject a generator other
than `OpenTerrainStudio`, missing files, and unsupported or contradictory metadata. Keep the image
files beside the manifest. Different height outputs become separate terrains at the same world
position; alternative EXR/PNG encodings of one output are deduplicated.

- **Blender:** run `python3 import-helpers/blender/package.py`, install the generated
  `import-helpers/.work/openterrainstudio-blender.zip` through **Preferences → Add-ons → Install from
  Disk**, and enable it. Use **File → Import → OpenTerrainStudio build (build.json)**. The add-on creates
  centred meshes in metres, respects an existing scene unit scale, and exposes masks as packed
  Non-Color images and point attributes. See the [Blender instructions](../import-helpers/blender/README.md).
- **Godot:** copy `import-helpers/godot/addons/openterrainstudio_import` into your project's `addons/`,
  enable it under **Project Settings → Plugins**, then use **Project → Tools → Import OpenTerrainStudio
  build…**. Choose the manifest and a destination `.scn`/`.tscn`. The scene contains terrain meshes and
  embedded float height/mask textures; select the root's **Masks** and **Preview Mask** properties.
  See the [Godot instructions](../import-helpers/godot/README.md). The helper has its own lossless PNG16
  decoder, so the ordinary Godot PNG precision limitation above does not apply to this import path.
- **Unreal — untested in Unreal:** copy the bundled `OpenTerrainStudioImport` native plugin into your
  project's `Plugins/`, build its Editor target, and enable it plus Python Editor Script Plugin and
  Editor Scripting Utilities. From the Python console, import `ots_import_build` from
  `import-helpers/unreal` and call `import_build(build_json_path, "/Game/OpenTerrainStudio/Build001")`.
  Keep the whole helper folder available for its shared validator. Use a non-World-Partition level and
  PNG16 heightfields at valid Landscape sizes. The script uses the recorded Unreal scales and creates
  linear layer-weight texture assets for masks; it does not author a Landscape material or painted
  layers. See the [Unreal setup and limitations](../import-helpers/unreal/README.md).

**Tested on Linux, 25 September 2026:** Blender **5.2.0 LTS**, Godot **4.7.2** and **4.6.2**, and plain
Python **3.13.5**. Tests use the actual Rust export writer, two terrains and two masks, a rectangular
world, negative heights, both formats, single-output/mask-only builds, and invalid/missing files.
Every 129² EXR vertex and mask sample matched the source exactly. PNG-only heights differed by at
most **0.018433 m** over a 2,400 m range, consistent with 16-bit quantisation and float rounding.
Godot passed saved-scene reload checks. The packaged Blender add-on passed operator import and
`.blend` save/reload checks for every vertex, mask attribute and packed image pixel. A **1009²** EXR import in Blender 5.2 and Godot 4.7.2
matched all **1,018,081** vertex heights exactly, with at most **0.0301 mm** horizontal rounding error.
Python scale/layout tests and the unchanged Rust `unreal_hints_reproduce_png_heights` test passed.
Reproduction commands and full results are in the [helper test guide](../import-helpers/README.md#tests).

**Not tested:** Unreal Editor (including native bridge compilation; its API target is UE **5.6**),
interactive file-dialog clicks, GPU appearance, packaged games, Windows/macOS, or other Blender
versions. Blender/Godot helpers currently build full-resolution meshes up to 4,194,304 vertices each;
there is no tiling/LOD, and the Godot helper does not create collision. The Unreal script and bridge
must be validated in an Unreal project before being described as a tested importer.
