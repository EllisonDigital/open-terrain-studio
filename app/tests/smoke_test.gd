## Headless smoke test for the Rust extension.
## Run from the repo root after `cargo build`:
##   godot --headless --path app --script res://tests/smoke_test.gd
extends SceneTree

var failures := 0

func check(cond: bool, what: String) -> void:
	if cond:
		print("  ok   ", what)
	else:
		failures += 1
		printerr("  FAIL ", what)

func _initialize() -> void:
	run.call_deferred()

func run() -> void:
	print("OpenTerrainStudio smoke test, engine ", TerrainProject.get_app_version())
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()

	var types: Array = graph.get_node_types()
	check(types.size() >= 36, "node types registered (%d)" % types.size())

	var fbm: String = graph.add_node("noise.fbm", Vector2(0, 0))
	var levels: String = graph.add_node("adjust.levels", Vector2(300, 0))
	check(fbm != "" and levels != "", "add nodes")
	check(graph.connect_ports(fbm, "out", levels, "in") == "", "connect fbm -> levels")
	check(graph.connect_ports(levels, "out", fbm, "nope") != "", "bad connection refused")
	check(graph.set_param(fbm, "octaves", 4) == 4, "set int param")
	check(graph.set_param(fbm, "gain", 5.0) == 1.0, "float param clamped")
	check(graph.set_param(fbm, "basis", "simplex") == "simplex", "set enum param")
	check(graph.get_nodes().size() == 2 and graph.get_links().size() == 1, "nodes and links listed")
	check(project.is_modified(), "project marked modified")

	# Undo / redo.
	check(project.get_undo_label() == "Set Basis", "undo label: %s" % project.get_undo_label())
	check(project.undo() == "Set Basis" and graph.get_nodes()[0]["params"]["basis"] == "perlin", "undo restores basis")
	check(project.redo() == "Set Basis" and graph.get_nodes()[0]["params"]["basis"] == "simplex", "redo reapplies it")
	var steps := 0
	while project.can_undo():
		project.undo()
		steps += 1
	check(steps == 6 and graph.get_nodes().is_empty() and not project.is_modified(), "undo back to empty (%d steps)" % steps)
	while project.can_redo():
		project.redo()
	check(graph.get_nodes().size() == 2 and graph.get_links().size() == 1, "redo everything")
	project.begin_edit_group("Tweak")
	graph.set_param(fbm, "octaves", 5)
	graph.set_param(fbm, "lacunarity", 2.5)
	project.end_edit_group()
	check(project.undo() == "Tweak" and graph.get_nodes()[0]["params"]["octaves"] == 4, "grouped edits undo together")
	project.redo()

	# Curve parameters and mask-driven parameter ports.
	var curve: String = graph.add_node("adjust.curve", Vector2(600, 0))
	var stored: Variant = graph.set_param(curve, "curve", PackedVector2Array([Vector2(0, 0), Vector2(0.5, 0.2), Vector2(1, 1)]))
	check(stored is Array and stored.size() == 3, "curve stored: %s" % [stored])
	check(graph.set_param_exposed(fbm, "height_m", true), "expose fbm height")
	var fbm_inputs: Array = graph.get_nodes()[0]["inputs"]
	check(fbm_inputs.size() == 1 and fbm_inputs[0]["key"] == "p:height_m" and fbm_inputs[0]["param"] == "height_m", "parameter port listed")
	check(not graph.set_param_exposed(fbm, "octaves", true), "octaves can't be driven")
	var grad: String = graph.add_node("primitive.gradient", Vector2(-300, 0))
	check(graph.connect_ports(grad, "out", fbm, "p:height_m") == "", "mask drives fbm height")
	graph.remove_node(curve)

	# Background evaluation through the builder node.
	var builder := TerrainBuilder.new()
	root.add_child(builder)
	var progress_seen := [false]
	builder.progress.connect(func(_g, _f): progress_seen[0] = true)
	# A stale request must never be delivered.
	var stale: int = builder.request_preview(project, fbm, "out", 256)
	var gen: int = builder.request_preview(project, levels, "out", 256)
	var preview: TerrainPreview = await builder.preview_ready
	check(preview.get_generation() == gen and gen == stale + 1, "only the latest request delivered")
	check(preview.get_node_id() == levels, "preview is for the requested node")
	check(progress_seen[0], "progress signal emitted")
	var img: Image = preview.get_image()
	check(img != null and img.get_width() == 256 and img.get_format() == Image.FORMAT_RF, "preview image 256x256 RF")
	check(absf(preview.get_min()) < 0.01 and absf(preview.get_max() - 2000.0) < 0.1, "levels fills 0..2000 m")

	# Missing input reports an error instead of crashing.
	var lonely: String = graph.add_node("adjust.levels", Vector2(0, 300))
	builder.request_preview(project, lonely, "out", 64)
	var failed: Array = await builder.preview_failed
	check(String(failed[1]).contains("input"), "missing input reported: %s" % failed[1])

	# The cache: viewing the same node again computes nothing.
	builder.request_preview(project, levels, "out", 256)
	var again: TerrainPreview = await builder.preview_ready
	check(again.get_computed_nodes() == 0, "cached preview computed %d nodes" % again.get_computed_nodes())

	# Masks come with the terrain they were computed from.
	var slope: String = graph.add_node("data.slope", Vector2(600, 0))
	graph.connect_ports(levels, "out", slope, "in")
	builder.request_preview(project, slope, "out", 128)
	var mask: TerrainPreview = await builder.preview_ready
	check(mask.get_port_type() == "mask" and mask.get_base_node_id() == levels, "mask preview has base terrain")
	check(mask.get_base_image() != null and mask.get_base_image().get_width() == 128, "base image")
	check(mask.get_computed_nodes() == 4, "new resolution: 4 nodes computed, base reused: %d" % mask.get_computed_nodes())

	# Save / load round trip.
	var path := OS.get_user_data_dir().path_join("smoke.otstudio")
	check(project.save(path), "save project")
	check(not project.is_modified(), "saved project not modified")
	var loaded := TerrainProject.new()
	check(loaded.load(path), "load project")
	check(loaded.get_graph().get_nodes().size() == 5, "loaded 5 nodes")
	check(not loaded.can_undo(), "fresh history after load")
	check(loaded.get_project_dir() == path.get_base_dir(), "project dir known")

	# Export on a worker thread.
	var exporter := TerrainExporter.new()
	root.add_child(exporter)
	var out_dir := OS.get_user_data_dir().path_join("smoke_export")
	check(exporter.request_export(project, levels, "out", 129, out_dir, PackedStringArray(["exr32", "png16"])), "export requested")
	var result: Array = await exporter.export_finished
	check(result[0], "export succeeded: %s" % result[1])
	check((result[2] as PackedStringArray).size() == 3, "3 files written")
	var png := Image.load_from_file(out_dir.path_join("levels-%s_out_129.png" % levels))
	check(png != null and png.get_width() == 129, "exported PNG loads in Godot")

	# Build every marked output.
	check(project.set_export(levels, "out", "exr32", true), "mark levels")
	check(project.set_export(slope, "out", "png16", true), "mark slope")
	check(not project.set_export(slope, "out", "jpeg", true), "unknown format refused")
	check(project.get_exports().size() == 2, "2 outputs marked")
	check(Array(graph.get_nodes().filter(func(n): return n["id"] == slope)[0]["exported"]) == ["out"], "node lists its marks")
	var build_dir := OS.get_user_data_dir().path_join("smoke_build")
	check(exporter.request_build(project, 65, build_dir), "build requested")
	var built: Array = await exporter.export_finished
	check(built[0] and (built[2] as PackedStringArray).size() == 3, "build wrote 2 images + build.json: %s" % built[1])
	graph.remove_node(slope)
	check(project.get_exports().size() == 1, "deleting a node drops its marks")
	project.undo()
	check(project.get_exports().size() == 2, "undo brings the mark back")

	print("FAILED: %d" % failures if failures else "ALL PASSED")
	quit(1 if failures else 0)
