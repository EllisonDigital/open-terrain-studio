## Property inspector. Built entirely from node schemas, so a new node type
## needs no UI code. Shows world settings when no node is selected.
extends ScrollContainer

signal param_changed(node_id: String, key: String, value: Variant)
signal world_changed

var project: TerrainProject
var _box: VBoxContainer
var _node_id := ""
var _controls := {} # param key -> control (for pushing back clamped values)


func _ready() -> void:
	horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	custom_minimum_size = Vector2(300, 0)
	var margin := MarginContainer.new()
	margin.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	for side in ["left", "right", "top", "bottom"]:
		margin.add_theme_constant_override("margin_" + side, 10)
	add_child(margin)
	_box = VBoxContainer.new()
	_box.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_box.add_theme_constant_override("separation", 8)
	margin.add_child(_box)


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


func _note(text: String) -> void:
	var l := Label.new()
	l.text = text
	l.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	l.modulate = Color(1, 1, 1, 0.65)
	_box.add_child(l)


func _row(label: String, control: Control, tooltip := "") -> void:
	var v := VBoxContainer.new()
	v.add_theme_constant_override("separation", 2)
	var l := Label.new()
	l.text = label
	l.tooltip_text = tooltip
	l.mouse_filter = Control.MOUSE_FILTER_PASS
	control.tooltip_text = tooltip
	control.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	v.add_child(l)
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
	var values: Dictionary = node["params"]
	for p in schema["params"]:
		_add_param(p, values.get(p["key"], p["default"]))


func refresh_value(key: String, value: Variant) -> void:
	var c: Control = _controls.get(key)
	if c == null:
		return
	if c.has_meta("spin"):
		(c.get_meta("spin") as SpinBox).set_value_no_signal(value)
	elif c is CheckBox:
		(c as CheckBox).set_pressed_no_signal(value)


func _add_param(p: Dictionary, value: Variant) -> void:
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
		_:
			return
	_controls[key] = control
	_row(p["label"], control, p["description"])


func _seed_control(value: int, on_change: Callable) -> Control:
	var h := _number(value, 0, 999_999, 1.0, "", on_change)
	var dice := Button.new()
	dice.text = "Random"
	dice.tooltip_text = "Pick a random seed"
	var spin: SpinBox = h.get_meta("spin")
	dice.pressed.connect(func(): spin.value = randi_range(0, 999_999))
	h.add_child(dice)
	return h
