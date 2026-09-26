<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Blender add-on

1. From the repository root, run `python3 import-helpers/blender/package.py`. It creates `import-helpers/.work/openterrainstudio-blender.zip` with the add-on and both licence files.
2. In Blender, open **Edit → Preferences → Add-ons**, choose **Install from Disk** in its menu, select the ZIP, and enable **OpenTerrainStudio build importer**. This is a legacy add-on, not a Blender Extensions repository package.
3. Use **File → Import → OpenTerrainStudio build (build.json)** and choose the exported manifest.

Each height output creates one mesh in an `OpenTerrainStudio build` collection. EXR is preferred when an output has both formats. Coordinates are `X = column×cell_x − width/2`, `Y = depth/2 − row×cell_y`, `Z = height_m`: the same centred orientation as the original Blender export check. Vertex heights are baked; no modifier interpolation changes the samples. Existing scene unit scale is respected through object scale, rather than changing the rest of your scene. Increase viewport clipping for large worlds.

All masks are packed, **Non-Color** images. Pick them from the image selector, or use the labelled Image Texture nodes in the imported **OTS mask textures** material. They are intentionally unconnected so the helper does not choose a terrain material for you. On each terrain they are also named FLOAT/POINT attributes (`OTS ["node", "port"]`), selectable in Geometry Nodes or via a named attribute. UVs sample texel centres. A mask-only build creates the images/material and an empty collection.

All imported images are packed for portability, including PNG data converted directly from uint16 into a float image. No source PNG is routed through an eight-bit Blender image buffer. Undo the import as one operator step; rerunning imports a new collection and leaves earlier imports intact.

Headless verification: [tests and measured limits](../README.md#tests). Tested with Blender 5.2.0 LTS on 25 September 2026; the add-on declares Blender 4.2 as its API minimum, but older versions were not run.
### Vegetation points

PointSet CSV/JSON outputs produce one mesh object per species, with one vertex per point and `rotation` (radians) and `scale` point attributes. Its Geometry Nodes **Instance on Points** modifier references an unlinked placeholder cone through an Object Info node. Replace that Object Info object's reference with your species object (nominal size at scale 1). Points share the centred, metre-scaled terrain frame; the source rotation is reflected in Blender's Y axis. No object is created per point. See [points-format.md](../points-format.md).

The 5,000/1,000,000-point headless checks in `tests/blender_check.py` are prepared but **not run on Windows 26 September 2026** (Blender executable unavailable). The <30 s target is unverified on this machine.
