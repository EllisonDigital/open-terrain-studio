## Drives the real main window: waits for the first preview, edits a
## parameter, adds a node, saves, reloads and exports, taking screenshots.
## Needs a display (or xvfb) and a GPU (or lavapipe):
##   xvfb-run godot --path app --script res://tests/ui_test.gd -- <screenshot_dir>
extends SceneTree

var failures := 0
var shots := ""

func check(cond: bool, what: String) -> void:
	print(("  ok   " if cond else "  FAIL ") + what)
	if not cond:
		failures += 1

func frames(n: int) -> void:
	for i in n:
		await process_frame

func wait_preview(main) -> bool:
	for i in 600:
		await process_frame
		if not main.builder.is_busy():
			await frames(5)
			return true
	return false

func shot(name: String) -> void:
	if shots == "":
		return
	var t := Time.get_ticks_msec()
	await frames(3)
	print("  (3 frames took %d ms)" % (Time.get_ticks_msec() - t))
	root.get_texture().get_image().save_png(shots.path_join(name + ".png"))

func _initialize() -> void:
	var args := OS.get_cmdline_user_args()
	shots = args[0] if args.size() > 0 else ""
	run.call_deferred()

func run() -> void:
	root.size = Vector2i(1600, 900)
	var main = load("res://main.tscn").instantiate()
	root.add_child(main)
	check(await wait_preview(main), "starter preview finished")
	check(main.viewed_id != "", "a node is viewed")
	check(main._stats_label.text.contains("2000"), "stats show 0-2000 m: " + main._stats_label.text)
	await shot("01_starter")

	# Edit fBm parameters through the inspector signal path.
	var fbm := ""
	for n in main.graph.get_nodes():
		if n["type"] == "noise.fbm":
			fbm = n["id"]
	main.graph_panel.select_node(fbm)
	main._view_node(fbm)
	await wait_preview(main)
	main.inspector.param_changed.emit(fbm, "feature_size_m", 1800.0)
	main.inspector.param_changed.emit(fbm, "octaves", 8)
	main.inspector.param_changed.emit(fbm, "basis", "simplex")
	check(await wait_preview(main), "preview after param edits")
	check(main.get_window().title.contains("*"), "title shows unsaved changes")

	# Add a Combine node and wire a second noise in.
	var perlin: String = main.graph_panel.add_node_at("noise.perlin", Vector2(40, 260))
	var combine: String = main.graph_panel.add_node_at("combine.combine", Vector2(340, 220))
	check(main.graph.connect_ports(fbm, "out", combine, "a") == "", "wire fbm -> combine.a")
	check(main.graph.connect_ports(perlin, "out", combine, "b") == "", "wire perlin -> combine.b")
	main.graph.set_param(perlin, "height_m", 300.0)
	main.graph.set_param(perlin, "feature_size_m", 400.0)
	main.graph_panel.rebuild()
	main._view_node(combine)
	check(await wait_preview(main), "combine preview")
	check(main.graph_panel.get_connection_list().size() == 3, "graph shows 3 links")
	await shot("02_combine")

	# World settings panel.
	main._on_selection_cleared()
	await frames(3)
	await shot("03_world")

	# Save, new, reopen.
	var path := OS.get_user_data_dir().path_join("ui_test.otstudio")
	check(main._save_project(path), "save")
	check(not main.get_window().title.contains("*"), "title clean after save")
	main._new_project_with_starter_graph()
	await wait_preview(main)
	main._open_project(path)
	check(await wait_preview(main), "reopened project previews")
	check(main.viewed_id == combine, "viewed node restored")
	check(main.graph.get_nodes().size() == 4, "4 nodes after reload")

	# The export dialog must fit on screen (an autowrapped label once made it 2,000+ px tall).
	main._on_menu(main.Menu.EXPORT)
	await frames(3)
	check(main._export_dialog.size.y < 600, "export dialog fits: %s" % main._export_dialog.size)
	main._export_dialog.hide()

	# Export through the dialog path.
	var out := OS.get_user_data_dir().path_join("ui_export")
	main._export_folder.text = out
	main._export_res.select(main.EXPORT_RESOLUTIONS.find(1009))
	main._start_export()
	for i in 600:
		await process_frame
		if not main.exporter.is_busy():
			break
	check(main._status_label.text.begins_with("Exported 3 files"), "export: " + main._status_label.text)
	check(FileAccess.file_exists(out.path_join("build.json")), "build.json written")
	main._message.hide()

	print("FAILED: %d" % failures if failures else "ALL PASSED")
	quit(1 if failures else 0)
