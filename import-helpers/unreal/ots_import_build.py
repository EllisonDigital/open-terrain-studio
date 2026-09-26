# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""OpenTerrainStudio -> Unreal Editor. UNTESTED IN UNREAL.

Pure-Python plan/maths are tested. Creating Landscape components requires the
bundled editor-only C++ bridge (UE 5.6 API target), not a fictitious Python API.
Keep the import-helpers directory together; the shared validator ships in the
Blender add-on but imports no bpy. See README.md for install and execution.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import struct

_shared = Path(__file__).resolve().parents[1] / "blender" / "openterrainstudio_import"
if str(_shared) not in sys.path:
    sys.path.insert(0, str(_shared))
from ots_manifest import BuildError, load_build, select_outputs, read_png16, read_points, label


def expected_hints(info):
    # BuildInfo serialises f32s using their shortest decimal spelling. Recreate
    # Rust's f32 subtraction before widening, including narrow elevated ranges.
    f32 = lambda v: struct.unpack("<f", struct.pack("<f", v))[0]
    try:
        lo, hi = map(f32, info["height_range_m"])
        span = f32(hi - lo)
    except (OverflowError, struct.error) as exc:
        raise BuildError("height_range_m is outside float32 range") from exc
    return {"xy_scale": info["cell_size_m"][0] * 100.0,
            "z_scale": span * 12800.0 / 65535.0,
            "z_location": (lo + span * 32768.0 / 65535.0) * 100.0}


def unreal_height_m(value, hints):
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 65535:
        raise BuildError("Height sample must be an unsigned 16-bit integer")
    return (hints["z_location"] + (value - 32768.0) / 128.0 * hints["z_scale"]) / 100.0


def landscape_layout(resolution):
    """Choose exact legal components, never pad/resample the user's heights."""
    width, height = resolution
    candidates = []
    for quads in (255, 127, 63, 31, 15, 7):
        for sections in (2, 1):
            component = quads * sections
            nx, ny = (width - 1) // component, (height - 1) // component
            if (width - 1) % component == 0 and (height - 1) % component == 0 and 1 <= nx <= 32 and 1 <= ny <= 32:
                candidates.append((nx * ny, -sections, -quads, quads, sections, nx, ny))
    if not candidates:
        raise BuildError(f"{width}x{height} is not a supported Landscape layout; export 1009, 2017, 4033 or 8129 (no automatic resampling)")
    _, _, _, quads, sections, nx, ny = min(candidates)
    return {"section_size_quads": quads, "sections_per_component": sections, "components": [nx, ny]}


def asset_name(entry):
    text = f"{entry['node']}_{entry['port']}"
    clean = re.sub(r"[^A-Za-z0-9_]", "_", text)[:80]
    digest = hashlib.sha256(label(entry).encode("utf-8")).hexdigest()[:12]
    return "OTS_" + clean + "_" + digest


def plan_build(filename):
    info = load_build(filename)
    hints = info.get("unreal")
    if not isinstance(hints, dict):
        raise BuildError("build.json has no unreal import hints; re-export from OpenTerrainStudio")
    for key, expected in expected_hints(info).items():
        actual = hints.get(key)
        if isinstance(actual, bool) or not isinstance(actual, (int, float)) or not math.isfinite(actual):
            raise BuildError("Missing or invalid unreal." + key)
        if not math.isclose(actual, expected, rel_tol=1e-7, abs_tol=1e-5):
            raise BuildError(f"unreal.{key} disagrees with world metadata; re-export this build (old pre-v0.1 scale formula is not exact)")
    chosen = select_outputs(info, ("png16", "exr32", "csv", "json"))
    heightfields = [e for e in chosen if e["data"] == "heightfield"]
    masks = [e for e in chosen if e["data"] == "mask"]
    points = [e for e in chosen if e["data"] == "PointSet"]
    layout = landscape_layout(info["resolution"]) if heightfields else None
    for entry in chosen:
        if entry["data"] == "heightfield" and entry["format"] != "png16":
            raise BuildError("Unreal Landscape requires a PNG16 heightfield for " + label(entry))
        if entry["format"] == "png16":
            read_png16(entry["path"], info["resolution"])
        elif entry["data"] == "PointSet":
            read_points(entry, info)
    # OTS's xy_scale is derived from the X spacing. Rectangular worlds may need
    # a different Y scale; retain exact extents instead of forcing square cells.
    return {"source": info["manifest_path"], "resolution": info["resolution"],
            "world_size_m": info["world_size_m"],
            "scale_cm": [hints["xy_scale"], info["cell_size_m"][1] * 100.0, hints["z_scale"]],
            "location_cm": [0.0, 0.0, hints["z_location"]],
            "layout": layout, "heightfields": heightfields, "masks": masks, "points": points}


def instance_batches(entry, info, batch_size=8192):
    """Landscape's +X,+Y origin is the world corner; Y grows with image rows."""
    buckets = [[] for _ in entry["species"]]
    for x, y, z, degrees, scale, index in read_points(entry, info):
        buckets[index].append((x * 100.0, y * 100.0, z * 100.0, degrees, scale))
    for species, rows in zip(entry["species"], buckets):
        for start in range(0, len(rows), batch_size):
            yield species, rows[start:start + batch_size]


def import_build(filename, destination="/Game/OpenTerrainStudio/Import"):
    """Create actors and linear mask texture assets; return both keyed by output.

    Does not overwrite existing assets, save the level, generate a material, or
    convert masks into painted Landscape layers. Texture assets are the layer
    weights to connect in your Landscape material. UNTESTED IN UNREAL.
    """
    plan = plan_build(filename)  # All metadata/files validated before editor writes.
    if not re.fullmatch(r"/Game(?:/[A-Za-z0-9_]+)+", destination):
        raise BuildError("Destination must be a /Game/... folder using letters, digits and underscores")
    try:
        import unreal
    except ImportError as exc:
        raise RuntimeError("Run import_build inside Unreal Editor; use --dry-run outside it") from exc
    bridge = getattr(unreal, "OTSLandscapeLibrary", None)
    if (plan["heightfields"] or plan["points"]) and bridge is None:
        raise RuntimeError("Enable/build the bundled OpenTerrainStudioImport editor plugin and restart Unreal; Landscape creation is not exposed by stock Python")
    names = [asset_name(e) for e in plan["masks"]]
    if plan["points"] and not plan["heightfields"]:
        raise BuildError("PointSet import needs a Landscape output")
    if len(set(names)) != len(names):
        raise BuildError("Mask asset name collision")
    for name in names:
        path = destination + "/" + name
        if unreal.EditorAssetLibrary.does_asset_exist(path):
            raise BuildError("Asset already exists; choose a new destination folder: " + path)
    textures, actors, created_paths = {}, {}, []
    vegetation = {}
    tools = unreal.AssetToolsHelpers.get_asset_tools()
    try:
        with unreal.ScopedEditorTransaction("Import OpenTerrainStudio build"):
            for entry in plan["heightfields"]:
                layout = plan["layout"]
                actor = bridge.create_landscape_from_png(
                    entry["path"], plan["resolution"][0], plan["resolution"][1],
                    layout["sections_per_component"], layout["section_size_quads"],
                    unreal.Vector(*plan["scale_cm"]), unreal.Vector(*plan["location_cm"]),
                    asset_name(entry))
                if actor is None:
                    raise RuntimeError("Landscape creation failed; see the Unreal Output Log")
                actors[label(entry)] = actor
            for entry in plan["points"]:
                # Landscape position is the corner; +Y follows image rows.
                # The native bridge owns registration and batched insertion.
                if bridge is None:
                    raise RuntimeError("The native bridge is required for PointSet instances")
                for species in entry["species"]:
                    owner = next(iter(actors.values()), None)
                    if owner is None:
                        raise BuildError("PointSet import needs a Landscape output")
                    component = bridge.create_species_instances(owner, species)
                    if component is None:
                        raise RuntimeError("Could not create vegetation component")
                    vegetation[(label(entry), species)] = component
                for species, batch in instance_batches(entry, plan):
                    transforms = [unreal.Transform(location=unreal.Vector(x, y, z),
                                                 rotation=unreal.Rotator(0, yaw, 0),
                                                 scale=unreal.Vector(scale, scale, scale))
                                  for x, y, z, yaw, scale in batch]
                    bridge.add_species_instances(vegetation[(label(entry), species)], transforms)
            for entry in plan["masks"]:
                task = unreal.AssetImportTask()
                for key, value in {"filename": entry["path"], "destination_path": destination,
                                   "destination_name": asset_name(entry), "automated": True,
                                   "replace_existing": False, "save": False,
                                   "factory": unreal.TextureFactory()}.items():
                    task.set_editor_property(key, value)
                tools.import_asset_tasks([task])
                paths = list(task.get_editor_property("imported_object_paths"))
                created_paths.extend(paths)
                if len(paths) != 1:
                    raise RuntimeError("Expected one imported mask texture: " + entry["path"])
                texture = unreal.EditorAssetLibrary.load_asset(paths[0])
                if not isinstance(texture, unreal.Texture2D):
                    raise RuntimeError("Import did not create a Texture2D: " + paths[0])
                texture.set_editor_property("srgb", False)
                texture.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_HDR if entry["format"] == "exr32" else unreal.TextureCompressionSettings.TC_GRAYSCALE)
                texture.set_editor_property("lod_group", unreal.TextureGroup.TEXTUREGROUP_TERRAIN_WEIGHTMAP)
                texture.set_editor_property("mip_gen_settings", unreal.TextureMipGenSettings.TMGS_NO_MIPMAPS)
                unreal.EditorAssetLibrary.set_metadata_tag(texture, "OTS.Node", entry["node"])
                unreal.EditorAssetLibrary.set_metadata_tag(texture, "OTS.Port", entry["port"])
                unreal.EditorAssetLibrary.set_metadata_tag(texture, "OTS.Build", plan["source"])
                textures[label(entry)] = texture
            for texture in textures.values():
                if not unreal.EditorAssetLibrary.save_loaded_asset(texture):
                    raise RuntimeError("Could not save imported mask texture")
    except Exception:
        # Only objects created by this invocation are removed; user assets are
        # refused in preflight. Imported levels are deliberately not auto-saved.
        for actor in actors.values():
            unreal.get_editor_subsystem(unreal.EditorActorSubsystem).destroy_actor(actor)
        for path in created_paths:
            unreal.EditorAssetLibrary.delete_asset(path)
        raise
    unreal.log_warning("OpenTerrainStudio import helper: UNTESTED IN UNREAL; verify size, heights and masks before saving the level.")
    return {"landscapes": actors, "layer_weight_textures": textures, "vegetation": vegetation, "plan": plan}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="OpenTerrainStudio Unreal importer (untested in Unreal)")
    parser.add_argument("build_json")
    parser.add_argument("--dry-run", action="store_true", help="Validate files/scales/layout without Unreal")
    parser.add_argument("--destination", default="/Game/OpenTerrainStudio/Import")
    args = parser.parse_args()
    if args.dry_run:
        print(json.dumps(plan_build(args.build_json), indent=2))
    else:
        import_build(args.build_json, args.destination)
