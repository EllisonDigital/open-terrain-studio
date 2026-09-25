## v0.6 colour test: colour nodes through the extension, gradient
## parameters, colour previews and colour export.
## godot --headless --path app --script res://tests/colour_test.gd
extends SceneTree

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
	create_timer(60.0).timeout.connect(func():
		printerr("Colour test timed out")
		quit(1))
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()
	var terrain := graph.add_node("terrain.mountain", Vector2.ZERO)
	var slope := graph.add_node("data.slope", Vector2(300, 200))
	var grass := graph.add_node("colour.colourise", Vector2(300, 0))
	var rock := graph.add_node("colour.colourise", Vector2(300, 400))
	var blend := graph.add_node("colour.blend", Vector2(600, 0))
	var normals := graph.add_node("output.normal_map", Vector2(600, 300))
	var occlusion := graph.add_node("data.occlusion", Vector2(600, 500))
	var splat := graph.add_node("output.splat", Vector2(900, 300))
	check(graph.connect_ports(terrain, "out", grass, "in") == "", "height -> colourise")
	check(graph.connect_ports(terrain, "out", slope, "in") == "", "height -> slope")
	check(graph.connect_ports(terrain, "out", rock, "in") == "", "height -> colourise rock")
	check(graph.connect_ports(grass, "out", blend, "a") == "", "colour -> blend bottom")
	check(graph.connect_ports(rock, "out", blend, "b") == "", "colour -> blend top")
	check(graph.connect_ports(slope, "out", blend, "mask") == "", "slope -> blend mask")
	check(graph.connect_ports(terrain, "out", normals, "in") == "", "height -> normal map")
	check(graph.connect_ports(terrain, "out", occlusion, "in") == "", "height -> occlusion")
	check(graph.connect_ports(slope, "out", splat, "layer_2") == "", "slope -> splat layer 2")
	check(graph.connect_ports(grass, "out", slope, "in") != "", "a colour map can't feed a mask input")
	check(graph.connect_ports(terrain, "out", blend, "a") != "", "a heightfield can't feed a colour input")

	# Gradient parameters round-trip as [t, r, g, b] stops.
	graph.set_param(rock, "preset", "custom")
	var stops: Variant = graph.set_param(rock, "gradient", [[1.0, 0.9, 0.9, 0.9], [0.0, 0.3, 0.3, 0.3]])
	check(stops is Array and stops.size() == 2 and stops[0][0] == 0.0, "gradient stored sorted: %s" % [stops])

	var builder := TerrainBuilder.new()
	root.add_child(builder)
	builder.preview_failed.connect(func(_generation, error):
		printerr("Unexpected build failure: ", error)
		quit(1))
	builder.request_preview(project, blend, "out", 65)
	var preview: TerrainPreview = await builder.preview_ready
	check(preview.get_port_type() == "color_map", "blend previews as a colour map")
	var img := preview.get_image()
	check(img.get_format() == Image.FORMAT_RGBAF and img.get_width() == 65, "RGBA float image")
	check(preview.get_base_node_id() == terrain, "colour drapes over the terrain it came from")
	check(preview.get_water_image() == null and preview.get_snow_image() == null, "no water or snow on a colour map")
	var c := preview.sample_color(4096.0, 4096.0)
	check(c.a == 1.0 and c.r >= 0.0 and c.r <= 1.0, "sample colour %s" % c)
	for port in ["weights_1_4", "weights_5_8", "weight_1", "weight_2"]:
		builder.request_preview(project, splat, port, 65)
		preview = await builder.preview_ready
		check(preview.get_port() == port and preview.get_max() <= 1.0, "splat " + port)
	builder.request_preview(project, normals, "out", 65)
	check((await builder.preview_ready).get_port_type() == "color_map", "normal map previews")
	builder.request_preview(project, occlusion, "out", 65)
	check((await builder.preview_ready).get_port_type() == "mask", "occlusion previews")

	# Build: colour maps as 8/16-bit RGBA PNG and EXR.
	var dir := OS.get_user_data_dir().path_join("colour_build")
	project.set_export(blend, "out", "png8", true)
	project.set_export(splat, "weights_1_4", "png8", true)
	project.set_export(normals, "out", "png16", true)
	var exporter := TerrainExporter.new()
	root.add_child(exporter)
	check(exporter.request_build(project, 65, dir), "build requested")
	var result: Array = await exporter.export_finished
	var files: PackedStringArray = result[2]
	check(result[0] and files.size() == 4, "build wrote 3 images + build.json: %s" % [result])
	for f in files:
		if f.ends_with(".png"):
			var loaded := Image.load_from_file(f)
			check(loaded != null and loaded.get_width() == 65, "exported %s loads" % f.get_file())

	# The Colour tab: portals bring Terrain outputs over; tabs are saved.
	var portal := graph.send_to_colour_tab(terrain, "out", Vector2(-300, 0))
	check(portal != "", "height sent to the Colour tab")
	var mask_portal := graph.send_to_colour_tab(slope, "out", Vector2(-300, 150))
	check(graph.send_to_colour_tab(blend, "out", Vector2.ZERO) == "", "colour maps can't be sent")
	var tabs := {}
	var types := {}
	for n in graph.get_nodes():
		tabs[n["id"]] = n["tab"]
		types[n["id"]] = n["type"]
	check(tabs[portal] == "colour" and tabs[terrain] == "terrain", "portal in the Colour tab, source in Terrain")
	check(types[portal] == "portal.height" and types[mask_portal] == "portal.mask", "portal types follow the output")
	var paint := graph.add_node_in_tab("colour.colourise", Vector2(0, 0), "colour")
	check(graph.connect_ports(portal, "out", paint, "in") == "", "portal -> colourise")
	builder.request_preview(project, paint, "out", 65)
	preview = await builder.preview_ready
	check(preview.get_port_type() == "color_map" and preview.get_base_node_id() == portal, "colour through a portal drapes over the portal's terrain")
	check(project.undo() == "Connect" and project.undo() == "Add Colourise", "undo tab edits")
	project.redo()
	project.redo()
	var saved := OS.get_user_data_dir().path_join("colour_tabs.otstudio")
	check(project.save(saved), "save with a Colour tab")
	var reopened := TerrainProject.new()
	check(reopened.load(saved), "reopen")
	var reopened_tabs := {}
	for n in reopened.get_graph().get_nodes():
		reopened_tabs[n["id"]] = n["tab"]
	check(reopened_tabs == tabs.merged({paint: "colour"}), "tabs survive saving")

	print("COLOUR FAILED: %d" % failures if failures else "COLOUR ALL PASSED")
	quit(1 if failures else 0)
