# Colour and texturing (v0.6)

Colour work lives in the **Colour** tab. Terrain outputs arrive through portals: select a
Terrain node, then press **Send … to Colour tab** in its inspector. The portal references the
original output rather than copying it. Colour edits therefore reuse cached terrain results.
The tab is stored per node in `.otstudio` projects; older projects open in the Terrain tab.

## Nodes

| Node | Use |
| --- | --- |
| Colourise | Map a heightfield or mask to a custom gradient or a natural preset (terrain, grass, rock, sand, snow, desert, water). |
| Blend Colours | Put one colour map over another through an optional mask, with opacity and normal, multiply, screen or overlay blending. |
| Colour Layers | Stack a base and up to four optional colour/mask pairs. |
| Colour Image | Stretch a user-supplied PNG, JPEG or EXR over the world; adjust brightness and saturation or flip vertically. |
| Normal Map | Export tangent-space normals from terrain, with OpenGL (Godot/Blender) or DirectX (Unreal) green-channel convention. |
| Occlusion | A cavity/open-sky mask from hollows at three scales set by Radius. |
| Splat Map | Normalise up to eight material masks to sum to one; output individual masks and two RGBA packs (layers 1–4 and 5–8). Unconnected layer 1 fills the remaining weight. |

Gradient stops are `[position, red, green, blue]`, with values in 0–1. The inspector lets you
add, remove, position and recolour stops. Colour maps contain RGBA samples with sRGB-encoded
colours; packed weights and normal maps use the same four channels as data, not display colour.
Colour maps only connect to colour inputs; heightfields and masks still convert between each
other as before.

Select a colour output to see it directly in the 2D map or draped over its upstream terrain
in 3D. Use a **Height Portal** to bring terrain into the Colour tab, then connect it to
Colourise, Normal Map or Occlusion. Use a **Mask Portal** for e.g. slope, river, snow or sea
masks. The *River coast* example includes a Colour tab with terrain colours, river and lake
water, snow, normal map and four-layer splat map.

## Export

Mark an output in the inspector or Build tab. Colour maps, normal maps and packed weights
export as **RGBA PNG 8-bit**, **RGBA PNG 16-bit** or **RGBA EXR 32-bit float**. Scalar masks
and heightfields remain greyscale. `png8` files end `_8bit.png`, so marking both PNG bit depths
does not overwrite either file. `build.json` records each file's type and encoding.

For a four-material landscape in Unreal or Godot:

1. Export the final terrain Height as PNG 16-bit (and EXR 32-bit if needed).
2. Feed material masks into Splat Map: layer 1 = base/grass, layer 2 = rock, layer 3 = shore,
   layer 4 = snow. Export **Weights 1–4** as RGBA PNG 8-bit. R, G, B and A correspond to
   layers 1–4 and sum to one at each pixel. Alternatively export each Weight mask separately.
3. Export a Normal Map as PNG 8-bit: choose DirectX for Unreal, OpenGL for Godot or Blender.
4. Export the finished colour map as PNG 8-bit. In an engine, use it as the terrain albedo or
   for reference while assigning the materials to the weight-map channels.

Import the heightmap first, then add a landscape material with four layers. Assign each RGBA
weight channel to the corresponding material layer and plug the colour and normal maps into
the material's colour and normal inputs. Match the XY scale to `build.json`'s `cell_size_m`;
the orientation is row 0 = world Y 0, column 0 = world X 0 (top-left image pixel = world
origin). For engine-specific heightmap scaling, see [export guides](export-guides.md).

**Limits:** colour nodes run on the CPU; the 3D preview still draws water only when a
heightfield is selected. Satellite images are stretched over the whole world, not
georeferenced. Occlusion is a multiscale cavity approximation, not ray-traced AO. The
written guide has not yet been timed with an Unreal and Godot import, so the roadmap's
under-10-minute exit criterion remains unverified.
