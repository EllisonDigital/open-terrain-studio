## v0.4 benchmark: preview time while "dragging" a noise slider at 1,024², and
## erosion at 2,048², on the GPU and with Force CPU.
##   godot --path app --rendering-driver vulkan --script res://tests/gpu_bench.gd
extends SceneTree

var builder: TerrainBuilder

func _initialize() -> void:
	run.call_deferred()

## Median milliseconds per preview over `edits` parameter changes, measured
## from the request to the result (evaluation, read-back and image).
func slider_drag(project: TerrainProject, node: String, viewed: String, port: String, res: int, edits: int) -> float:
	var graph: TerrainGraph = project.get_graph()
	var times: Array[float] = []
	for i in edits:
		graph.set_param(node, "feature_size_m", 2000.0 + 10.0 * i)
		var t0 := Time.get_ticks_usec()
		builder.request_preview(project, viewed, port, res)
		await builder.preview_ready
		times.append((Time.get_ticks_usec() - t0) / 1000.0)
	times.sort()
	return times[times.size() / 2]

func one(project: TerrainProject, viewed: String, port: String, res: int) -> float:
	TerrainBuilder.clear_cache()
	var t0 := Time.get_ticks_usec()
	builder.request_preview(project, viewed, port, res)
	await builder.preview_ready
	return (Time.get_ticks_usec() - t0) / 1000.0

func run() -> void:
	var status: Dictionary = TerrainBuilder.get_gpu_status()
	while status["state"] == "starting":
		await create_timer(0.05).timeout
		status = TerrainBuilder.get_gpu_status()
	print("Device: ", status.get("name", status.get("reason", "")))
	builder = TerrainBuilder.new()
	root.add_child(builder)

	# Starter graph: fBm -> Levels, plus a longer chain fBm -> Warp -> Blur -> Slope.
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()
	var fbm := graph.add_node("noise.fbm", Vector2.ZERO)
	var levels := graph.add_node("adjust.levels", Vector2(300, 0))
	graph.connect_ports(fbm, "out", levels, "in")
	var warp := graph.add_node("adjust.warp", Vector2(300, 200))
	var blur := graph.add_node("adjust.blur", Vector2(600, 200))
	var slope := graph.add_node("data.slope", Vector2(900, 200))
	graph.connect_ports(fbm, "out", warp, "in")
	graph.connect_ports(warp, "out", blur, "in")
	graph.connect_ports(blur, "out", slope, "in")
	var thermal := graph.add_node("simulate.thermal", Vector2(600, 400))
	graph.connect_ports(fbm, "out", thermal, "in")
	var hydraulic := graph.add_node("simulate.hydraulic", Vector2(600, 600))
	graph.connect_ports(fbm, "out", hydraulic, "in")

	for force_cpu in [false, true]:
		TerrainBuilder.set_force_cpu(force_cpu)
		var label := "CPU" if force_cpu else "GPU"
		# Warm up (first dispatch, allocations).
		await slider_drag(project, fbm, levels, "out", 1024, 3)
		var a := await slider_drag(project, fbm, levels, "out", 1024, 20)
		var b := await slider_drag(project, fbm, slope, "out", 1024, 20)
		var c := await slider_drag(project, fbm, levels, "out", 2048, 10)
		print("%s  slider fBm->Levels 1024²: %6.1f ms (%4.1f fps)   fBm->Warp->Blur->Slope 1024²: %6.1f ms   fBm->Levels 2048²: %6.1f ms" % [
			label, a, 1000.0 / a, b, c])
		var t := await one(project, thermal, "height", 2048)
		print("%s  thermal erosion 2048² (60 s, cold): %7.0f ms" % [label, t])
		var h := await one(project, hydraulic, "height", 2048)
		print("%s  hydraulic erosion 2048² (1000 kyr, cold, CPU solver): %7.0f ms" % [label, h])
	TerrainBuilder.set_force_cpu(false)
	quit(0)
