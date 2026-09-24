# blender --background --factory-startup --python blender_check.py -- <export_dir>
import bpy, json, os, sys, math

out = sys.argv[sys.argv.index("--") + 1]
info = json.load(open(os.path.join(out, "build.json")))
exr = next(f["file"] for f in info["files"] if f["format"] == "exr32")
n = info["resolution"][0]
size = info["world_size_m"][0]

img = bpy.data.images.load(os.path.join(out, exr))
img.colorspace_settings.name = "Non-Color"
w, h = img.size
px = list(img.pixels)  # RGBA float, Blender row 0 = bottom of the image

def pixel(col, row_from_top):
    r = h - 1 - row_from_top
    return px[(r * w + col) * 4]

bpy.ops.mesh.primitive_grid_add(x_subdivisions=n - 1, y_subdivisions=n - 1, size=size)
obj = bpy.context.active_object
tex = bpy.data.textures.new("height", "IMAGE")
tex.image = img
tex.extension = "EXTEND"
tex.use_interpolation = False
mod = obj.modifiers.new("Displace", "DISPLACE")
mod.texture = tex
mod.texture_coords = "UV"
mod.strength = 1.0
mod.mid_level = 0.0

dg = bpy.context.evaluated_depsgraph_get()
me = obj.evaluated_get(dg).to_mesh()
verts = me.vertices
xs = [v.co.x for v in verts]
ys = [v.co.y for v in verts]
zs = [v.co.z for v in verts]
report = {
    "blender": bpy.app.version_string,
    "image_size": [w, h],
    "vertices": len(verts),
    "grid_verts_per_side": n,
    "x_range_m": [min(xs), max(xs)],
    "y_range_m": [min(ys), max(ys)],
    "z_range_m": [min(zs), max(zs)],
    "exr_min_max_m": [min(px[0::4]), max(px[0::4])],
}

# Compare each vertex with the pixel for world (col, row): Blender x grows with
# col; row 0 (world Y = 0) is the top of the image, which UV maps to +Y.
cell = size / (n - 1)
errs = []
for v, z in zip(verts, zs):
    col = round((v.co.x + size / 2) / cell)
    row = round((size / 2 - v.co.y) / cell)
    errs.append(abs(z - pixel(col, row)))
errs.sort()
report["abs_error_m"] = {
    "median": errs[len(errs) // 2],
    "p99": errs[int(len(errs) * 0.99)],
    "max": errs[-1],
}
# Same comparison with Y not flipped, to prove the orientation.
errs_flip = []
for v, z in list(zip(verts, zs))[::97]:
    col = round((v.co.x + size / 2) / cell)
    row = round((v.co.y + size / 2) / cell)
    errs_flip.append(abs(z - pixel(col, row)))
report["median_error_if_y_not_flipped_m"] = sorted(errs_flip)[len(errs_flip) // 2]
print("REPORT " + json.dumps(report))
