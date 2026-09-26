# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
extends RefCounted
const BuildRoot = preload("terrain_build.gd")
const Png16 = preload("png16.gd")
const TerrainShader = preload("terrain.gdshader")
const ORIENTATION := "row 0 = world Y 0, column 0 = world X 0"

static func _number(value: Variant) -> bool:
	return (value is int or value is float) and is_finite(float(value))

static func validate(info: Variant, directory: String) -> String:
	if not info is Dictionary or info.get("generator") != "OpenTerrainStudio":
		return "Not an OpenTerrainStudio build: generator must be 'OpenTerrainStudio'"
	for key in ["world_size_m", "height_range_m", "resolution", "cell_size_m"]:
		var pair: Variant = info.get(key)
		if not pair is Array or pair.size() != 2:
			return key + " must contain two numbers"
		for value in pair:
			if not _number(value):
				return key + " must contain finite numbers"
			if key != "height_range_m" and value <= 0:
				return key + " must be positive"
			if key == "resolution" and (value != floor(value) or value < 2 or value > 16384):
				return "resolution must contain integers from 2 to 16384"
	if info.height_range_m[1] <= info.height_range_m[0]:
		return "height_range_m max must exceed min"
	if info.get("orientation") != ORIENTATION:
		return "Unsupported or missing orientation; expected " + ORIENTATION
	for axis in 2:
		var actual := float(info.cell_size_m[axis]) * (float(info.resolution[axis]) - 1.0)
		if absf(actual - info.world_size_m[axis]) > maxf(0.000001, info.world_size_m[axis] * 0.000001):
			return "cell_size_m, world_size_m and resolution disagree"
	var files: Variant = info.get("files")
	if not files is Array or files.is_empty():
		return "build.json has no files to import"
	var seen := {}
	var types := {}
	for entry in files:
		if not entry is Dictionary:
			return "Each files entry must be an object"
		for key in ["file", "node", "port"]:
			if not entry.get(key) is String or entry[key].is_empty():
				return "Each files entry needs a nonempty " + key
		if not entry.get("data") is String or not entry.get("format") is String:
			return "Each files entry needs string data and format"
		var data: String = entry.get("data", "")
		var format: String = entry.get("format", "")
		if (data == "PointSet" and format not in ["csv", "json"]) or (data not in ["heightfield", "mask", "PointSet"]) or (data != "PointSet" and format not in ["exr32", "png16"]):
			return "Unsupported data/format for " + entry.file
		var expected := "metres" if data == "PointSet" else (("metres" if data == "heightfield" else "0..1") if format == "exr32" else ("0..65535 = height_range_m min..max" if data == "heightfield" else "0..65535 = 0..1"))
		if entry.get("encoding") != expected:
			return "Unsupported encoding for " + entry.file
		if data == "PointSet":
			if not entry.get("count") is int or entry.count < 0 or not entry.get("species") is Array:
				return "Invalid PointSet count/species"
			var unique := {}
			for species in entry.species:
				if not species is String or species.is_empty() or "," in species or unique.has(species):
					return "Invalid PointSet species"
				unique[species] = true
		var key := JSON.stringify([entry.node, entry.port])
		if types.has(key) and types[key] != data:
			return "Conflicting data types for " + key
		types[key] = data
		var identity := JSON.stringify([entry.node, entry.port, format])
		if seen.has(identity):
			return "Duplicate output format: " + identity
		seen[identity] = true
		var name: String = entry.file
		if name.is_absolute_path() or "\\" in name or ".." in name.split("/"):
			return "File must be inside the build folder: " + name
		if not FileAccess.file_exists(directory.path_join(name)):
			return "Missing build file: " + directory.path_join(name)
	return ""

static func _points(entry: Dictionary, info: Dictionary, directory: String) -> Dictionary:
	var text := FileAccess.get_file_as_string(directory.path_join(entry.file))
	var rows: Array = []
	var species: Array = entry.species
	if entry.format == "json":
		var parsed := JSON.new()
		if parsed.parse(text) != OK or not parsed.data is Dictionary:
			return {"error": "Invalid PointSet JSON"}
		var data: Dictionary = parsed.data
		if data.get("format") != "ots-points" or data.get("version") != 1 or data.get("species") != species or not data.get("points") is Array:
			return {"error": "Unsupported PointSet version/species/points"}
		rows = data.points
	else:
		var lines := text.strip_edges().split("\n")
		if lines.is_empty() or lines[0].strip_edges() != "x,y,z,rotation_deg,scale,species":
			return {"error": "Invalid PointSet CSV header"}
		for i in range(1, lines.size()):
			var parts := lines[i].strip_edges().split(",")
			if parts.size() != 6 or not species.has(parts[5]):
				return {"error": "Invalid PointSet CSV species/row"}
			var row := []
			for j in 5:
				if not parts[j].is_valid_float():
					return {"error": "Invalid PointSet CSV number"}
				row.append(parts[j].to_float())
			row.append(species.find(parts[5]))
			rows.append(row)
	if rows.size() != entry.count:
		return {"error": "PointSet count mismatch"}
	var buckets: Array = []
	for unused in species:
		buckets.append([])
	for row in rows:
		if not row is Array or row.size() != 6:
			return {"error": "Invalid PointSet row"}
		for j in 5:
			if not _number(row[j]):
				return {"error": "Non-finite PointSet number"}
		if not row[5] is int or row[5] < 0 or row[5] >= species.size():
			return {"error": "Unknown PointSet species index"}
		if row[0] < -0.000001 or row[0] > info.world_size_m[0] + 0.000001 or row[1] < -0.000001 or row[1] > info.world_size_m[1] + 0.000001 or row[4] <= 0:
			return {"error": "PointSet outside world bounds or invalid scale"}
		buckets[row[5]].append(row)
	return {"buckets": buckets}

static func _instances(entry: Dictionary, buckets: Array, info: Dictionary, scene: Node3D) -> void:
	for species_index in entry.species.size():
		var rows: Array = buckets[species_index]
		var child := MultiMeshInstance3D.new()
		child.name = (entry.node + "_" + entry.port + "_" + entry.species[species_index]).validate_node_name()
		child.set_meta("ots_species", entry.species[species_index])
		child.set_meta("ots_node", entry.node)
		child.set_meta("ots_port", entry.port)
		var multi := MultiMesh.new()
		multi.transform_format = MultiMesh.TRANSFORM_3D
		var placeholder := CylinderMesh.new()
		placeholder.top_radius = 0.0
		placeholder.bottom_radius = 0.3
		placeholder.height = 1.0
		multi.mesh = placeholder
		multi.instance_count = rows.size()
		var sx := float(info.world_size_m[0]) / 2.0
		var sy := float(info.world_size_m[1]) / 2.0
		for i in rows.size():
			var row: Array = rows[i]
			var basis := Basis(Vector3.UP, deg_to_rad(float(row[3]))).scaled(Vector3.ONE * float(row[4]))
			multi.set_instance_transform(i, Transform3D(basis, Vector3(float(row[0]) - sx, float(row[2]), float(row[1]) - sy)))
		child.multimesh = multi
		scene.add_child(child, true)
		child.owner = scene

static func _image(entry: Dictionary, info: Dictionary, directory: String) -> Dictionary:
	var image: Image
	var path := directory.path_join(entry.file)
	var size := Vector2i(info.resolution[0], info.resolution[1])
	if entry.format == "png16":
		var decoded := Png16.read_image(path, size)
		if decoded.has("error"):
			return decoded
		image = decoded.image
	else:
		image = Image.load_from_file(path)
		if image == null or image.is_empty():
			return {"error": "Cannot read EXR: " + path}
		if image.get_format() not in [Image.FORMAT_RF, Image.FORMAT_RGBF, Image.FORMAT_RGBAF]:
			return {"error": "Expected full-float EXR: " + path}
		image.convert(Image.FORMAT_RF)
	if image.get_size() != size:
		return {"error": "Image dimensions do not match build resolution: " + entry.file}
	var values := image.get_data().to_float32_array()
	for i in values.size():
		var value := float(values[i])
		if entry.format == "png16" and entry.data == "heightfield":
			value = info.height_range_m[0] + value * (info.height_range_m[1] - info.height_range_m[0])
		if not is_finite(value) or (entry.data == "mask" and (value < 0.0 or value > 1.0)):
			return {"error": "Nonfinite height or mask outside 0..1: " + entry.file}
		values[i] = value
	image = Image.create_from_data(size.x, size.y, false, Image.FORMAT_RF, values.to_byte_array())
	return {"image": image, "values": values}

static func _mesh(heights: PackedFloat32Array, info: Dictionary) -> ArrayMesh:
	var width := int(info.resolution[0])
	var height := int(info.resolution[1])
	var dx := float(info.cell_size_m[0])
	var dz := float(info.cell_size_m[1])
	var vertices := PackedVector3Array()
	var normals := PackedVector3Array()
	var uvs := PackedVector2Array()
	vertices.resize(width * height)
	normals.resize(width * height)
	uvs.resize(width * height)
	for y in height:
		for x in width:
			var i := y * width + x
			vertices[i] = Vector3(x * dx - info.world_size_m[0] / 2.0, heights[i], y * dz - info.world_size_m[1] / 2.0)
			uvs[i] = Vector2((x + 0.5) / width, (y + 0.5) / height)
			var left := maxi(x - 1, 0)
			var right := mini(x + 1, width - 1)
			var top := maxi(y - 1, 0)
			var bottom := mini(y + 1, height - 1)
			var hx := (heights[y * width + right] - heights[y * width + left]) / ((right - left) * dx)
			var hz := (heights[bottom * width + x] - heights[top * width + x]) / ((bottom - top) * dz)
			normals[i] = Vector3(-hx, 1.0, -hz).normalized()
	var indices := PackedInt32Array()
	indices.resize((width - 1) * (height - 1) * 6)
	var cursor := 0
	for y in height - 1:
		for x in width - 1:
			var a := y * width + x
			for index in [a, a + 1, a + width, a + 1, a + width + 1, a + width]:
				indices[cursor] = index
				cursor += 1
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = vertices
	arrays[Mesh.ARRAY_NORMAL] = normals
	arrays[Mesh.ARRAY_TEX_UV] = uvs
	arrays[Mesh.ARRAY_INDEX] = indices
	var mesh := ArrayMesh.new()
	# Disable lossy vertex compression; exported floats are the reference.
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays, [], {}, 0)
	return mesh

static func import_build(filename: String) -> Dictionary:
	var file := FileAccess.open(filename, FileAccess.READ)
	if file == null:
		return {"error": "Cannot read build.json: " + filename}
	var json := JSON.new()
	if json.parse(file.get_as_text()) != OK:
		return {"error": "Invalid build.json: " + json.get_error_message()}
	var directory := filename.get_base_dir()
	var error := validate(json.data, directory)
	if not error.is_empty():
		return {"error": error}
	var info: Dictionary = json.data
	if int(info.resolution[0]) * int(info.resolution[1]) > 4194304:
		return {"error": "This importer is limited to 4,194,304 vertices per terrain; export a smaller build"}
	var selected := {}
	for entry in info.files:
		var key := JSON.stringify([entry.node, entry.port])
		if not selected.has(key) or entry.format == "exr32" or (entry.format == "csv" and selected[key].format == "json"):
			selected[key] = entry
	var loaded := {}
	for key in selected:
		var result := _points(selected[key], info, directory) if selected[key].data == "PointSet" else _image(selected[key], info, directory)
		if result.has("error"):
			return result
		loaded[key] = result
	var scene := Node3D.new()
	scene.set_script(BuildRoot)
	scene.name = "OpenTerrainStudioBuild"
	scene.source_build = filename
	scene.world_size_m = Vector2(info.world_size_m[0], info.world_size_m[1])
	scene.resolution = Vector2i(info.resolution[0], info.resolution[1])
	for key in selected:
		var entry: Dictionary = selected[key]
		if entry.data == "PointSet":
			_instances(entry, loaded[key].buckets, info, scene)
			continue
		var texture := ImageTexture.create_from_image(loaded[key].image)
		texture.resource_name = key
		if entry.data == "mask":
			scene.masks[key] = texture
			continue
		scene.height_textures[key] = texture
		var child := MeshInstance3D.new()
		child.name = (entry.node + "_" + entry.port).validate_node_name()
		child.mesh = _mesh(loaded[key].values, info)
		var material := ShaderMaterial.new()
		material.shader = TerrainShader
		child.material_override = material
		child.set_meta("ots_node", entry.node)
		child.set_meta("ots_port", entry.port)
		scene.add_child(child, true)
		child.owner = scene
	return {"scene": scene}

static func save_scene(scene: Node3D, path: String) -> Error:
	var packed := PackedScene.new()
	var error := packed.pack(scene)
	if error != OK:
		return error
	return ResourceSaver.save(packed, path)
