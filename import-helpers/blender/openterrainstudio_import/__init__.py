# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
"""File > Import > OpenTerrainStudio build. One vertex per exported sample."""
bl_info = {
    "name": "OpenTerrainStudio build importer",
    "author": "EllisonDigital",
    "version": (0, 1, 0),
    "blender": (4, 2, 0),
    "location": "File > Import > OpenTerrainStudio build (build.json)",
    "description": "Import terrain and masks in metres, preserving EXR/PNG16 precision",
    "category": "Import-Export",
}

from array import array
import math
import bpy
from bpy.props import StringProperty
from bpy_extras.io_utils import ImportHelper
from .ots_manifest import BuildError, load_build, select_outputs, label, read_png16, read_points


def read_output(entry, info):
    width, height = info["resolution"]
    if entry["format"] == "png16":
        _, _, integers = read_png16(entry["path"], info["resolution"])
        values = [v / 65535.0 for v in integers]
        # Float image avoids Blender's byte image paths, regardless of version.
        image = bpy.data.images.new(label(entry), width, height, alpha=False, float_buffer=True)
        rgba = array("f", [0.0]) * (width * height * 4)
        for row in range(height):
            for col in range(width):
                index = ((height - 1 - row) * width + col) * 4
                value = values[row * width + col]
                rgba[index:index + 4] = array("f", (value, value, value, 1.0))
        image.colorspace_settings.name = "Non-Color"
        image.pixels.foreach_set(rgba)
        image.file_format = "OPEN_EXR"
        image.pack()
        if entry["data"] == "heightfield":
            lo, hi = info["height_range_m"]
            values = [lo + v * (hi - lo) for v in values]
    else:
        image = bpy.data.images.load(entry["path"], check_existing=False)
        image.colorspace_settings.name = "Non-Color"
        if list(image.size) != info["resolution"] or not image.is_float:
            bpy.data.images.remove(image)
            raise BuildError("EXR must be full-float and match build resolution: " + entry["file"])
        rgba = array("f", [0.0]) * len(image.pixels)
        image.pixels.foreach_get(rgba)
        channels = image.channels
        values = [rgba[((height - 1 - row) * width + col) * channels]
                  for row in range(height) for col in range(width)]
        image.pack()
    if not all(math.isfinite(v) for v in values):
        bpy.data.images.remove(image)
        raise BuildError("Nonfinite samples in " + entry["file"])
    if entry["data"] == "mask" and any(v < 0 or v > 1 for v in values):
        bpy.data.images.remove(image)
        raise BuildError("Mask values must be 0..1: " + entry["file"])
    image.use_fake_user = True
    image["ots_node"] = entry["node"]
    image["ots_port"] = entry["port"]
    image["ots_data"] = entry["data"]
    image["ots_encoding"] = entry["encoding"]
    image["ots_source_file"] = entry["path"]
    return image, values


def make_mesh(name, heights, info):
    width, height = info["resolution"]
    sx, sy = info["world_size_m"]
    dx, dy = info["cell_size_m"]
    vertices = array("f")
    for row in range(height):
        for col in range(width):
            vertices.extend((col * dx - sx / 2, sy / 2 - row * dy, heights[row * width + col]))
    loops = array("i")
    for row in range(height - 1):
        for col in range(width - 1):
            a = row * width + col
            loops.extend((a, a + width, a + width + 1, a + 1))
    mesh = bpy.data.meshes.new(name)
    mesh.vertices.add(width * height)
    mesh.vertices.foreach_set("co", vertices)
    mesh.loops.add(len(loops))
    mesh.loops.foreach_set("vertex_index", loops)
    mesh.polygons.add((width - 1) * (height - 1))
    mesh.polygons.foreach_set("loop_start", array("i", range(0, len(loops), 4)))
    mesh.polygons.foreach_set("loop_total", array("i", [4]) * len(mesh.polygons))
    uv = mesh.uv_layers.new(name="OTS samples")
    coords = array("f")
    for i in loops:
        coords.extend(((i % width + 0.5) / width, 1.0 - (i // width + 0.5) / height))
    uv.data.foreach_set("uv", coords)
    mesh.update()
    return mesh


def make_points(collection, entry, points, info, unit_scale):
    sx, sy = info["world_size_m"]
    species = entry["species"]
    buckets = [[] for _ in species]
    for x, y, z, degrees, scale, index in points:
        buckets[index].append((x - sx / 2, sy / 2 - y, z, -math.radians(degrees), scale))
    for index, name in enumerate(species):
        rows = buckets[index]
        mesh = bpy.data.meshes.new("OTS points " + name)
        mesh.vertices.add(len(rows))
        coords, rotations, scales = array("f"), array("f"), array("f")
        for x, y, z, yaw, scale in rows:
            coords.extend((x, y, z))
            rotations.append(yaw)
            scales.append(scale)
        mesh.vertices.foreach_set("co", coords)
        mesh.attributes.new("rotation", "FLOAT", "POINT").data.foreach_set("value", rotations)
        mesh.attributes.new("scale", "FLOAT", "POINT").data.foreach_set("value", scales)
        obj = bpy.data.objects.new("OTS " + name, mesh)
        collection.objects.link(obj)
        obj.scale = (1.0 / unit_scale,) * 3
        obj["ots_species"] = name
        obj["ots_node"], obj["ots_port"] = entry["node"], entry["port"]
        # The source object is deliberately not linked: replace it in Object Info
        # without modifying the point mesh or regenerating the node group.
        cone_mesh = bpy.data.meshes.new("OTS placeholder cone " + name)
        cone_mesh.from_pydata([(0, 0, 0), (-0.3, -0.3, 1), (0.3, -0.3, 1), (0, 0.3, 1)],
                              [], [(0, 1, 2), (0, 2, 3), (0, 3, 1), (1, 3, 2)])
        cone = bpy.data.objects.new("OTS placeholder " + name, cone_mesh)
        group = bpy.data.node_groups.new("OTS instances " + name, "GeometryNodeTree")
        group.interface.new_socket(name="Geometry", in_out="INPUT", socket_type="NodeSocketGeometry")
        group.interface.new_socket(name="Geometry", in_out="OUTPUT", socket_type="NodeSocketGeometry")
        nodes, links = group.nodes, group.links
        source = nodes.new("NodeGroupInput")
        output = nodes.new("NodeGroupOutput")
        instance = nodes.new("GeometryNodeInstanceOnPoints")
        object_info = nodes.new("GeometryNodeObjectInfo")
        object_info.inputs["Object"].default_value = cone
        object_info.transform_space = "ORIGINAL"
        yaw = nodes.new("GeometryNodeInputNamedAttribute")
        yaw.data_type = "FLOAT"
        yaw.inputs["Name"].default_value = "rotation"
        vector = nodes.new("ShaderNodeCombineXYZ")
        size = nodes.new("GeometryNodeInputNamedAttribute")
        size.data_type = "FLOAT"
        size.inputs["Name"].default_value = "scale"
        links.new(source.outputs["Geometry"], instance.inputs["Points"])
        links.new(object_info.outputs["Geometry"], instance.inputs["Instance"])
        links.new(yaw.outputs["Attribute"], vector.inputs["Z"])
        links.new(vector.outputs["Vector"], instance.inputs["Rotation"])
        links.new(size.outputs["Attribute"], instance.inputs["Scale"])
        links.new(instance.outputs["Instances"], output.inputs["Geometry"])
        modifier = obj.modifiers.new("OTS instance on points", "NODES")
        modifier.node_group = group


def import_build(context, filename):
    info = load_build(filename)  # Preflight all named files before creating geometry.
    if math.prod(info["resolution"]) > 4_194_304:
        raise BuildError("This importer is limited to 4,194,304 vertices per terrain; export a smaller build")
    # Validate/read every output before linking anything into the scene.
    outputs = []
    point_outputs = []
    try:
        for entry in select_outputs(info):
            if entry["data"] == "PointSet":
                point_outputs.append((entry, read_points(entry, info)))
            else:
                outputs.append((entry, *read_output(entry, info)))
    except Exception:
        for _, image, _ in outputs:
            bpy.data.images.remove(image)
        raise
    collection = bpy.data.collections.new("OpenTerrainStudio build")
    context.scene.collection.children.link(collection)
    collection["ots_build"] = info["manifest_path"]
    masks = [(entry, image, values) for entry, image, values in outputs if entry["data"] == "mask"]
    material = bpy.data.materials.new("OTS mask textures")
    material.use_nodes = True
    material.use_fake_user = True  # Also retain mask-only builds.
    for index, (entry, image, _) in enumerate(masks):
        node = material.node_tree.nodes.new("ShaderNodeTexImage")
        node.name = node.label = label(entry)
        node.image = image
        node.interpolation = "Closest"
        node.extension = "EXTEND"
        node.location = (-600, -300 * index)
    # Do not change scene unit settings; a pre-existing project may use cm.
    unit_scale = context.scene.unit_settings.scale_length
    for entry, _, values in outputs:
        if entry["data"] != "heightfield":
            continue
        name = "OTS " + label(entry)
        mesh = make_mesh(name, values, info)
        obj = bpy.data.objects.new(name, mesh)
        collection.objects.link(obj)
        obj.scale = (1.0 / unit_scale,) * 3
        obj["ots_node"], obj["ots_port"] = entry["node"], entry["port"]
        obj["ots_build"] = info["manifest_path"]
        mesh.materials.append(material)
        for mask, _, samples in masks:
            attr = mesh.attributes.new("OTS " + label(mask), "FLOAT", "POINT")
            attr.data.foreach_set("value", array("f", samples))
    for entry, points in point_outputs:
        make_points(collection, entry, points, info, unit_scale)
    return collection


class OTS_OT_import_build(bpy.types.Operator, ImportHelper):
    bl_idname = "import_scene.openterrainstudio_build"
    bl_label = "Import OpenTerrainStudio build"
    bl_options = {"REGISTER", "UNDO"}
    filename_ext = ".json"
    filter_glob: StringProperty(default="*.json", options={"HIDDEN"})

    def execute(self, context):
        try:
            result = import_build(context, self.filepath)
        except (BuildError, OSError, RuntimeError) as exc:
            self.report({"ERROR"}, str(exc))
            return {"CANCELLED"}
        self.report({"INFO"}, f"Imported {len(result.objects)} terrain and species objects; masks are in OTS mask textures")
        return {"FINISHED"}


def menu_import(self, context):
    self.layout.operator(OTS_OT_import_build.bl_idname, text="OpenTerrainStudio build (build.json)")


def register():
    bpy.utils.register_class(OTS_OT_import_build)
    bpy.types.TOPBAR_MT_file_import.append(menu_import)


def unregister():
    bpy.types.TOPBAR_MT_file_import.remove(menu_import)
    bpy.utils.unregister_class(OTS_OT_import_build)
