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
	check(types.size() >= 6, "node types registered (%d)" % types.size())

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

	# Save / load round trip.
	var path := OS.get_user_data_dir().path_join("smoke.otstudio")
	check(project.save(path), "save project")
	check(not project.is_modified(), "saved project not modified")
	var loaded := TerrainProject.new()
	check(loaded.load(path), "load project")
	check(loaded.get_graph().get_nodes().size() == 3, "loaded 3 nodes")

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

	print("FAILED: %d" % failures if failures else "ALL PASSED")
	quit(1 if failures else 0)
