## v0.5 water test: the water nodes through the extension, and the water
## level the 3D view draws.
## godot --headless --path app --script res://tests/water_test.gd
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
		printerr("Water test timed out")
		quit(1))
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()
	var mountain := graph.add_node("terrain.mountain", Vector2.ZERO)
	var rivers := graph.add_node("simulate.rivers", Vector2(300, 0))
	var lakes := graph.add_node("simulate.lakes", Vector2(600, 0))
	var sea := graph.add_node("simulate.sea", Vector2(900, 0))
	var snow := graph.add_node("simulate.snow", Vector2(1200, 0))
	var flow := graph.add_node("data.flow", Vector2(300, 300))
	var wetness := graph.add_node("data.wetness", Vector2(600, 300))
	for node in [rivers, lakes, sea, snow, flow, wetness]:
		check(node != "", "water node registered")
	check(graph.connect_ports(mountain, "out", rivers, "in") == "", "mountain -> rivers")
	check(graph.connect_ports(rivers, "height", lakes, "in") == "", "rivers -> lakes")
	check(graph.connect_ports(lakes, "height", sea, "in") == "", "lakes -> sea")
	check(graph.connect_ports(sea, "height", snow, "in") == "", "sea -> snow")
	check(graph.connect_ports(mountain, "out", flow, "in") == "", "mountain -> flow")
	check(graph.connect_ports(mountain, "out", wetness, "in") == "", "mountain -> wetness")
	check(graph.connect_ports(sea, "sea", wetness, "water") == "", "sea mask -> wetness water")
	graph.set_param(sea, "sea_level_m", 150.0)
	var builder := TerrainBuilder.new()
	root.add_child(builder)
	builder.preview_failed.connect(func(_generation, error):
		printerr("Unexpected build failure: ", error)
		quit(1))

	var outputs := {
		rivers: ["height", "water_surface", "river", "riverbank"],
		lakes: ["height", "water_surface", "lakes", "shore"],
		sea: ["height", "water_surface", "sea", "shallow", "shoreline"],
		snow: ["height", "snow"],
		flow: ["accumulation", "direction", "basins"],
		wetness: ["wetness", "water_distance"],
	}
	for node in outputs:
		for port in outputs[node]:
			builder.request_preview(project, node, port, 129)
			var preview: TerrainPreview = await builder.preview_ready
			check(preview.get_port() == port and preview.get_image().get_width() == 129, "preview %s" % port)
			if preview.get_port_type() == "mask":
				check(preview.get_min() >= 0.0 and preview.get_max() <= 1.0, "normalised mask " + port)

	# The 3D view's water: the sea (and any river or lake water) over the
	# final terrain, none over the bare mountain or a mask.
	builder.request_preview(project, snow, "height", 129)
	var final: TerrainPreview = await builder.preview_ready
	var water: Image = final.get_water_image()
	check(water != null and water.get_width() == 129, "water level over the final terrain")
	if water != null:
		check(is_equal_approx(water.get_pixel(0, 0).r, 150.0), "sea at its level in the corner: %s" % water.get_pixel(0, 0).r)
		check(water.get_pixel(64, 64).r < -1.0e5, "dry mountain top")
	builder.request_preview(project, mountain, "out", 129)
	check((await builder.preview_ready).get_water_image() == null, "no water over the bare mountain")
	builder.request_preview(project, sea, "sea", 129)
	check((await builder.preview_ready).get_water_image() == null, "no water over a mask")

	print("WATER FAILED: %d" % failures if failures else "WATER ALL PASSED")
	quit(1 if failures else 0)
