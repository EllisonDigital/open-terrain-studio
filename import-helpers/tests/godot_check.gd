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
		for variant in ["points", "points-million"]:
			var manifest := folder.path_join(variant + "-build.json")
			if not FileAccess.file_exists(manifest):
				continue
			var info: Dictionary = JSON.parse_string(FileAccess.get_file_as_string(manifest))
			var entry: Dictionary = {}
			for candidate in info.files:
				if candidate.data == "PointSet" and candidate.format == "csv":
					entry = candidate
			var started := Time.get_ticks_msec()
			var result := Importer.import_build(manifest)
			check(not result.has("error"), "point import " + str(result.get("error", "")))
			if result.has("error"):
				continue
			var scene: Node3D = result.scene
			var elapsed := float(Time.get_ticks_msec() - started) / 1000.0
			check(variant != "points-million" or elapsed < 30.0, "million point import <30 s")
			var save_path := folder.path_join("godot-" + variant + ".scn")
			check(Importer.save_scene(scene, save_path) == OK, "save points")
			scene.free()
			var packed := ResourceLoader.load(save_path, "PackedScene", ResourceLoader.CACHE_MODE_IGNORE) as PackedScene
			check(packed != null, "reload points")
			if packed == null:
				continue
			scene = packed.instantiate()
			root.add_child(scene)
			var containers := {}
			var terrain: MeshInstance3D
			for child in scene.get_children():
				if child is MultiMeshInstance3D:
					containers[child.get_meta("ots_species")] = child
				elif child is MeshInstance3D and child.get_meta("ots_node", "") == "terrain_a":
					terrain = child
			check(containers.size() == entry.species.size(), "one MultiMesh per species")
			var cursors := {}
			for species in entry.species:
				cursors[species] = 0
			var lines := FileAccess.get_file_as_string(folder.path_join(entry.file)).strip_edges().split("\n")
			for line_index in range(1, lines.size()):
				var values := lines[line_index].strip_edges().split(",")
				var species: String = values[5]
				var mesh: MultiMesh = containers[species].multimesh
				var i: int = cursors[species]
				cursors[species] = i + 1
				var transform := containers[species].transform * mesh.get_instance_transform(i)
				var x := values[0].to_float()
				var y := values[1].to_float()
				var z := values[2].to_float()
				var scale := values[4].to_float()
				check(transform.origin.distance_to(Vector3(x - info.world_size_m[0]/2.0, z, y - info.world_size_m[1]/2.0)) < 0.001, "point world position")
				var facing := transform.basis.x.normalized()
				check(absf(wrapf(atan2(-facing.z, facing.x) - deg_to_rad(values[3].to_float()), -PI, PI)) < 0.0001, "point yaw")
				check(absf(transform.basis.get_scale().x - scale) < 0.00001, "point scale")
				var col := roundi(x / info.cell_size_m[0])
				var row := roundi(y / info.cell_size_m[1])
				var vertices: PackedVector3Array = terrain.mesh.surface_get_arrays(0)[Mesh.ARRAY_VERTEX]
				check(absf(transform.origin.y - vertices[row * int(info.resolution[0]) + col].y) <= (info.height_range_m[1] - info.height_range_m[0]) / 65535.0 / 2.0 + 0.001, "point height aligns with terrain")
			for species in entry.species:
				var expected := 0
				for i in range(1, lines.size()):
					if lines[i].strip_edges().get_slice(",", 5) == species:
						expected += 1
				check(cursors[species] == expected and containers[species].multimesh.instance_count == expected, "every instance/species")
			reports.append({"manifest": variant, "points": entry.count, "seconds": elapsed})
			scene.free()
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
