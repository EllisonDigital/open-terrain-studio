## OpenTerrainStudio main window.
##
## Layout (Gaea-style): 3D viewport top-left, node graph bottom-left,
## inspector on the right, menu bar on top, status bar at the bottom.
## All terrain data lives in the Rust core; this script only wires UI to it.
extends Control

const TerrainView := preload("res://scripts/terrain_view.gd")
const MapView := preload("res://scripts/map_view.gd")
const GraphPanel := preload("res://scripts/graph_panel.gd")
const Inspector := preload("res://scripts/inspector.gd")
const BuildPanel := preload("res://scripts/build_panel.gd")

const PREVIEW_RESOLUTIONS := [256, 512, 1024, 2048]
const EXPORT_RESOLUTIONS := [512, 1009, 1024, 2017, 2048, 4033, 4096, 8129, 8192]
const SETTINGS_PATH := "user://settings.cfg"
const REPO_URL := "https://github.com/EllisonDigital/open-terrain-studio"
## Example projects bundled with the app, built only from built-in nodes.
const EXAMPLES := [
	["Alpine range", "res://examples/alpine_range.otstudio"],
	["Canyon", "res://examples/canyon.otstudio"],
	["Dune field", "res://examples/dune_field.otstudio"],
	["Eroded strata", "res://examples/eroded_strata.otstudio"],
	["Asterfall Crown — Hero World", "res://examples/asterfall_crown.otstudio"],
	["River coast", "res://examples/river_coast.otstudio"],
]

enum Menu { NEW, OPEN, SAVE, SAVE_AS, EXPORT, QUIT, WORLD, DOCS, ABOUT, UNDO, REDO, MARK_EXPORT, BUILD,
		AUTO_UPDATE, UPDATE_PREVIEW, FORCE_CPU, EXAMPLE = 100 }

var project: TerrainProject
var graph: TerrainGraph
var builder: TerrainBuilder
var exporter: TerrainExporter

var view: SubViewportContainer
var map_view: Control
var graph_panel: GraphEdit
var inspector: ScrollContainer
var build_panel: ScrollContainer
var side_tabs: TabContainer

var project_path := ""
var viewed_id := ""
## Output of the viewed node shown in the viewport (nodes like Hydraulic
## Erosion have several: Height, Flow, Wear…). "" = its first output.
var viewed_port := ""
var preview_resolution := 512
var view_2d := false
## Recompute the preview after every edit; off = only on Update (F5).
var auto_update := true
## Machine settings (not saved in projects): user://settings.cfg.
var settings := ConfigFile.new()

var _status_label: Label
var _stats_label: Label
var _progress: ProgressBar
var _open_dialog: FileDialog
var _save_dialog: FileDialog
var _export_dialog: ConfirmationDialog
var _export_res: OptionButton
var _export_exr: CheckBox
var _export_png: CheckBox
var _export_folder: LineEdit
var _folder_dialog: FileDialog
var _confirm: ConfirmationDialog
var _confirm_action: Callable
var _message: AcceptDialog
var _edit_menu: PopupMenu
var _view_switch: OptionButton
var _output_label: Label
var _output_picker: OptionButton
var _last_preview: TerrainPreview
var _settings_menu: PopupMenu
var _update_button: Button
var _res_picker: OptionButton
var _preview_stale := false
var _gpu_label: Label
var _gpu_state := ""


func _ready() -> void:
	get_tree().set_auto_accept_quit(false)
	_load_settings()
	project = TerrainProject.new()
	graph = project.get_graph()

	builder = TerrainBuilder.new()
	builder.progress.connect(_on_build_progress)
	builder.preview_ready.connect(_on_preview_ready)
	builder.preview_failed.connect(_on_preview_failed)
	add_child(builder)

	exporter = TerrainExporter.new()
	exporter.progress.connect(func(f): _progress.value = f * 100.0)
	exporter.export_finished.connect(_on_export_finished)
	add_child(exporter)

	_build_ui()
	_build_dialogs()
	_new_project_with_starter_graph()
	_poll_gpu_status()


# ---- layout ---------------------------------------------------------------

func _build_ui() -> void:
	var root := VBoxContainer.new()
	root.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	root.add_theme_constant_override("separation", 0)
	add_child(root)

	# Menu bar.
	var menubar := MenuBar.new()
	var file_menu := PopupMenu.new()
	file_menu.name = "File"
	file_menu.add_item("New", Menu.NEW, KEY_MASK_CMD_OR_CTRL | KEY_N)
	file_menu.add_item("Open…", Menu.OPEN, KEY_MASK_CMD_OR_CTRL | KEY_O)
	var examples := PopupMenu.new()
	examples.name = "examples"
	for i in EXAMPLES.size():
		examples.add_item(EXAMPLES[i][0], Menu.EXAMPLE + i)
	examples.id_pressed.connect(_on_menu)
	file_menu.add_child(examples)
	file_menu.add_submenu_node_item("Open Example", examples)
	file_menu.add_separator()
	file_menu.add_item("Save", Menu.SAVE, KEY_MASK_CMD_OR_CTRL | KEY_S)
	file_menu.add_item("Save As…", Menu.SAVE_AS, KEY_MASK_CMD_OR_CTRL | KEY_MASK_SHIFT | KEY_S)
	file_menu.add_separator()
	file_menu.add_item("Export Viewed Node…", Menu.EXPORT, KEY_MASK_CMD_OR_CTRL | KEY_E)
	file_menu.add_separator()
	file_menu.add_item("Quit", Menu.QUIT, KEY_MASK_CMD_OR_CTRL | KEY_Q)
	file_menu.id_pressed.connect(_on_menu)
	menubar.add_child(file_menu)
	_edit_menu = PopupMenu.new()
	_edit_menu.name = "Edit"
	_edit_menu.add_item("Undo", Menu.UNDO, KEY_MASK_CMD_OR_CTRL | KEY_Z)
	_edit_menu.add_item("Redo", Menu.REDO, KEY_MASK_CMD_OR_CTRL | KEY_MASK_SHIFT | KEY_Z)
	_edit_menu.id_pressed.connect(_on_menu)
	_edit_menu.about_to_popup.connect(_update_edit_menu)
	menubar.add_child(_edit_menu)
	var build_menu := PopupMenu.new()
	build_menu.name = "Build"
	build_menu.add_item("Mark Viewed Output for Export", Menu.MARK_EXPORT, KEY_MASK_CMD_OR_CTRL | KEY_MASK_SHIFT | KEY_E)
	build_menu.add_item("Build Marked Outputs", Menu.BUILD, KEY_MASK_CMD_OR_CTRL | KEY_B)
	build_menu.id_pressed.connect(_on_menu)
	menubar.add_child(build_menu)
	var project_menu := PopupMenu.new()
	project_menu.name = "Project"
	project_menu.add_item("World Settings", Menu.WORLD)
	project_menu.id_pressed.connect(_on_menu)
	menubar.add_child(project_menu)
	_settings_menu = PopupMenu.new()
	_settings_menu.name = "Settings"
	_settings_menu.add_check_item("Auto-update Preview", Menu.AUTO_UPDATE)
	_settings_menu.add_item("Update Preview", Menu.UPDATE_PREVIEW, KEY_F5)
	_settings_menu.add_separator()
	_settings_menu.add_check_item("Force CPU (debugging)", Menu.FORCE_CPU)
	_settings_menu.set_item_tooltip(_settings_menu.get_item_index(Menu.FORCE_CPU),
			"Compute every node on the CPU even when a GPU is available.")
	_settings_menu.id_pressed.connect(_on_menu)
	_settings_menu.about_to_popup.connect(_update_settings_menu)
	menubar.add_child(_settings_menu)
	var help_menu := PopupMenu.new()
	help_menu.name = "Help"
	help_menu.add_item("Documentation", Menu.DOCS)
	help_menu.add_item("About OpenTerrainStudio", Menu.ABOUT)
	help_menu.id_pressed.connect(_on_menu)
	menubar.add_child(help_menu)
	root.add_child(menubar)

	# Main area.
	var hsplit := HSplitContainer.new()
	hsplit.size_flags_vertical = Control.SIZE_EXPAND_FILL
	root.add_child(hsplit)

	var vsplit := VSplitContainer.new()
	vsplit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	hsplit.add_child(vsplit)

	var view_box := VBoxContainer.new()
	view_box.size_flags_vertical = Control.SIZE_EXPAND_FILL
	view_box.size_flags_stretch_ratio = 1.8
	view_box.add_theme_constant_override("separation", 0)
	vsplit.add_child(view_box)
	view = TerrainView.new()
	map_view = MapView.new()
	view_box.add_child(_build_view_toolbar())
	var stack := Control.new()
	stack.size_flags_vertical = Control.SIZE_EXPAND_FILL
	view_box.add_child(stack)
	view.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	stack.add_child(view)
	map_view.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	map_view.visible = false
	stack.add_child(map_view)

	graph_panel = GraphPanel.new()
	graph_panel.size_flags_vertical = Control.SIZE_EXPAND_FILL
	graph_panel.node_activated.connect(_on_node_activated)
	graph_panel.selection_cleared.connect(_on_selection_cleared)
	graph_panel.graph_edited.connect(_on_graph_edited)
	graph_panel.layout_edited.connect(_update_title)
	graph_panel.status.connect(_set_status)
	graph_panel.project = project
	vsplit.add_child(graph_panel)

	side_tabs = TabContainer.new()
	side_tabs.custom_minimum_size = Vector2(340, 0)
	hsplit.add_child(side_tabs)
	inspector = Inspector.new()
	inspector.name = "Settings"
	inspector.project = project
	inspector.param_changed.connect(_on_param_changed)
	inspector.port_toggled.connect(_on_port_toggled)
	inspector.export_toggled.connect(_on_export_toggled)
	inspector.world_changed.connect(_on_world_changed)
	side_tabs.add_child(inspector)
	build_panel = BuildPanel.new()
	build_panel.name = "Build"
	build_panel.project = project
	build_panel.builds_on_gpu = settings.get_value("compute", "builds_on_gpu", false)
	build_panel.builds_on_gpu_toggled.connect(func(on):
		settings.set_value("compute", "builds_on_gpu", on)
		_save_settings()
		TerrainBuilder.set_builds_on_gpu(on))
	build_panel.build_requested.connect(_start_build)
	build_panel.export_toggled.connect(_on_export_toggled)
	build_panel.view_requested.connect(func(id):
		graph_panel.select_node(id)
		_view_node(id))
	side_tabs.add_child(build_panel)

	# Status bar.
	var status_bar := PanelContainer.new()
	var sb := HBoxContainer.new()
	status_bar.add_child(sb)
	_status_label = Label.new()
	_status_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_status_label.clip_text = true
	sb.add_child(_status_label)
	_stats_label = Label.new()
	_stats_label.modulate = Color(1, 1, 1, 0.7)
	sb.add_child(_stats_label)
	_gpu_label = Label.new()
	_gpu_label.modulate = Color(1, 1, 1, 0.7)
	_gpu_label.mouse_filter = Control.MOUSE_FILTER_PASS
	sb.add_child(_gpu_label)
	_progress = ProgressBar.new()
	_progress.custom_minimum_size = Vector2(180, 0)
	_progress.show_percentage = false
	_progress.visible = false
	sb.add_child(_progress)
	root.add_child(status_bar)


func _build_view_toolbar() -> Control:
	var bar := HBoxContainer.new()
	bar.add_theme_constant_override("separation", 12)

	_view_switch = OptionButton.new()
	_view_switch.add_item("3D")
	_view_switch.add_item("2D map")
	_view_switch.tooltip_text = "3D terrain, or the 2D map with the value under the mouse (Tab)"
	_view_switch.item_selected.connect(func(i): _set_view_2d(i == 1))
	bar.add_child(_view_switch)

	_output_label = _label("Output")
	bar.add_child(_output_label)
	_output_picker = OptionButton.new()
	_output_picker.tooltip_text = "Which of the viewed node's outputs to show, export and mark"
	_output_picker.item_selected.connect(func(i):
		_select_output(String(_output_picker.get_item_metadata(i))))
	bar.add_child(_output_picker)
	_output_label.visible = false
	_output_picker.visible = false

	bar.add_child(_label("Preview"))
	var res := OptionButton.new()
	for r in PREVIEW_RESOLUTIONS:
		res.add_item("%d²" % r)
	res.select(PREVIEW_RESOLUTIONS.find(preview_resolution))
	res.tooltip_text = "Preview resolution. Parameters are in metres, so higher resolutions only add detail."
	res.item_selected.connect(func(i):
		preview_resolution = PREVIEW_RESOLUTIONS[i]
		_request_preview())
	bar.add_child(res)
	_res_picker = res

	_update_button = Button.new()
	_update_button.text = "Update (F5)"
	_update_button.tooltip_text = "Recompute the preview. Auto-update is off (Settings menu)."
	_update_button.pressed.connect(_request_preview)
	_update_button.visible = not auto_update
	bar.add_child(_update_button)

	bar.add_child(_label("View"))
	var mode := OptionButton.new()
	for m in ["Natural", "Clay", "Height colours"]:
		mode.add_item(m)
	mode.tooltip_text = "Colouring. Masks are always shown as a colour overlay on the terrain they came from."
	mode.item_selected.connect(func(i):
		view.set_view_mode(i)
		map_view.set_view_mode(i))
	bar.add_child(mode)

	bar.add_child(_label("Vertical scale"))
	var exag := SpinBox.new()
	exag.min_value = 0.1
	exag.max_value = 10.0
	exag.step = 0.1
	exag.value = 1.0
	exag.suffix = "×"
	exag.value_changed.connect(func(v): view.set_exaggeration(v))
	bar.add_child(exag)

	bar.add_child(_label("Sun"))
	var sun := HSlider.new()
	sun.min_value = -180
	sun.max_value = 180
	sun.value = -130
	sun.custom_minimum_size.x = 120
	sun.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	sun.tooltip_text = "Sun direction"
	sun.value_changed.connect(func(v): view.set_sun_angle(v, 38.0))
	bar.add_child(sun)

	var frame := Button.new()
	frame.text = "Frame (F)"
	frame.pressed.connect(func():
		view.frame_all()
		map_view.fit())
	bar.add_child(frame)
	return bar


func _set_view_2d(on: bool) -> void:
	view_2d = on
	_view_switch.select(1 if on else 0)
	view.visible = not on
	map_view.visible = on
	if _last_preview != null:
		(map_view if on else view).show_preview(_last_preview)


func _label(text: String) -> Label:
	var l := Label.new()
	l.text = text
	l.modulate = Color(1, 1, 1, 0.7)
	return l


func _build_dialogs() -> void:
	var ext: String = TerrainProject.get_file_extension()
	_open_dialog = FileDialog.new()
	_open_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_open_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_open_dialog.use_native_dialog = true
	_open_dialog.filters = PackedStringArray(["*.%s ; OpenTerrainStudio project" % ext])
	_open_dialog.file_selected.connect(_open_project)
	add_child(_open_dialog)

	_save_dialog = FileDialog.new()
	_save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	_save_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_save_dialog.use_native_dialog = true
	_save_dialog.filters = _open_dialog.filters
	_save_dialog.file_selected.connect(func(path):
		if path.get_extension() != ext:
			path += "." + ext
		_save_project(path))
	add_child(_save_dialog)

	_folder_dialog = FileDialog.new()
	_folder_dialog.file_mode = FileDialog.FILE_MODE_OPEN_DIR
	_folder_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_folder_dialog.use_native_dialog = true
	_folder_dialog.dir_selected.connect(func(dir): _export_folder.text = dir)
	add_child(_folder_dialog)

	# Export dialog.
	_export_dialog = ConfirmationDialog.new()
	_export_dialog.title = "Export"
	_export_dialog.ok_button_text = "Export"
	var v := VBoxContainer.new()
	v.custom_minimum_size = Vector2(440, 0)
	v.add_child(_label("Resolution (samples per side)"))
	_export_res = OptionButton.new()
	for r in EXPORT_RESOLUTIONS:
		var note := "  (Unreal landscape size)" if r in [1009, 2017, 4033, 8129] else ""
		_export_res.add_item("%d%s" % [r, note])
	v.add_child(_export_res)
	v.add_child(_label("Formats"))
	_export_exr = CheckBox.new()
	_export_exr.text = "EXR 32-bit float (heights in metres) — Blender, Godot"
	_export_exr.button_pressed = true
	v.add_child(_export_exr)
	_export_png = CheckBox.new()
	_export_png.text = "PNG 16-bit (0–65535 = world height range) — Unreal, Godot"
	_export_png.button_pressed = true
	v.add_child(_export_png)
	v.add_child(_label("Folder"))
	var fh := HBoxContainer.new()
	_export_folder = LineEdit.new()
	_export_folder.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	fh.add_child(_export_folder)
	var browse := Button.new()
	browse.text = "Browse…"
	browse.pressed.connect(func(): _folder_dialog.popup_centered_ratio(0.6))
	fh.add_child(browse)
	v.add_child(fh)
	var hint := _label("A build.json with the world size, height range and Unreal import values is written alongside.")
	hint.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	# Without a width, the first popup measures the wrapped text one word per
	# line and the dialog grows taller than the screen.
	hint.custom_minimum_size = Vector2(440, 0)
	v.add_child(hint)
	_export_dialog.add_child(v)
	_export_dialog.confirmed.connect(_start_export)
	add_child(_export_dialog)

	_confirm = ConfirmationDialog.new()
	_confirm.title = "Unsaved changes"
	_confirm.dialog_text = "Discard unsaved changes?"
	_confirm.ok_button_text = "Discard"
	_confirm.confirmed.connect(func(): _confirm_action.call())
	add_child(_confirm)

	_message = AcceptDialog.new()
	add_child(_message)


# ---- project lifecycle ----------------------------------------------------

func _new_project_with_starter_graph() -> void:
	project.new_project()
	project_path = ""
	# A useful starting point: fractal noise remapped to the world height range.
	var fbm := graph.add_node("noise.fbm", Vector2(40, 60))
	var levels := graph.add_node("adjust.levels", Vector2(340, 60))
	graph.connect_ports(fbm, "out", levels, "in")
	project.reset_history()
	_after_project_loaded(levels)
	_set_status("New project. Right-click the graph to add nodes; click a node to view it.")


func _open_project(path: String) -> void:
	if not project.load(path):
		_show_message("Could not open project", project.get_last_error())
		return
	project_path = path
	_restore_ui_state()
	_set_status("Opened %s" % path.get_file())


## Open a bundled example as a new, untitled project.
func _open_example(index: int) -> void:
	var example: Array = EXAMPLES[index]
	var json := FileAccess.get_file_as_string(example[1])
	if json == "" or not project.load_json(json):
		_show_message("Could not open example", project.get_last_error())
		return
	project_path = ""
	_restore_ui_state()
	_set_status("Opened the %s example. Save it to keep your changes." % example[0])


func _restore_ui_state() -> void:
	var ui: Variant = JSON.parse_string(project.get_ui_state())
	var state: Dictionary = ui if ui is Dictionary else {}
	var viewed: String = state.get("viewed_node", "")
	if not graph.has_node(viewed):
		var nodes := graph.get_nodes()
		viewed = nodes[-1]["id"] if nodes.size() > 0 else ""
	viewed_port = state.get("viewed_port", "")
	if state.has("preview_resolution") and int(state["preview_resolution"]) in PREVIEW_RESOLUTIONS:
		preview_resolution = int(state["preview_resolution"])
		_res_picker.select(PREVIEW_RESOLUTIONS.find(preview_resolution))
	_after_project_loaded(viewed)
	view.set_camera_state(state.get("camera", {}))
	_set_view_2d(state.get("view_2d", false))
	var warnings := project.get_warnings()
	if warnings.size() > 0:
		_show_message("Opened with warnings", "\n".join(warnings))


func _save_project(path: String) -> bool:
	project.set_ui_state(JSON.stringify({
		"viewed_node": viewed_id,
		"viewed_port": _viewed_port(),
		"preview_resolution": preview_resolution,
		"camera": view.get_camera_state(),
		"view_2d": view_2d,
	}))
	if not project.save(path):
		_show_message("Could not save project", project.get_last_error())
		return false
	project_path = path
	_update_title()
	_set_status("Saved %s" % path.get_file())
	return true


func _after_project_loaded(viewed: String) -> void:
	graph_panel.set_graph(graph)
	_apply_world()
	view.clear()
	map_view.clear()
	_last_preview = null
	view.frame_all()
	map_view.fit()
	build_panel.refresh()
	viewed_id = ""
	if viewed != "":
		graph_panel.select_node(viewed)
		_view_node(viewed, viewed_port)
	else:
		inspector.show_world()
	_update_title()


func _apply_world() -> void:
	view.set_world(project.get_world_size(), project.get_height_min(), project.get_height_max())
	map_view.set_world(project.get_world_size(), project.get_height_min(), project.get_height_max())


func _update_title() -> void:
	var name := project_path.get_file() if project_path != "" else "Untitled"
	var dirty := "*" if project.is_modified() else ""
	get_window().title = "%s%s — OpenTerrainStudio %s" % [name, dirty, TerrainProject.get_app_version()]


## Run `action` now, or after confirming if there are unsaved changes.
func _confirm_discard(action: Callable) -> void:
	if project.is_modified():
		_confirm_action = action
		_confirm.popup_centered()
	else:
		action.call()


# ---- viewing & editing ----------------------------------------------------

## View a node's output (its first output unless `port` names another).
func _view_node(id: String, port := "") -> void:
	viewed_id = id
	viewed_port = port
	_update_output_picker()
	graph_panel.set_viewed(id, _viewed_port())
	_show_inspector_for(id)
	_request_preview()


## Show another output of the viewed node.
func _select_output(port: String) -> void:
	viewed_port = port
	graph_panel.set_viewed(viewed_id, _viewed_port())
	_request_preview()


## The viewed output: `viewed_port` if the node still has it, else its first output.
func _viewed_port() -> String:
	var outputs := _outputs_of(viewed_id)
	for o in outputs:
		if o["key"] == viewed_port:
			return viewed_port
	return outputs[0]["key"] if outputs.size() > 0 else ""


func _outputs_of(id: String) -> Array:
	for n in graph.get_nodes():
		if n["id"] == id:
			return n["outputs"]
	return []


func _update_output_picker() -> void:
	var outputs := _outputs_of(viewed_id)
	_output_picker.clear()
	var current := _viewed_port()
	for o in outputs:
		_output_picker.add_item(o["label"])
		_output_picker.set_item_metadata(_output_picker.item_count - 1, o["key"])
		if o["key"] == current:
			_output_picker.select(_output_picker.item_count - 1)
	_output_label.visible = outputs.size() > 1
	_output_picker.visible = outputs.size() > 1


func _show_inspector_for(id: String) -> void:
	for n in graph.get_nodes():
		if n["id"] == id:
			inspector.show_node(n, graph.get_node_type(n["type"]))
			return
	inspector.show_world()


## The terrain changed: update the preview now, or mark it out of date when
## auto-update is off.
func _terrain_edited() -> void:
	if auto_update:
		_request_preview()
	else:
		_preview_stale = true
		_update_button.modulate = Color(1.0, 0.85, 0.4)
		_update_button.tooltip_text = "The preview is out of date. Recompute it (F5)."


func _request_preview() -> void:
	_preview_stale = false
	if _update_button != null:
		_update_button.modulate = Color.WHITE
		_update_button.tooltip_text = "Recompute the preview. Auto-update is off (Settings menu)."
	if viewed_id == "" or not graph.has_node(viewed_id):
		builder.cancel()
		view.clear()
		map_view.clear()
		_last_preview = null
		_stats_label.text = ""
		return
	var port := _viewed_port()
	if port == "":
		_set_status("This node has no output to preview.")
		return
	_progress.visible = true
	_progress.value = 0
	builder.request_preview(project, viewed_id, port, preview_resolution)


func _on_node_activated(id: String) -> void:
	if id != viewed_id:
		_view_node(id)
	else:
		_show_inspector_for(id)


func _on_selection_cleared() -> void:
	inspector.show_world()
	if not graph.has_node(viewed_id):
		viewed_id = ""
		_request_preview()


func _on_graph_edited() -> void:
	_update_title()
	if viewed_id != "" and not graph.has_node(viewed_id):
		viewed_id = ""
	build_panel.refresh()
	_terrain_edited()


func _on_param_changed(node_id: String, key: String, value: Variant) -> void:
	var stored: Variant = graph.set_param(node_id, key, value)
	if stored == null:
		_set_status(graph.get_last_error())
		return
	if typeof(stored) != TYPE_ARRAY and stored != value:
		inspector.refresh_value(key, stored)
	_update_title()
	_terrain_edited()


func _on_port_toggled(node_id: String, key: String, exposed: bool) -> void:
	if not graph.set_param_exposed(node_id, key, exposed):
		_set_status(graph.get_last_error())
	graph_panel.rebuild()
	_show_inspector_for(node_id)
	_on_graph_edited()
	if exposed:
		_set_status("Connect a mask to the new port on the node to drive this value.")


func _on_export_toggled(node_id: String, port: String, format: String, on: bool) -> void:
	if not project.set_export(node_id, port, format, on):
		_set_status(project.get_last_error())
	graph_panel.rebuild()
	build_panel.refresh()
	if inspector.get_node_id() == node_id:
		_show_inspector_for(node_id)
	_update_title()


func _on_world_changed() -> void:
	_apply_world()
	_update_title()
	_terrain_edited()


# ---- undo / redo ------------------------------------------------------------

func _undo_redo(redo: bool) -> void:
	var label: String = project.redo() if redo else project.undo()
	if label == "":
		_set_status("Nothing to %s." % ("redo" if redo else "undo"))
		return
	# Everything may have changed: redraw from the core.
	graph_panel.rebuild()
	_apply_world()
	if viewed_id != "" and not graph.has_node(viewed_id):
		viewed_id = ""
	graph_panel.set_viewed(viewed_id, _viewed_port())
	_update_output_picker()
	var shown: String = inspector.get_node_id()
	if shown != "" and graph.has_node(shown):
		_show_inspector_for(shown)
	else:
		inspector.show_world()
	build_panel.refresh()
	_update_title()
	_terrain_edited()
	_set_status("%s: %s" % ["Redo" if redo else "Undo", label])


func _update_edit_menu() -> void:
	var u := project.get_undo_label()
	var r := project.get_redo_label()
	_edit_menu.set_item_text(_edit_menu.get_item_index(Menu.UNDO), "Undo " + u if u != "" else "Undo")
	_edit_menu.set_item_text(_edit_menu.get_item_index(Menu.REDO), "Redo " + r if r != "" else "Redo")
	_edit_menu.set_item_disabled(_edit_menu.get_item_index(Menu.UNDO), not project.can_undo())
	_edit_menu.set_item_disabled(_edit_menu.get_item_index(Menu.REDO), not project.can_redo())


func _unhandled_key_input(event: InputEvent) -> void:
	var k := event as InputEventKey
	if k == null or not k.pressed or k.echo:
		return
	# Ctrl+Y is the other common Redo shortcut.
	if k.keycode == KEY_Y and k.is_command_or_control_pressed():
		_undo_redo(true)
		get_viewport().set_input_as_handled()
	elif k.keycode == KEY_TAB and not k.is_command_or_control_pressed():
		_set_view_2d(not view_2d)
		get_viewport().set_input_as_handled()


# ---- builder / exporter callbacks -----------------------------------------

func _on_build_progress(_generation: int, fraction: float) -> void:
	_progress.value = fraction * 100.0


func _on_preview_ready(preview: TerrainPreview) -> void:
	_progress.visible = false
	_last_preview = preview
	if view_2d:
		map_view.show_preview(preview)
	else:
		view.show_preview(preview)
	var span_text := "%.0f – %.0f m" % [preview.get_min(), preview.get_max()]
	if preview.get_port_type() == "color_map":
		span_text = "colour"
		if preview.get_base_node_id() != "":
			span_text += " on %s" % preview.get_base_node_id()
	elif preview.get_port_type() == "mask":
		span_text = "mask %.2f – %.2f" % [preview.get_min(), preview.get_max()]
		if preview.get_base_node_id() != "":
			span_text += " on %s" % preview.get_base_node_id()
	var n := preview.get_computed_nodes()
	_stats_label.text = "%d²  ·  %s  ·  %s, %.0f ms" % [
		preview.get_resolution(), span_text,
		"from cache" if n == 0 else "%d node%s computed" % [n, "" if n == 1 else "s"],
		preview.get_millis()]
	_update_gpu_label()
	_set_status("")


func _on_preview_failed(_generation: int, message: String) -> void:
	_progress.visible = false
	view.clear()
	map_view.clear()
	_last_preview = null
	_stats_label.text = ""
	_set_status("⚠ " + message)


func _start_export() -> void:
	var formats := PackedStringArray()
	if _export_exr.button_pressed:
		formats.append("exr32")
	if _export_png.button_pressed:
		formats.append("png16")
	var folder := _export_folder.text.strip_edges()
	if formats.is_empty() or folder == "":
		_show_message("Export", "Choose at least one format and a folder.")
		return
	var res: int = EXPORT_RESOLUTIONS[_export_res.selected]
	project.set_build_resolution(res)
	project.set_build_folder(folder)
	if exporter.request_export(project, viewed_id, _viewed_port(), res, folder, formats):
		_progress.visible = true
		_progress.value = 0
		_set_status("Exporting at %d²…" % res)


## Build every marked output (Build tab / Ctrl+B).
func _start_build(resolution: int, folder: String) -> void:
	if project.get_exports().is_empty():
		side_tabs.current_tab = build_panel.get_index()
		_show_message("Build", "Nothing is marked for export yet. Select a node and tick a format under Export in its settings.")
		return
	if folder == "":
		folder = "output"
	if project.get_project_dir() == "" and not folder.is_absolute_path():
		folder = _default_export_folder()
	project.set_build_resolution(resolution)
	project.set_build_folder(folder)
	if exporter.request_build(project, resolution, folder):
		build_panel.busy = true
		_progress.visible = true
		_progress.value = 0
		_set_status("Building %d output%s at %d²…" % [project.get_exports().size(), "" if project.get_exports().size() == 1 else "s", resolution])


func _on_export_finished(ok: bool, message: String, files: PackedStringArray) -> void:
	_progress.visible = false
	build_panel.busy = false
	build_panel.refresh()
	_update_title()
	if ok:
		_set_status("Exported %d files to %s" % [files.size(), files[0].get_base_dir()])
		_show_message("Export complete", "Written:\n" + "\n".join(Array(files).map(func(f): return String(f).get_file())))
	else:
		_show_message("Export failed", message)


# ---- menus ----------------------------------------------------------------

func _on_menu(id: int) -> void:
	if id >= Menu.EXAMPLE:
		_confirm_discard(_open_example.bind(id - Menu.EXAMPLE))
		return
	match id:
		Menu.NEW:
			_confirm_discard(_new_project_with_starter_graph)
		Menu.OPEN:
			_confirm_discard(func(): _open_dialog.popup_centered_ratio(0.6))
		Menu.SAVE:
			if project_path == "":
				_save_dialog.popup_centered_ratio(0.6)
			else:
				_save_project(project_path)
		Menu.SAVE_AS:
			_save_dialog.popup_centered_ratio(0.6)
		Menu.EXPORT:
			if viewed_id == "":
				_show_message("Export", "Select the node to export first.")
				return
			_export_res.select(maxi(EXPORT_RESOLUTIONS.find(project.get_build_resolution()), 0))
			if _export_folder.text == "":
				_export_folder.text = _default_export_folder()
			_export_dialog.popup_centered()
		Menu.QUIT:
			_confirm_discard(func(): get_tree().quit())
		Menu.UNDO:
			_undo_redo(false)
		Menu.REDO:
			_undo_redo(true)
		Menu.MARK_EXPORT:
			if viewed_id == "":
				_show_message("Export", "View a node first (click it in the graph).")
				return
			var port := _viewed_port()
			var marked := not project.get_export_formats(viewed_id, port).is_empty()
			project.begin_edit_group("Unmark for export" if marked else "Mark for export")
			# Colour maps are marked as 8-bit PNG (what engines load), the rest as EXR + 16-bit PNG.
			var is_color := _last_preview != null and _last_preview.get_port_type() == "color_map"
			var mark_formats := ["png8"] if is_color else ["exr32", "png16"]
			for f in ["exr32", "png16", "png8"]:
				project.set_export(viewed_id, port, f, not marked and f in mark_formats)
			project.end_edit_group()
			_on_export_toggled(viewed_id, port, "exr32", not marked)
			_set_status("%s %s for export." % ["Unmarked" if marked else "Marked", viewed_id])
		Menu.BUILD:
			side_tabs.current_tab = build_panel.get_index()
			_start_build(project.get_build_resolution(), project.get_build_folder())
		Menu.WORLD:
			graph_panel.select_node("")
			inspector.show_world()
		Menu.AUTO_UPDATE:
			auto_update = not auto_update
			settings.set_value("preview", "auto_update", auto_update)
			_save_settings()
			_update_button.visible = not auto_update
			if auto_update and _preview_stale:
				_request_preview()
		Menu.UPDATE_PREVIEW:
			_request_preview()
		Menu.FORCE_CPU:
			var on := not bool(settings.get_value("compute", "force_cpu", false))
			settings.set_value("compute", "force_cpu", on)
			_save_settings()
			TerrainBuilder.set_force_cpu(on)
			_update_gpu_label()
			graph_panel.gpu_active = _gpu_state == "ready" and not on
			graph_panel.rebuild()
			_request_preview()
			_set_status("Computing on the CPU only." if on else "Computing on the GPU where nodes support it.")
		Menu.DOCS:
			OS.shell_open(REPO_URL)
		Menu.ABOUT:
			_show_message("About OpenTerrainStudio",
				"OpenTerrainStudio %s\nOpen-source node-based terrain authoring.\n\n© 2026 EllisonDigital. Dual-licensed MIT / Apache-2.0.\nNot affiliated with QuadSpinner or Gaea." % TerrainProject.get_app_version())


func _default_export_folder() -> String:
	if project_path != "":
		return project_path.get_base_dir().path_join("output")
	return OS.get_system_dir(OS.SYSTEM_DIR_DOCUMENTS).path_join("OpenTerrainStudio/output")


func _notification(what: int) -> void:
	if what == NOTIFICATION_WM_CLOSE_REQUEST:
		_confirm_discard(func(): get_tree().quit())


func _set_status(text: String) -> void:
	_status_label.text = text


func _show_message(title: String, text: String) -> void:
	_message.title = title
	_message.dialog_text = text
	_message.popup_centered()


# ---- settings and GPU status ------------------------------------------------

func _load_settings() -> void:
	settings.load(SETTINGS_PATH)
	auto_update = settings.get_value("preview", "auto_update", true)
	TerrainBuilder.set_force_cpu(settings.get_value("compute", "force_cpu", false))
	TerrainBuilder.set_builds_on_gpu(settings.get_value("compute", "builds_on_gpu", false))


func _save_settings() -> void:
	settings.save(SETTINGS_PATH)


func _update_settings_menu() -> void:
	_settings_menu.set_item_checked(_settings_menu.get_item_index(Menu.AUTO_UPDATE), auto_update)
	_settings_menu.set_item_checked(_settings_menu.get_item_index(Menu.FORCE_CPU),
			settings.get_value("compute", "force_cpu", false))
	_settings_menu.set_item_disabled(_settings_menu.get_item_index(Menu.FORCE_CPU), _gpu_state != "ready")


## The GPU starts in the background: check until it is ready (or known missing).
func _poll_gpu_status() -> void:
	while _update_gpu_label() == "starting":
		await get_tree().create_timer(0.25).timeout
	graph_panel.gpu_active = _gpu_state == "ready" and not settings.get_value("compute", "force_cpu", false)
	graph_panel.rebuild()


## Show the compute device in the status bar; returns its state.
func _update_gpu_label() -> String:
	var s: Dictionary = TerrainBuilder.get_gpu_status()
	_gpu_state = s["state"]
	match _gpu_state:
		"starting":
			_gpu_label.text = "  ·  GPU starting…"
			_gpu_label.tooltip_text = ""
		"unavailable":
			_gpu_label.text = "  ·  CPU only"
			_gpu_label.tooltip_text = "No GPU compute: %s.\nEvery node runs on the CPU, with the same results." % s.get("reason", "")
		_:
			if s["force_cpu"]:
				_gpu_label.text = "  ·  CPU (forced)"
			else:
				_gpu_label.text = "  ·  GPU"
			var tip := "GPU: %s\nNodes marked GPU in the graph run there; the rest on the CPU." % s["name"]
			if int(s["fallbacks"]) > 0:
				_gpu_label.text += " ⚠"
				tip += "\n%d GPU run(s) failed and used the CPU. Last error: %s" % [s["fallbacks"], s["last_error"]]
			_gpu_label.tooltip_text = tip
	return _gpu_state
