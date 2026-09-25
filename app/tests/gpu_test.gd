## v0.4 GPU test: every GPU kernel against its CPU version, and a preview
## computed on the GPU. Needs a Vulkan (or D3D12/Metal) device, so it can't run
## with --headless:
##   godot --path app --rendering-driver vulkan --script res://tests/gpu_test.gd
## A software device works too (Mesa lavapipe, e.g. under xvfb-run in CI).
## Set OTS_GPU_TEST_RES for another check resolution (default 257).
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
	create_timer(600.0).timeout.connect(func():
		printerr("GPU test timed out")
		quit(1))
	var status: Dictionary = TerrainBuilder.get_gpu_status()
	while status["state"] == "starting":
		await create_timer(0.05).timeout
		status = TerrainBuilder.get_gpu_status()
	if status["state"] != "ready":
		printerr("No GPU: ", status.get("reason", "?"))
		print("GPU FAILED: no device")
		quit(1)
		return
	print("Device: ", status["name"])

	var res := int(OS.get_environment("OTS_GPU_TEST_RES")) if OS.get_environment("OTS_GPU_TEST_RES") != "" else 257
	var reports: Array = TerrainBuilder.run_gpu_check(res)
	check(reports.size() > 20, "%d kernel checks ran at %d²" % [reports.size(), res])
	for r in reports:
		var line := "%-58s %-7s max %-12s over %.3f%%  (tol %s)  cpu %6.1f ms  gpu %6.1f ms" % [
			r["case"], r["port"], "%.2f ppm" % (r["max_error"] * 1e6), r["outliers"] * 100.0,
			"%.0f ppm" % (r["tolerance"] * 1e6), r["cpu_ms"], r["gpu_ms"]]
		if not r["passed"]:
			line += "  worst " + r["worst"]
		if r["error"] != "":
			line += "  ERROR " + r["error"]
		check(r["passed"], line)

	# A preview uses the GPU and matches the CPU.
	var project := TerrainProject.new()
	var graph: TerrainGraph = project.get_graph()
	var fbm := graph.add_node("noise.fbm", Vector2.ZERO)
	var blur := graph.add_node("adjust.blur", Vector2(300, 0))
	graph.connect_ports(fbm, "out", blur, "in")
	var builder := TerrainBuilder.new()
	root.add_child(builder)
	builder.request_preview(project, blur, "out", 513)
	var on_gpu: TerrainPreview = await builder.preview_ready
	check(on_gpu.get_gpu_name() == status["name"], "preview computed on the GPU")
	TerrainBuilder.set_force_cpu(true)
	builder.request_preview(project, blur, "out", 513)
	var on_cpu: TerrainPreview = await builder.preview_ready
	TerrainBuilder.set_force_cpu(false)
	check(on_cpu.get_gpu_name() == "", "Force CPU previews on the CPU")
	check(on_cpu.get_computed_nodes() == 2, "CPU results are cached separately from GPU ones")
	var worst := 0.0
	for i in 50:
		var x := 8192.0 * i / 49.0
		worst = maxf(worst, absf(on_gpu.sample(x, 8192.0 - x) - on_cpu.sample(x, 8192.0 - x)))
	check(worst < 0.2, "GPU and CPU previews agree (%.4f m)" % worst)
	status = TerrainBuilder.get_gpu_status()
	check(status["fallbacks"] == 0, "no GPU fallbacks (%s)" % status.get("last_error", ""))
	print("GPU FAILED: %d" % failures if failures else "GPU ALL PASSED")
	quit(1 if failures else 0)
