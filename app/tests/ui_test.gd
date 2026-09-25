## Drives the real main window: waits for the first preview, edits a
## parameter, adds a node, saves, reloads and exports, then the v0.2 tools:
## undo/redo, examples, the 2D map, mask overlays and ports, and Build.
## Takes screenshots along the way.
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

	# ---- v0.2 -----------------------------------------------------------------

	# Undo / redo through the Edit menu path.
	var before_nodes: int = main.graph.get_nodes().size()
	var terrace: String = main.graph_panel.add_node_at("adjust.terrace", Vector2(640, 220))
	check(main.graph.get_nodes().size() == before_nodes + 1, "terrace added")
	main._on_menu(main.Menu.UNDO)
	check(main.graph.get_nodes().size() == before_nodes and main.graph_panel.get_node_or_null(NodePath(terrace)) == null, "undo removes it from graph and screen")
	check(main._status_label.text.begins_with("Undo: Add Terrace"), "status: " + main._status_label.text)
	main._on_menu(main.Menu.REDO)
	check(main.graph.get_nodes().size() == before_nodes + 1, "redo adds it back")

	# 100+ undo steps of parameter edits.
	main._view_node(fbm)
	var start_seed: int = main.graph.get_nodes().filter(func(n): return n["id"] == fbm)[0]["params"]["seed"]
	for i in 120:
		main.project.break_undo_merge()
		main._on_param_changed(fbm, "seed", 1000 + i)
	for i in 120:
		main.project.undo()
	var seed_now: int = main.graph.get_nodes().filter(func(n): return n["id"] == fbm)[0]["params"]["seed"]
	check(seed_now == start_seed, "120 undos restore the seed (%d)" % seed_now)
	for i in 120:
		main.project.redo()
	main._undo_redo(false)
	main._undo_redo(true)
	seed_now = main.graph.get_nodes().filter(func(n): return n["id"] == fbm)[0]["params"]["seed"]
	check(seed_now == 1119, "redo 120 steps (%d)" % seed_now)
	await wait_preview(main)

	# Examples: the three reference landforms.
	for i in main.EXAMPLES.size():
		main._open_example(i)
		check(await wait_preview(main), "example %s previews" % main.EXAMPLES[i][0])
		check(main.project_path == "" and main.graph.get_nodes().size() >= 4, "example loaded untitled")
		check(main.project.get_exports().size() >= 2, "example has marked outputs")
		main._set_view_2d(false)
		await frames(10)
		await shot("10_example_%d_3d" % i)

	# Water: the River coast example draws its sea, lakes and rivers.
	main._open_example(main.EXAMPLES.size() - 1)
	check(await wait_preview(main), "river coast previews")
	check(main.view._water.visible, "water drawn over the river coast")
	main._view_node("n_0005")
	check(await wait_preview(main), "sea height")
	check(main.view._water.visible, "water drawn over the sea's height")
	main._output_picker.select(2)
	main._output_picker.item_selected.emit(2)
	check(await wait_preview(main), "sea mask")
	check(not main.view._water.visible, "no water over a mask")

	# Erosion: pick each output of Hydraulic Erosion from the toolbar.
	main._open_example(3)
	check(await wait_preview(main), "eroded strata previews")
	main.graph_panel.select_node("n_0002")
	main._view_node("n_0002")
	check(await wait_preview(main), "hydraulic height")
	check(main._output_picker.visible and main._output_picker.item_count == 5, "output picker lists 5 outputs")
	main._output_picker.select(1)
	main._output_picker.item_selected.emit(1)
	check(await wait_preview(main), "flow preview")
	check(main._last_preview.get_port() == "flow" and main._last_preview.get_computed_nodes() == 0, "flow reuses the simulation")
	check(main._last_preview.get_base_node_id() == "n_0002", "flow drapes over the eroded height")
	await frames(10)
	await shot("16_erosion_flow")
	var gn_h: GraphNode = main.graph_panel.get_node(NodePath("n_0002"))
	var marked_label := gn_h.find_children("*", "Label", true, false).filter(func(l): return l.has_meta("port") and l.text.begins_with("▶ "))
	check(marked_label.size() == 1 and marked_label[0].get_meta("port") == "flow", "graph marks the viewed output")
	var ui_path := OS.get_user_data_dir().path_join("ui_erosion.otstudio")
	check(main._save_project(ui_path), "save erosion project")
	main._open_project(ui_path)
	check(await wait_preview(main), "reopened erosion project")
	check(main._viewed_port() == "flow", "viewed output restored: " + main._viewed_port())
	main._open_example(2)
	check(await wait_preview(main), "back to the dune field")

	# 2D map view with a value readout.
	main._set_view_2d(true)
	await frames(5)
	var mv = main.map_view
	var readout: String = mv.readout_at(mv.size * 0.5)
	check(readout.contains("height") and readout.contains(" m"), "2D readout: " + readout)
	check(mv.readout_at(Vector2(-10, -10)) == "", "no readout off the map")
	await shot("11_map_2d")

	# Masks: shown over the terrain they came from, in 2D and 3D.
	# The first Slope in the dune example is upstream of its Blur.
	var slope := ""
	for n in main.graph.get_nodes():
		if n["type"] == "data.slope" and slope == "":
			slope = n["id"]
	main.graph_panel.select_node(slope)
	main._view_node(slope)
	check(await wait_preview(main), "mask preview")
	check(main._last_preview.get_port_type() == "mask" and main._last_preview.get_base_node_id() != "", "mask has a base terrain")
	check(main._stats_label.text.contains("mask"), "stats: " + main._stats_label.text)
	readout = mv.readout_at(mv.size * 0.5)
	check(readout.contains("mask") and readout.contains("terrain"), "mask readout: " + readout)
	main._set_view_2d(false)
	await frames(10)
	await shot("12_mask_overlay")

	# Mask-driven parameter: expose a port from the inspector.
	var levels := ""
	for n in main.graph.get_nodes():
		if n["type"] == "adjust.blur":
			levels = n["id"]
	main.graph_panel.select_node(levels)
	main._view_node(levels)
	await wait_preview(main)
	main.inspector.port_toggled.emit(levels, "strength", true)
	var gn: GraphNode = main.graph_panel.get_node(NodePath(levels))
	check(gn.get_child_count() >= 2, "blur node shows the Strength port row")
	check(main.graph.connect_ports(slope, "out", levels, "p:strength") == "", "slope drives blur strength")
	main.graph_panel.rebuild()
	main._on_graph_edited()
	check(await wait_preview(main), "driven preview")
	await shot("13_param_port")

	# Curve editor in the inspector.
	var curve: String = main.graph_panel.add_node_at("adjust.curve", Vector2(900, 400))
	check(main.graph.connect_ports(levels, "out", curve, "in") == "", "wire curve")
	main._view_node(curve)
	await wait_preview(main)
	main.inspector.param_changed.emit(curve, "curve", PackedVector2Array([Vector2(0, 0), Vector2(0.4, 0.15), Vector2(1, 1)]))
	check(await wait_preview(main), "curve preview")
	await frames(3)
	await shot("14_curve")

	# Build tab: every marked output at once.
	main.side_tabs.current_tab = main.build_panel.get_index()
	main.build_panel.refresh()
	await frames(3)
	await shot("15_build_tab")
	var build_out := OS.get_user_data_dir().path_join("ui_build")
	var marked := 0
	for e in main.project.get_exports():
		marked += e["formats"].size()
	main._start_build(513, build_out)
	for i in 900:
		await process_frame
		if not main.exporter.is_busy():
			break
	check(main._status_label.text.begins_with("Exported"), "build: " + main._status_label.text)
	var info = JSON.parse_string(FileAccess.get_file_as_string(build_out.path_join("build.json")))
	check(info is Dictionary and info["files"].size() == marked, "build.json lists %d marked files" % marked)
	main._message.hide()

	print("FAILED: %d" % failures if failures else "ALL PASSED")
	quit(1 if failures else 0)
