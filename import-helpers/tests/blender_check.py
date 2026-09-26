# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""blender --background --factory-startup --python-exit-code 1 --python THIS -- EXPORT_DIR [single]"""
from array import array
import json
import math
from pathlib import Path
import sys
import time
import bpy
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "blender"))
import openterrainstudio_import as addon
from openterrainstudio_import.ots_manifest import BuildError, select_outputs, load_build, read_points

folder = Path(sys.argv[sys.argv.index("--") + 1])
single_only = sys.argv[-1] == "single"
addon.register()
reports = []
for filename in (["single.json"] if single_only else ["build.json", "png-only.json", "single.json", "masks-only.json"]):
    info = load_build(folder / filename)
    collection = addon.import_build(bpy.context, folder / filename)
    selected = select_outputs(info)
    heights = [f for f in selected if f["data"] == "heightfield"]
    masks = [f for f in selected if f["data"] == "mask"]
    assert len(collection.objects) == len(heights)
    width, height = info["resolution"]
    worst_height, worst_xy, worst_mask = 0.0, 0.0, 0.0
    for obj in collection.objects:
        entry = next(f for f in heights if (f["node"], f["port"]) == (obj["ots_node"], obj["ots_port"]))
        reference = array("f")
        reference.frombytes((folder / (entry["node"] + "_" + entry["port"] + ".f32")).read_bytes())
        # The expected buffer comes directly from the Rust input Grid, not the importer.
        assert len(obj.data.vertices) == len(reference)
        for i, vertex in enumerate(obj.data.vertices):
            row, col = divmod(i, width)
            expected_x = col * info["cell_size_m"][0] - info["world_size_m"][0] / 2
            expected_y = info["world_size_m"][1] / 2 - row * info["cell_size_m"][1]
            worst_xy = max(worst_xy, abs(vertex.co.x - expected_x), abs(vertex.co.y - expected_y))
            worst_height = max(worst_height, abs(vertex.co.z - reference[i]))
        # Faces face up, and masks match every vertex in the same orientation.
        assert all(face.normal.z > 0 for face in obj.data.polygons)
        for mask in masks:
            reference = array("f")
            reference.frombytes((folder / (mask["node"] + "_" + mask["port"] + ".f32")).read_bytes())
            attr = obj.data.attributes["OTS " + addon.label(mask)]
            assert attr.domain == "POINT" and attr.data_type == "FLOAT"
            for actual, expected in zip(attr.data, reference):
                worst_mask = max(worst_mask, abs(actual.value - expected))
        texture_nodes = [n for n in obj.data.materials[0].node_tree.nodes if n.type == "TEX_IMAGE"]
        assert len(texture_nodes) == len(masks)
    height_tolerance = (info["height_range_m"][1] - info["height_range_m"][0]) / 65535 / 2 + 0.001 if filename == "png-only.json" else 0.001
    assert worst_height <= height_tolerance, (filename, worst_height)
    assert worst_xy <= 0.001
    assert worst_mask <= (0.5 / 65535 + 1e-7 if filename == "png-only.json" else 1e-7)
    reports.append({"manifest": filename, "vertices_per_terrain": width * height, "terrains": len(heights), "masks": len(masks), "max_height_error_m": worst_height, "max_xy_error_m": worst_xy, "max_mask_error": worst_mask})
    # Avoid retaining multi-million-vertex fixtures between cases.
    for obj in list(collection.objects):
        mesh = obj.data
        bpy.data.objects.remove(obj, do_unlink=True)
        bpy.data.meshes.remove(mesh)
    bpy.data.collections.remove(collection)
if not single_only:
    for name in ["bad-generator", "missing", "bad-orientation", "bad-resolution"]:
        before = len(bpy.data.objects)
        try:
            addon.import_build(bpy.context, folder / (name + ".json"))
        except BuildError:
            pass
        else:
            raise AssertionError("accepted " + name)
        assert len(bpy.data.objects) == before
    # Honour an existing centimetre scene, without altering it.
    bpy.context.scene.unit_settings.scale_length = 0.01
    c = addon.import_build(bpy.context, folder / "single.json")
    assert abs(c.objects[0].scale.x - 100) < 0.001
addon.unregister()
for name in ("points", "points-million"):
    path = folder / (name + "-build.json")
    if not path.exists():
        continue
    info = load_build(path)
    entry = next(e for e in select_outputs(info) if e["data"] == "PointSet")
    rows = read_points(entry, info)
    started = time.perf_counter()
    collection = addon.import_build(bpy.context, path)
    duration = time.perf_counter() - started
    point_objects = [obj for obj in collection.objects if "ots_species" in obj]
    assert len(point_objects) == len(entry["species"])
    terrain = next(obj for obj in collection.objects if obj.get("ots_node") == "terrain_a")
    sx, sy = info["world_size_m"]
    buckets = [[r for r in rows if r[5] == index] for index in range(len(entry["species"]))]
    for obj in point_objects:
        index = entry["species"].index(obj["ots_species"])
        assert len(obj.data.vertices) == len(buckets[index])
        assert obj.modifiers[0].type == "NODES"
        yaw = obj.data.attributes["rotation"]
        scales = obj.data.attributes["scale"]
        for i, row in enumerate(buckets[index]):
            x, y, z, degrees, scale, _ = row
            point = obj.matrix_world @ obj.data.vertices[i].co
            assert max(abs(point.x - (x - sx/2)), abs(point.y - (sy/2 - y)), abs(point.z - z)) < 1e-3
            assert abs(yaw.data[i].value + math.radians(degrees)) < 1e-4
            assert abs(scales.data[i].value - scale) < 1e-5
            col = round(x / info["cell_size_m"][0])
            grid_row = round(y / info["cell_size_m"][1])
            imported_height = terrain.data.vertices[grid_row * info["resolution"][0] + col].co.z
            assert abs(point.z - imported_height) <= (info["height_range_m"][1] - info["height_range_m"][0]) / 65535 / 2 + 0.001
    reports.append({"manifest": path.name, "points": len(rows), "seconds": duration})
    if len(rows) >= 1_000_000:
        assert duration < 30, duration
    for obj in list(collection.objects):
        mesh = obj.data
        bpy.data.objects.remove(obj, do_unlink=True)
        bpy.data.meshes.remove(mesh)
    bpy.data.collections.remove(collection)
print("BLENDER_IMPORT_REPORT " + json.dumps({"blender": bpy.app.version_string, "results": reports}))
