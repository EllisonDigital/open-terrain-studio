## v0.7 vegetation test: the Forest valley example's populations through the
## extension, the 3D view's plants, the data view, species presets, and
## points exported to CSV landing where the preview put them.
## godot --headless --path app --script res://tests/vegetation_test.gd
extends SceneTree

const Inspector := preload("res://scripts/inspector.gd")
const TerrainView := preload("res://scripts/terrain_view.gd")

var failures := 0

func check(condition: bool, message: String) -> void:
	if condition:
		print("  ok   ", message)
	else:
		failures += 1
		printerr("  FAIL ", message)

func _initialize() -> void:
	run.call_deferred()

func run() -> void:
	create_timer(120.0).timeout.connect(func():
		printerr("Vegetation test timed out")
		quit(1))
	var project := TerrainProject.new()
	check(project.load_json(FileAccess.get_file_as_string("res://examples/forest_valley.otstudio")),
			"Forest valley example loads")
	var graph: TerrainGraph = project.get_graph()
	var pine := "n_0006"
	var birch := "n_0007"
	var shrubs := "n_0008"
	var rocks := "n_0009"
	# v0.7.5: populations live in the Vegetation tab, terrain arrives by portal.
	var tabs := {}
	for n in graph.get_nodes():
		tabs[n["id"]] = n["tab"]
	check([pine, birch, shrubs, rocks].all(func(id): return tabs[id] == "vegetation") and tabs["n_0003"] == "terrain",
			"populations in the Vegetation tab, terrain in Terrain")
	var sent := graph.send_to_tab("n_0003", "height", Vector2(0, -900), "vegetation")
	check(sent != "" and project.undo() != "", "heights can be sent to the Vegetation tab (undone)")
	check(graph.send_to_tab(pine, "points", Vector2.ZERO, "colour") == "", "points can't go through a portal")
	check(graph.send_to_tab("n_0003", "height", Vector2.ZERO, "nowhere") == "", "unknown tab refused")
	var builder := TerrainBuilder.new()
	root.add_child(builder)
	builder.preview_failed.connect(func(_generation, error):
		printerr("Unexpected build failure: ", error)
		quit(1))

	# Viewing the last population draws the whole chain upstream of it.
	builder.request_preview(project, shrubs, "density", 257)
	var preview: TerrainPreview = await builder.preview_ready
	check(preview.get_port_type() == "mask", "shrub density is a mask")
	check(preview.get_base_node_id() == "n_000b", "draped over the eroded terrain (via its portal)")
	var layers: Array = preview.get_vegetation(0)
	var nodes := layers.map(func(l): return l["node"])
	check(nodes.has(pine) and nodes.has(birch) and nodes.has(shrubs), "pine, birch and shrubs drawn: %s" % [nodes])
	var kinds := {}
	for l in layers:
		kinds[l["node"]] = l["kind"]
		check(l["count"] > 0 and (l["points"] as PackedFloat32Array).size() == l["count"] * 5,
				"%s has %d points" % [l["node"], l["count"]])
	check(kinds.get(pine) == "tree" and kinds.get(shrubs) == "shrub", "placeholder kinds: %s" % [kinds])
	var total := preview.get_vegetation_count()
	var capped: Array = preview.get_vegetation(1000)
	var shown := 0
	for l in capped:
		shown += (l["points"] as PackedFloat32Array).size() / 5
	check(shown <= 1000 and shown > 0, "density cap: %d of %d shown" % [shown, total])
	var pine_capped: Array = capped.filter(func(l): return l["node"] == pine)
	check((pine_capped[0]["points"] as PackedFloat32Array).size() / 5 >= 300, "trees get their share first")

	# The 3D view builds one MultiMesh per population, within its cap.
	var view := TerrainView.new()
	root.add_child(view)
	view.set_world(project.get_world_size(), project.get_height_min(), project.get_height_max())
	view.show_preview(preview)
	var drawn: int = view.get_plant_instance_count()
	check(drawn > 0 and drawn <= mini(total, TerrainView.MAX_VEGETATION), "3D view draws %d plants" % drawn)
	view.set_show_plants(false)
	check(view.get_plant_instance_count() == 0, "Plants toggle hides them")
	view.set_show_plants(true)
	check(not view.is_overlay_shown(), "v0.7.6: a vegetation node shows its plants, not its weight map")
	view.set_show_weight_map(true)
	check(view.is_overlay_shown(), "Weight map toggle paints the density under the plants")
	view.set_show_weight_map(false)

	# Points output: shown as a mask of points, with the count.
	builder.request_preview(project, pine, "points", 257)
	var pts: TerrainPreview = await builder.preview_ready
	check(pts.get_port_type() == "point_set" and pts.get_point_count() > 100, "pine points: %d" % pts.get_point_count())
	check(pts.get_max() == 1.0 and pts.get_image().get_width() == 257, "points drawn as a mask")
	view.show_preview(pts)
	check(not view.is_overlay_shown(), "points output: plants only until Weight map is on")

	# Ecosystem data view.
	builder.set_data_view(true)
	builder.request_preview(project, birch, "density", 257)
	var data: TerrainPreview = await builder.preview_ready
	check(data.is_data_view() and data.get_port_type() == "color_map", "data view is a colour map")
	builder.set_data_view(false)
	builder.request_preview(project, birch, "density", 257)
	check(not (await builder.preview_ready).is_data_view(), "data view off again")

	# Species presets: bundled, applied as one undo step.
	var presets: Array = Inspector.species_presets()
	check(presets.size() >= 15, "%d bundled species presets" % presets.size())
	var spruce: Array = presets.filter(func(p): return p["name"] == "Norway spruce")
	check(spruce.size() == 1, "Norway spruce preset listed")
	if spruce.size() == 1:
		var result: Dictionary = graph.apply_species_preset(pine, FileAccess.get_file_as_string(spruce[0]["path"]))
		check(result["ok"] and (result["warnings"] as PackedStringArray).is_empty(), "spruce applied cleanly")
		check(_param(graph, pine, "species") == "norway_spruce", "species set by the preset")
		check(project.undo() != "" and _param(graph, pine, "species") == "scots_pine", "undo restores the pine")
	var bad: Dictionary = graph.apply_species_preset(pine, "name: broken\nlist:\n  - a\n")
	check(not bad["ok"], "an invalid preset is refused: %s" % bad.get("error", ""))

	# Export: points to CSV at the preview resolution land exactly where the
	# preview placed them.
	var folder := ProjectSettings.globalize_path("user://vegetation_test_build")
	DirAccess.make_dir_recursive_absolute(folder)
	var exporter := TerrainExporter.new()
	root.add_child(exporter)
	check(exporter.request_build(project, 257, folder), "build started")
	var finished: Array = await exporter.export_finished
	check(finished[0], "build finished: %s" % finished[1])
	var files: PackedStringArray = finished[2]
	var csv_path := ""
	for f in files:
		if f.get_file().begins_with("trees-n_0006_points") and f.ends_with(".csv"):
			csv_path = f
	check(csv_path != "", "pine points CSV written")
	var rows := _read_csv(csv_path)
	var pine_layer: Array = layers.filter(func(l): return l["node"] == pine)
	var preview_pts: PackedFloat32Array = pine_layer[0]["points"] if pine_layer.size() == 1 else PackedFloat32Array()
	check(rows.size() == preview_pts.size() / 5 and rows.size() > 0, "CSV has every point (%d)" % rows.size())
	var worst := 0.0
	for i in mini(rows.size(), preview_pts.size() / 5):
		var a := _placement(rows[i], project.get_world_size())
		var b := _placement(Array(preview_pts.slice(i * 5, i * 5 + 5)), project.get_world_size())
		worst = maxf(worst, a.origin.distance_to(b.origin))
	check(worst < 0.01, "exported placement matches the preview (worst %.4f m)" % worst)
	var info: Variant = JSON.parse_string(FileAccess.get_file_as_string(folder.path_join("build.json")))
	var entries: Array = (info["files"] as Array).filter(func(e): return e["format"] == "csv")
	check(entries.size() == 4 and entries.all(func(e): return e["data"] == "point_set"), "build.json lists 4 point files")

	print("VEGETATION FAILED: %d" % failures if failures else "VEGETATION ALL PASSED")
	quit(1 if failures else 0)


func _param(graph: TerrainGraph, id: String, key: String) -> Variant:
	for n in graph.get_nodes():
		if n["id"] == id:
			return n["params"][key]
	return null


## Rows of a points CSV as [x, y, z, rotation_deg, scale] floats.
func _read_csv(path: String) -> Array:
	var rows := []
	var f := FileAccess.open(path, FileAccess.READ)
	if f == null:
		return rows
	var header := f.get_csv_line()
	if header[0] != "x" or header[5] != "species":
		return rows
	while not f.eof_reached():
		var line := f.get_csv_line()
		if line.size() < 6:
			continue
		rows.append([float(line[0]), float(line[1]), float(line[2]), float(line[3]), float(line[4])])
	return rows


## The simple Godot import: a point's transform with the terrain centred on
## the origin (x → X, y → Z, z → Y), turned about Y, scaled.
func _placement(p: Array, world_size: float) -> Transform3D:
	var basis := Basis(Vector3.UP, -deg_to_rad(p[3])).scaled(Vector3.ONE * p[4])
	return Transform3D(basis, Vector3(p[0] - world_size * 0.5, p[2], p[1] - world_size * 0.5))
