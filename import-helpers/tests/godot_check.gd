# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
extends SceneTree
const Importer = preload("res://addons/openterrainstudio_import/importer.gd")
var failures := 0

func check(condition: bool, message: String) -> void:
	if not condition:
		failures += 1
		printerr("FAIL: ", message)

func _initialize() -> void:
	run.call_deferred()

func run() -> void:
	var folder := OS.get_cmdline_user_args()[0]
	var single_only := OS.get_cmdline_user_args().size() > 1
	var reports := []
	for name in (["single"] if single_only else ["build", "png-only", "single", "masks-only"]):
		var result := Importer.import_build(folder.path_join(name + ".json"))
		if result.has("error"):
			printerr(result.error)
			quit(1)
			return
		var scene: Node3D = result.scene
		var save_path := folder.path_join("godot-" + name + ".scn")
		check(Importer.save_scene(scene, save_path) == OK, "save embedded scene")
		scene.free()
		var packed := ResourceLoader.load(save_path, "PackedScene", ResourceLoader.CACHE_MODE_IGNORE) as PackedScene
		if packed == null:
			quit(1)
			return
		scene = packed.instantiate()
		root.add_child(scene)
		var info: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(folder.path_join(name + ".json")))
		var w := int(info.resolution[0])
		var h := int(info.resolution[1])
		var expected_terrains := 0 if name == "masks-only" else (1 if name == "single" else 2)
		check(scene.get_child_count() == expected_terrains, "deduplicate height encodings")
		check(scene.masks.size() == (0 if name == "single" else 2), "all masks exposed")
		var worst_height := 0.0
		var worst_xy := 0.0
		var worst_mask := 0.0
		for terrain: MeshInstance3D in scene.get_children():
			var node: String = terrain.get_meta("ots_node")
			var port: String = terrain.get_meta("ots_port")
			var reference := FileAccess.get_file_as_bytes(folder.path_join(node + "_" + port + ".f32")).to_float32_array()
			var arrays: Array = terrain.mesh.surface_get_arrays(0)
			var vertices: PackedVector3Array = arrays[Mesh.ARRAY_VERTEX]
			var normals: PackedVector3Array = arrays[Mesh.ARRAY_NORMAL]
			check(vertices.size() == w * h, "one vertex per pixel")
			var texture: Texture2D = scene.height_textures[JSON.stringify([node, port])]
			var image := texture.get_image()
			check(image.get_format() == Image.FORMAT_RF, "full-float image survived scene save/reload")
			for i in vertices.size():
				var x := i % w
				var y := i / w
				var expected_x: float = x * info.cell_size_m[0] - info.world_size_m[0] / 2.0
				var expected_z: float = y * info.cell_size_m[1] - info.world_size_m[1] / 2.0
				worst_xy = maxf(worst_xy, maxf(absf(vertices[i].x - expected_x), absf(vertices[i].z - expected_z)))
				worst_height = maxf(worst_height, absf(vertices[i].y - reference[i]))
				check(vertices[i].y == image.get_pixel(x,y).r, "vertex matches its exact texture pixel")
				check(normals[i].y > 0, "normal faces upward")
		for key in scene.masks:
			var parts: Array = JSON.parse_string(key)
			var reference := FileAccess.get_file_as_bytes(folder.path_join(parts[0] + "_" + parts[1] + ".f32")).to_float32_array()
			var image: Image = scene.masks[key].get_image()
			check(image.get_format() == Image.FORMAT_RF, "mask remains RF")
			for y in h:
				for x in w:
					worst_mask = maxf(worst_mask, absf(image.get_pixel(x,y).r - reference[y*w+x]))
			scene.preview_mask = key
			for terrain: MeshInstance3D in scene.get_children():
				check(terrain.material_override.get_shader_parameter("show_mask"), "mask preview selection")
		var tolerance := 2400.0 / 65535.0 / 2.0 + 0.001 if name == "png-only" else 0.001
		check(worst_height <= tolerance, "%s height error %f" % [name, worst_height])
		check(worst_xy < 0.001, "world extent/orientation")
		check(worst_mask <= (0.5 / 65535 + 0.0000001 if name == "png-only" else 0.0000001), "mask precision/orientation")
		reports.append({"manifest": name, "vertices_per_terrain": w*h, "terrains": expected_terrains, "max_height_error_m": worst_height, "max_xy_error_m": worst_xy, "max_mask_error": worst_mask})
		scene.free()
	if not single_only:
		var decoded: Dictionary = Importer.Png16.read_image(folder.path_join("filters.png"), Vector2i(9,11))
		check(not decoded.has("error"), "decode all five PNG row filters")
		if decoded.has("image"):
			for i in 99:
				check(absf(decoded.image.get_pixel(i % 9, i / 9).r - float((i * 12347 + 37) % 65536) / 65535.0) < 0.00000004, "all sixteen PNG bits preserved")
		check(Importer.Png16.read_image(folder.path_join("corrupt.png"), Vector2i(9,11)).has("error"), "reject corrupt PNG")
		for name in ["bad-generator", "missing", "bad-orientation", "bad-resolution"]:
			var result := Importer.import_build(folder.path_join(name + ".json"))
			check(result.has("error"), "reject " + name)
	print("GODOT_IMPORT_REPORT ", JSON.stringify({"godot": Engine.get_version_info().string, "results": reports, "failures": failures}))
	quit(1 if failures else 0)
