## Property inspector. Built entirely from node schemas, so a new node type
## needs no UI code. Shows world settings when no node is selected.
extends ScrollContainer

const CurveEditor := preload("res://scripts/curve_editor.gd")
const GradientEditor := preload("res://scripts/gradient_editor.gd")

signal param_changed(node_id: String, key: String, value: Variant)
signal port_toggled(node_id: String, key: String, exposed: bool)
signal export_toggled(node_id: String, port: String, format: String, on: bool)
signal send_to_colour(node_id: String, port: String)
signal world_changed
## A species preset (YAML text) was chosen for a vegetation node.
signal species_preset_chosen(node_id: String, text: String)

const EXPORT_FORMATS := [
	["exr32", "EXR", "32-bit float EXR: heights in metres; colours RGBA"],
	["png16", "PNG 16", "16-bit PNG: heights over the world height range; colours RGBA"],
	["png8", "PNG 8", "8-bit PNG, e.g. colour and splat maps for engines"],
]
const POINT_EXPORT_FORMATS := [
	["csv", "CSV", "Points as CSV: x, y, z (metres), rotation_deg, scale, species"],
	["json", "JSON", "Points as JSON, same columns as the CSV"],
]
## Species presets bundled with the app (YAML, see docs/vegetation.md).
const SPECIES_DIR := "res://examples/species"

## Bundled presets, read once: [{path, name, node, biome, description}].
static var _species_presets: Array = []

var project: TerrainProject
var _box: VBoxContainer
var _node_id := ""
var _controls := {} # param key -> control (for pushing back clamped values)
var _file_dialog: FileDialog
var _file_target: LineEdit
var _preset_open_dialog: FileDialog
var _preset_save_dialog: FileDialog


func _ready() -> void:
	horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	custom_minimum_size = Vector2(320, 0)
	var margin := MarginContainer.new()
	margin.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	for side in ["left", "right", "top", "bottom"]:
		margin.add_theme_constant_override("margin_" + side, 10)
	add_child(margin)
	_box = VBoxContainer.new()
	_box.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_box.add_theme_constant_override("separation", 8)
	margin.add_child(_box)

	_file_dialog = FileDialog.new()
	_file_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_file_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_file_dialog.use_native_dialog = true
	_file_dialog.file_selected.connect(_on_file_chosen)
	add_child(_file_dialog)

	_preset_open_dialog = FileDialog.new()
	_preset_open_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	_preset_open_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_preset_open_dialog.use_native_dialog = true
	_preset_open_dialog.filters = PackedStringArray(["*.yaml, *.yml ; Species preset"])
	_preset_open_dialog.file_selected.connect(func(path):
		species_preset_chosen.emit(_node_id, FileAccess.get_file_as_string(path)))
	add_child(_preset_open_dialog)
	_preset_save_dialog = FileDialog.new()
	_preset_save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	_preset_save_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_preset_save_dialog.use_native_dialog = true
	_preset_save_dialog.filters = _preset_open_dialog.filters
	_preset_save_dialog.file_selected.connect(_save_species_preset)
	add_child(_preset_save_dialog)


## Id of the node being shown, or "" for world settings.
func get_node_id() -> String:
	return _node_id


func _clear() -> void:
	for c in _box.get_children():
		_box.remove_child(c)
		c.queue_free()
	_controls.clear()


func _heading(text: String) -> void:
	var l := Label.new()
	l.text = text
	l.add_theme_font_size_override("font_size", 18)
	_box.add_child(l)


func _subheading(text: String) -> void:
	var l := Label.new()
	l.text = text
	l.add_theme_font_size_override("font_size", 15)
	_box.add_child(l)


func _note(text: String) -> Label:
	var l := Label.new()
	l.text = text
	l.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	l.modulate = Color(1, 1, 1, 0.65)
	# A fixed minimum width stops wrapped labels measuring one word per line.
	l.custom_minimum_size = Vector2(280, 0)
	_box.add_child(l)
	return l


## A labelled row. `extra` (optional) sits at the right of the label line.
func _row(label: String, control: Control, tooltip := "", extra: Control = null) -> void:
	var v := VBoxContainer.new()
	v.add_theme_constant_override("separation", 2)
	var head := HBoxContainer.new()
	var l := Label.new()
	l.text = label
	l.tooltip_text = tooltip
	l.mouse_filter = Control.MOUSE_FILTER_PASS
	l.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	head.add_child(l)
	if extra != null:
		head.add_child(extra)
	control.tooltip_text = tooltip
	control.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	v.add_child(head)
	v.add_child(control)
	_box.add_child(v)


## Number field; with a slider as well when the range is small enough to drag.
func _number(value: float, min_v: float, max_v: float, step: float, unit: String, on_change: Callable) -> Control:
	var h := HBoxContainer.new()
	var spin := SpinBox.new()
	spin.min_value = min_v
	spin.max_value = max_v
	spin.step = step
	spin.value = value
	spin.suffix = unit
	spin.select_all_on_focus = true
	spin.custom_minimum_size.x = 110
	if max_v - min_v <= 20.0:
		var slider := HSlider.new()
		slider.min_value = min_v
		slider.max_value = max_v
		slider.step = step
		slider.value = value
		slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		slider.size_flags_vertical = Control.SIZE_SHRINK_CENTER
		slider.share(spin)
		# A new drag is a new undo step.
		slider.drag_started.connect(func(): project.break_undo_merge())
		h.add_child(slider)
	else:
		spin.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	spin.value_changed.connect(on_change)
	h.add_child(spin)
	h.set_meta("spin", spin)
	return h


# ---- world settings -------------------------------------------------------

func show_world() -> void:
	_clear()
	_node_id = ""
	_heading("World")
	_note("Applies to every node. Sizes are in metres, so the same graph gives the same landscape at any resolution.")
	_row("World size", _number(project.get_world_size(), 1.0, 1_000_000.0, 1.0, " m",
			func(v): project.set_world_size(v); world_changed.emit()),
			"Width and depth of the terrain, in metres.")
	var hmin := _number(project.get_height_min(), -10000.0, 20000.0, 1.0, " m", func(_v): pass)
	var hmax := _number(project.get_height_max(), -10000.0, 20000.0, 1.0, " m", func(_v): pass)
	var apply_range := func(_v):
		var lo: float = hmin.get_meta("spin").value
		var hi: float = hmax.get_meta("spin").value
		if project.set_height_range(lo, hi):
			world_changed.emit()
	(hmin.get_meta("spin") as SpinBox).value_changed.connect(apply_range)
	(hmax.get_meta("spin") as SpinBox).value_changed.connect(apply_range)
	_row("Lowest height", hmin, "Bottom of the height range. 16-bit exports map this to black.")
	_row("Highest height", hmax, "Top of the height range. 16-bit exports map this to white.")
	_row("Project seed", _seed_control(project.get_seed(), func(v): project.set_seed(int(v)); world_changed.emit()),
			"Changes every node's randomness at once.")


# ---- node parameters ------------------------------------------------------

func show_node(node: Dictionary, schema: Dictionary) -> void:
	_clear()
	_node_id = node["id"]
	_heading(node["label"])
	var id_label := Label.new()
	id_label.text = "%s · %s" % [node["type"], node["id"]]
	id_label.modulate = Color(1, 1, 1, 0.45)
	_box.add_child(id_label)
	if not node["known"]:
		_note("This node type is not available in this version of OpenTerrainStudio. Its settings are kept unchanged.")
		return
	if schema.get("description", "") != "":
		_note(schema["description"])
	_box.add_child(HSeparator.new())
	if node.get("category", "") == "Vegetation":
		_add_preset_section(node)
	var values: Dictionary = node["params"]
	var exposed := Array(node.get("exposed", PackedStringArray()))
	for p in schema["params"]:
		_add_param(p, values.get(p["key"], p["default"]), exposed.has(p["key"]))
	_add_export_section(node)


func refresh_value(key: String, value: Variant) -> void:
	var c: Control = _controls.get(key)
	if c == null:
		return
	if c.has_meta("spin"):
		(c.get_meta("spin") as SpinBox).set_value_no_signal(value)
	elif c is CheckBox:
		(c as CheckBox).set_pressed_no_signal(value)


func _add_param(p: Dictionary, value: Variant, exposed: bool) -> void:
	var key: String = p["key"]
	var id := _node_id
	var unit: String = p["unit"]
	var suffix := (" " + unit) if unit != "" else ""
	var control: Control
	match p["kind"]:
		"float":
			control = _number(value, p["min"], p["max"], p["step"], suffix,
					func(v): param_changed.emit(id, key, float(v)))
		"int":
			if key == "seed":
				control = _seed_control(value, func(v): param_changed.emit(id, key, int(v)))
			else:
				control = _number(value, p["min"], p["max"], 1.0, suffix,
						func(v): param_changed.emit(id, key, int(v)))
		"bool":
			var cb := CheckBox.new()
			cb.text = "On"
			cb.button_pressed = value
			cb.toggled.connect(func(on): param_changed.emit(id, key, on))
			control = cb
		"enum":
			var ob := OptionButton.new()
			var options: Array = p["options"]
			for i in options.size():
				ob.add_item(options[i][1], i)
				if options[i][0] == value:
					ob.select(i)
			ob.item_selected.connect(func(i): param_changed.emit(id, key, options[i][0]))
			control = ob
		"curve":
			var ce := CurveEditor.new()
			var pts := PackedVector2Array()
			for pt in value:
				pts.append(Vector2(pt[0], pt[1]))
			ce.set_points(pts)
			ce.changed.connect(func(points): param_changed.emit(id, key, points))
			control = ce
		"gradient":
			var ge := GradientEditor.new()
			ge.set_stops(value)
			ge.changed.connect(func(stops): param_changed.emit(id, key, stops))
			control = ge
		"file":
			control = _file_control(value, p.get("filters", []), func(path): param_changed.emit(id, key, path))
		"text":
			var edit := LineEdit.new()
			edit.text = value
			edit.text_submitted.connect(func(t): param_changed.emit(id, key, t))
			edit.focus_exited.connect(func():
				if edit.text != value:
					param_changed.emit(id, key, edit.text))
			control = edit
		_:
			return
	_controls[key] = control
	var port_button: Button = null
	if p.get("drivable", false):
		port_button = Button.new()
		port_button.text = "Mask port"
		port_button.toggle_mode = true
		port_button.button_pressed = exposed
		port_button.flat = not exposed
		port_button.focus_mode = Control.FOCUS_NONE
		port_button.tooltip_text = "Drive this value with a mask: adds an input port to the node.\nThe mask scales the value: black = 0, white = the value set here."
		port_button.toggled.connect(func(on): port_toggled.emit(id, key, on))
	_row(p["label"], control, p["description"], port_button)
	if exposed:
		var n := _note("Scaled by the mask connected to the node's %s port." % p["label"])
		n.add_theme_color_override("font_color", Color(0.55, 0.75, 0.95))


func _file_control(value: String, filters: Array, on_change: Callable) -> Control:
	var h := HBoxContainer.new()
	var edit := LineEdit.new()
	edit.text = value
	edit.placeholder_text = "Choose a file…"
	edit.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	edit.text_submitted.connect(func(t): on_change.call(t))
	edit.focus_exited.connect(func():
		if edit.text != value:
			on_change.call(edit.text))
	edit.set_meta("on_change", on_change)
	h.add_child(edit)
	var browse := Button.new()
	browse.text = "Browse…"
	browse.pressed.connect(func():
		_file_target = edit
		_file_dialog.filters = PackedStringArray(filters)
		var dir := project.get_project_dir()
		if dir != "":
			_file_dialog.current_dir = dir
		_file_dialog.popup_centered_ratio(0.6))
	h.add_child(browse)
	return h


func _on_file_chosen(path: String) -> void:
	if _file_target == null or not is_instance_valid(_file_target):
		return
	# Files inside the project's folder are stored relative to it, so the
	# project folder can be moved or shared.
	var dir := project.get_project_dir()
	if dir != "" and path.begins_with(dir.path_join("")):
		path = path.substr(dir.path_join("").length())
	_file_target.text = path
	(_file_target.get_meta("on_change") as Callable).call(path)


func _add_export_section(node: Dictionary) -> void:
	var outputs: Array = node["outputs"]
	if outputs.is_empty():
		return
	_box.add_child(HSeparator.new())
	_subheading("Export")
	_note("Marked outputs are written by Build (Build tab), at the build resolution.")
	for o in outputs:
		var formats := project.get_export_formats(node["id"], o["key"])
		var h := HBoxContainer.new()
		if outputs.size() > 1:
			var l := Label.new()
			l.text = o["label"]
			h.add_child(l)
		for f in (POINT_EXPORT_FORMATS if o["type"] == "point_set" else EXPORT_FORMATS):
			var cb := CheckBox.new()
			cb.text = f[1]
			cb.tooltip_text = f[2]
			cb.button_pressed = formats.has(f[0])
			var id: String = node["id"]
			var port: String = o["key"]
			var format: String = f[0]
			cb.toggled.connect(func(on): export_toggled.emit(id, port, format, on))
			h.add_child(cb)
		_box.add_child(h)
	_add_colour_section(node)


## Terrain nodes: buttons that bring an output into the Colour tab.
func _add_colour_section(node: Dictionary) -> void:
	if node.get("tab", "terrain") != "terrain":
		return
	var sendable: Array = node["outputs"].filter(func(o): return o["type"] in ["heightfield", "mask"])
	if sendable.is_empty():
		return
	_box.add_child(HSeparator.new())
	_subheading("Colour tab")
	_note("Colour the terrain in the Colour tab: send an output there as a portal.")
	for o in sendable:
		var b := Button.new()
		b.text = "Send %s to Colour tab" % o["label"] if sendable.size() > 1 else "Send to Colour tab"
		b.tooltip_text = "Adds a portal in the Colour tab that brings this output there."
		var id: String = node["id"]
		var port: String = o["key"]
		b.pressed.connect(func(): send_to_colour.emit(id, port))
		_box.add_child(b)


# ---- species presets --------------------------------------------------------

## Bundled species presets, sorted by biome then name.
static func species_presets() -> Array:
	if _species_presets.is_empty():
		for file in DirAccess.get_files_at(SPECIES_DIR):
			if file.get_extension() not in ["yaml", "yml"]:
				continue
			var path := SPECIES_DIR.path_join(file)
			var info: Dictionary = TerrainGraph.read_species_preset(FileAccess.get_file_as_string(path))
			if info["ok"]:
				info["path"] = path
				_species_presets.append(info)
		_species_presets.sort_custom(func(a, b):
			return [a["biome"], a["name"]] < [b["biome"], b["name"]])
	return _species_presets


## Vegetation nodes: pick a species preset, load one from a file, or save one.
func _add_preset_section(node: Dictionary) -> void:
	var id: String = node["id"]
	var ob := OptionButton.new()
	ob.add_item("Choose a preset…")
	ob.set_item_disabled(0, true)
	var matching := species_presets().filter(func(p): return p["node"] == node["type"])
	for p in matching:
		ob.add_item("%s  (%s)" % [p["name"], p["biome"]])
		ob.set_item_tooltip(ob.item_count - 1, p["description"])
		ob.set_item_metadata(ob.item_count - 1, p["path"])
	ob.select(0)
	ob.item_selected.connect(func(i):
		var path: String = ob.get_item_metadata(i)
		species_preset_chosen.emit(id, FileAccess.get_file_as_string(path)))
	var buttons := HBoxContainer.new()
	var load_button := Button.new()
	load_button.text = "Load…"
	load_button.tooltip_text = "Apply a species preset file (.yaml)"
	load_button.pressed.connect(func(): _preset_open_dialog.popup_centered_ratio(0.6))
	buttons.add_child(load_button)
	var save_button := Button.new()
	save_button.text = "Save…"
	save_button.tooltip_text = "Save these settings as a species preset file (.yaml)"
	save_button.pressed.connect(func(): _preset_save_dialog.popup_centered_ratio(0.6))
	buttons.add_child(save_button)
	_row("Species preset", ob,
			"Typical settings for a species. Applying one changes the settings below (undoable).", buttons)
	if matching.is_empty():
		_note("No bundled presets for this node.")


func _save_species_preset(path: String) -> void:
	if path.get_extension() not in ["yaml", "yml"]:
		path += ".yaml"
	var name := path.get_file().get_basename().replace("_", " ").capitalize()
	var yaml: String = project.get_graph().make_species_preset(_node_id, name, "")
	var f := FileAccess.open(path, FileAccess.WRITE)
	if f == null or yaml == "":
		push_warning("Could not save species preset %s" % path)
		return
	f.store_string(yaml)


func _seed_control(value: int, on_change: Callable) -> Control:
	var h := _number(value, 0, 999_999, 1.0, "", on_change)
	var dice := Button.new()
	dice.text = "Random"
	dice.tooltip_text = "Pick a random seed"
	var spin: SpinBox = h.get_meta("spin")
	dice.pressed.connect(func(): spin.value = randi_range(0, 999_999))
	h.add_child(dice)
	return h
