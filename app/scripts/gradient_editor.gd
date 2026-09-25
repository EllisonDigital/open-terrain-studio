## Editor for a gradient parameter: a preview bar plus one row per stop
## (position and colour). Emits `changed` with stops [[t, r, g, b], ...]
## (sRGB, 0..1) whenever a stop is added, removed, moved or recoloured.
extends VBoxContainer

signal changed(stops: Array)

var _stops: Array = [] # [[t, r, g, b], ...], sorted by t
var _bar: TextureRect
var _rows: VBoxContainer


func _init() -> void:
	size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_bar = TextureRect.new()
	_bar.custom_minimum_size = Vector2(0, 22)
	_bar.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	_bar.stretch_mode = TextureRect.STRETCH_SCALE
	add_child(_bar)
	_rows = VBoxContainer.new()
	add_child(_rows)
	var add := Button.new()
	add.text = "Add stop"
	add.tooltip_text = "Add a colour stop halfway along the widest gap."
	add.pressed.connect(_add_stop)
	add_child(add)


func set_stops(stops: Array) -> void:
	_stops = []
	for s in stops:
		_stops.append([float(s[0]), float(s[1]), float(s[2]), float(s[3])])
	_sort()
	_rebuild()


func get_stops() -> Array:
	return _stops.duplicate(true)


func _sort() -> void:
	_stops.sort_custom(func(a, b): return a[0] < b[0])


func _emit() -> void:
	_sort()
	_update_bar()
	changed.emit(get_stops())


func _update_bar() -> void:
	var g := Gradient.new()
	g.offsets = PackedFloat32Array()
	g.colors = PackedColorArray()
	for s in _stops:
		g.add_point(s[0], Color(s[1], s[2], s[3]))
	var tex := GradientTexture1D.new()
	tex.gradient = g
	tex.width = 256
	_bar.texture = tex


func _rebuild() -> void:
	for c in _rows.get_children():
		_rows.remove_child(c)
		c.queue_free()
	for i in _stops.size():
		var stop: Array = _stops[i]
		var row := HBoxContainer.new()
		var pos := SpinBox.new()
		pos.min_value = 0.0
		pos.max_value = 1.0
		pos.step = 0.01
		pos.value = stop[0]
		pos.tooltip_text = "Position along the gradient (0 = left, 1 = right)."
		pos.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		pos.value_changed.connect(func(v):
			stop[0] = v
			_emit())
		row.add_child(pos)
		var pick := ColorPickerButton.new()
		pick.color = Color(stop[1], stop[2], stop[3])
		pick.edit_alpha = false
		pick.custom_minimum_size = Vector2(48, 0)
		pick.color_changed.connect(func(c: Color):
			stop[1] = c.r
			stop[2] = c.g
			stop[3] = c.b
			_emit())
		row.add_child(pick)
		var remove := Button.new()
		remove.text = "×"
		remove.tooltip_text = "Remove this stop."
		remove.disabled = _stops.size() <= 1
		remove.pressed.connect(func():
			_stops.erase(stop)
			_rebuild()
			_emit())
		row.add_child(remove)
		_rows.add_child(row)
	_update_bar()


func _add_stop() -> void:
	if _stops.is_empty():
		_stops.append([0.5, 0.5, 0.5, 0.5])
	else:
		# Middle of the widest gap (including the ends).
		var edges := [0.0]
		for s in _stops:
			edges.append(s[0])
		edges.append(1.0)
		var best := 0
		for k in edges.size() - 1:
			if edges[k + 1] - edges[k] > edges[best + 1] - edges[best]:
				best = k
		var t: float = (edges[best] + edges[best + 1]) * 0.5
		var g := Gradient.new()
		g.offsets = PackedFloat32Array()
		g.colors = PackedColorArray()
		for s in _stops:
			g.add_point(s[0], Color(s[1], s[2], s[3]))
		var c := g.sample(t)
		_stops.append([t, c.r, c.g, c.b])
	_sort()
	_rebuild()
	_emit()
