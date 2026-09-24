## Checks how Godot reads an OpenTerrainStudio export (docs/export-guides.md).
##   godot --headless --path app --script res://../tests/import/godot_check.gd -- <export_dir>
extends SceneTree

func _initialize() -> void:
	var dir: String = OS.get_cmdline_user_args()[0]
	var info: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(dir.path_join("build.json")))
	var lo: float = info["height_range_m"][0]
	var span: float = info["height_range_m"][1] - lo
	var exr: Image
	var png: Image
	for f in info["files"]:
		var img := Image.load_from_file(dir.path_join(f["file"]))
		if f["format"] == "exr32":
			exr = img
		else:
			png = img
	print("EXR: %dx%d format=%d (FORMAT_RF=%d, FORMAT_RGBF=%d, FORMAT_RGBAF=%d)" % [exr.get_width(), exr.get_height(), exr.get_format(), Image.FORMAT_RF, Image.FORMAT_RGBF, Image.FORMAT_RGBAF])
	print("PNG: %dx%d format=%d (FORMAT_L8=%d)" % [png.get_width(), png.get_height(), png.get_format(), Image.FORMAT_L8])
	# Compare every pixel: EXR is metres; PNG is 0..1 of the height range.
	var exr_max_err := 0.0
	var png_max_err := 0.0
	var mn := INF
	var mx := -INF
	for y in exr.get_height():
		for x in exr.get_width():
			var h := exr.get_pixel(x, y).r
			mn = minf(mn, h)
			mx = maxf(mx, h)
			png_max_err = maxf(png_max_err, absf(lo + png.get_pixel(x, y).r * span - h))
	print("EXR heights: %.3f .. %.3f m" % [mn, mx])
	print("PNG vs EXR max error: %.3f m (one 8-bit step = %.3f m, one 16-bit step = %.4f m)" % [png_max_err, span / 255.0, span / 65535.0])
	# A HeightMapShape3D built straight from the EXR (collision).
	# The EXR loads as an RGB float image; HeightMapShape3D wants one channel.
	var single := exr.duplicate() as Image
	single.convert(Image.FORMAT_RF)
	# One sample per unit, so store heights divided by the cell size and scale
	# the CollisionShape3D uniformly by the cell size (non-uniform scaling of
	# collision shapes isn't reliable).
	var cell: float = info["cell_size_m"][0]
	var shape := HeightMapShape3D.new()
	shape.update_map_data_from_image(single, 0.0, 1.0 / cell)
	var col := CollisionShape3D.new()
	col.shape = shape
	col.scale = Vector3.ONE * cell
	print("HeightMapShape3D: %dx%d samples, spans %.1f m, heights %.3f .. %.3f m after scaling" % [
		shape.map_width, shape.map_depth, (shape.map_width - 1) * cell,
		shape.get_min_height() * cell, shape.get_max_height() * cell])
	col.free()
	# map_data is row-major like the image: index x + y * width.
	var ordered := true
	for p: Vector2i in [Vector2i(0, 0), Vector2i(shape.map_width - 1, 0), Vector2i(0, shape.map_depth - 1), Vector2i(123, 456)]:
		var i: int = p.x + p.y * shape.map_width
		ordered = ordered and absf(shape.map_data[i] * cell - single.get_pixel(p.x, p.y).r) < 0.01
	print("map_data row-major like the image: %s" % ordered)
	quit()
