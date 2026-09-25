## v0.3 integration test, independent of the v0.2 editor changes.
## godot --headless --path app --script res://tests/erosion_test.gd
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
		printerr("Erosion test timed out")
		quit(1))
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()
	var source := graph.add_node("noise.fbm", Vector2.ZERO)
	var hydraulic := graph.add_node("simulate.hydraulic", Vector2(300, 0))
	var thermal := graph.add_node("simulate.thermal", Vector2(600, 0))
	var hardness := graph.add_node("data.rock_hardness", Vector2(300, 300))
	check(hydraulic != "" and thermal != "" and hardness != "", "erosion nodes registered")
	check(graph.connect_ports(source, "out", hydraulic, "in") == "", "source -> hydraulic")
	check(graph.connect_ports(source, "out", hardness, "in") == "", "source -> hardness")
	check(graph.connect_ports(hardness, "out", hydraulic, "hardness") == "", "hardness -> hydraulic")
	check(graph.connect_ports(hydraulic, "height", thermal, "in") == "", "hydraulic Height -> thermal")
	var builder := TerrainBuilder.new()
	root.add_child(builder)
	builder.preview_failed.connect(func(_generation, error):
		printerr("Unexpected build failure: ", error)
		quit(1))
	for node in [hydraulic, thermal, hardness]:
		var ports: Array = ["height", "flow", "wear", "deposition", "sediment"] if node == hydraulic else (["height", "debris"] if node == thermal else ["out"])
		for port in ports:
			builder.request_preview(project, node, port, 129)
			var preview: TerrainPreview = await builder.preview_ready
			check(preview.get_port() == port and preview.get_node_id() == node, "preview output " + port)
			check(preview.get_image().get_width() == 129, "129² output image")
			# One simulation serves every output: only the first port computes it.
			if port != ports[0]:
				check(preview.get_computed_nodes() == 0, "%s reused the cached simulation" % port)
			if node == hydraulic and port != "height":
				check(preview.get_base_node_id() == hydraulic, "%s drapes over the eroded height" % port)
			if port != "height":
				check(preview.get_port_type() == "mask" and preview.get_min() >= 0.0 and preview.get_max() <= 1.0, "normalised mask " + port)
	# A long-running preview reports progress inside hydraulic, then is cancelled.
	graph.set_param(hydraulic, "duration_kyr", 5000.0)
	var simulation_progress := [false]
	builder.progress.connect(func(_generation, fraction):
		# There are three dependencies: source, hardness, hydraulic.
		if fraction > 0.67 and fraction < 0.99:
			simulation_progress[0] = true)
	builder.request_preview(project, hydraulic, "height", 512)
	while not simulation_progress[0] and builder.is_busy():
		await process_frame
	check(simulation_progress[0], "progress during hydraulic simulation")
	builder.cancel()
	check(not builder.is_busy(), "cancel stops the active job")
	var stale_delivered := [false]
	builder.preview_ready.connect(func(_preview): stale_delivered[0] = true)
	await create_timer(0.1).timeout
	check(not stale_delivered[0], "cancelled result is discarded")
	# The same request again runs to completion: nothing partial was cached.
	graph.set_param(hydraulic, "duration_kyr", 100.0)
	builder.request_preview(project, hydraulic, "height", 256)
	var finished: TerrainPreview = await builder.preview_ready
	check(finished.get_computed_nodes() >= 1, "re-requested simulation completed (%d nodes computed)" % finished.get_computed_nodes())
	print("EROSION FAILED: %d" % failures if failures else "EROSION ALL PASSED")
	quit(1 if failures else 0)
