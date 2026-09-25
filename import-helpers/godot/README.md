<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Godot editor plugin

1. Copy `addons/openterrainstudio_import/` into your project's `addons/` folder. Do not replace your project's `project.godot`; the one here is only a standalone test project.
2. Enable **OpenTerrainStudio build importer** in **Project → Project Settings → Plugins**.
3. Choose **Project → Tools → Import OpenTerrainStudio build…**. Select the external `build.json`, then a destination scene inside your project. Prefer `.scn` (binary) for large terrains; `.tscn` is also supported.

The saved scene contains one `MeshInstance3D` per heightfield at its original resolution. One unit is one metre. X is centred along ±world width/2, Y is elevation, and the first image row is the −Z edge. Heights and normals are baked into an `ArrayMesh`; the shader previews terrain or a selected mask. Source images are loaded directly and embedded as `FORMAT_RF` textures, avoiding the editor's default PNG/EXR import settings.

Select the root node to inspect **Height Textures** and **Masks**, keyed by `["node","port"]`. Paste a mask's key into **Preview Mask** to see it on the terrain, or clear the field for terrain shading. Drag/copy the texture resources into your own materials. Keep the add-on's runtime `terrain_build.gd` and `terrain.gdshader` with scenes that reference them.

Unlike Godot's standard eight-bit PNG path, this helper explicitly decodes OTS grayscale16 PNG into float samples before remapping heights. EXR remains the preferred source and retains the original float samples. Export both formats if you also need Unreal.

Import creates a new scene, not nodes inside your currently open scene. There is no collision, automatic reimport, LOD or tiling; add these using your game's terrain/physics approach. The helper currently caps images at 4,194,304 samples to limit full-resolution mesh allocations. Several exported terrain alternatives occupy the same space; choose which one to show.

Headless [tests](../README.md#tests) passed on Godot 4.7.2 and 4.6.2 on 25 September 2026, including binary-scene reload, float texture precision, multi-output manifests, and lossless PNG16 decoding. Interactive dialogs and GPU rendering were not exercised.
