## Editor for Curve parameters: drag points, click empty space to add one,
## right-click a point to remove it. The curve is drawn with the engine's own
## interpolation (TerrainGraph.eval_curve), so what you see is what you get.
extends Control

signal changed(points: PackedVector2Array)   ## while dragging and on release

const HANDLE := 6.0
const PAD := 8.0

var points := PackedVector2Array([Vector2(0, 0), Vector2(1, 1)])
var _drag := -1
var _hover := -1
var _samples := PackedFloat32Array()


func _ready() -> void:
	custom_minimum_size = Vector2(0, 200)
	mouse_filter = Control.MOUSE_FILTER_STOP
	tooltip_text = "Drag points to reshape. Click to add a point, right-click to remove one.\nLeft = lowest world height, right = highest."
	_resample()


func set_points(p: PackedVector2Array) -> void:
	points = p
	_resample()


func _resample() -> void:
	_samples = TerrainGraph.eval_curve(points, 96)
	queue_redraw()


func _area() -> Rect2:
	return Rect2(Vector2(PAD, PAD), size - Vector2(PAD, PAD) * 2.0)


func _to_screen(p: Vector2) -> Vector2:
	var r := _area()
	return r.position + Vector2(p.x * r.size.x, (1.0 - p.y) * r.size.y)


func _to_curve(s: Vector2) -> Vector2:
	var r := _area()
	var p := (s - r.position) / r.size
	return Vector2(clampf(p.x, 0.0, 1.0), clampf(1.0 - p.y, 0.0, 1.0))


func _point_at(s: Vector2) -> int:
	for i in points.size():
		if _to_screen(points[i]).distance_to(s) <= HANDLE * 1.8:
			return i
	return -1


func _draw() -> void:
	var r := _area()
	draw_rect(r, Color(0.08, 0.08, 0.09))
	for k in range(1, 4):
		var t := k / 4.0
		var c := Color(1, 1, 1, 0.07)
		draw_line(r.position + Vector2(r.size.x * t, 0), r.position + Vector2(r.size.x * t, r.size.y), c)
		draw_line(r.position + Vector2(0, r.size.y * t), r.position + Vector2(r.size.x, r.size.y * t), c)
	draw_line(_to_screen(Vector2(0, 0)), _to_screen(Vector2(1, 1)), Color(1, 1, 1, 0.12))
	if _samples.size() > 1:
		var line := PackedVector2Array()
		for i in _samples.size():
			line.append(_to_screen(Vector2(float(i) / (_samples.size() - 1), _samples[i])))
		draw_polyline(line, Color(0.95, 0.72, 0.30), 2.0, true)
	for i in points.size():
		var col := Color(1, 1, 1) if i == _hover or i == _drag else Color(0.85, 0.85, 0.85)
		draw_circle(_to_screen(points[i]), HANDLE, col)
		draw_circle(_to_screen(points[i]), HANDLE - 2.0, Color(0.15, 0.15, 0.16))
	draw_rect(r, Color(1, 1, 1, 0.2), false)


func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		var mb := event as InputEventMouseButton
		if mb.button_index == MOUSE_BUTTON_LEFT:
			if mb.pressed:
				_drag = _point_at(mb.position)
				if _drag < 0:
					points.append(_to_curve(mb.position))
					_drag = points.size() - 1
					_emit()
			elif _drag >= 0:
				_drag = -1
				_sort_and_emit()
			accept_event()
		elif mb.button_index == MOUSE_BUTTON_RIGHT and mb.pressed:
			var i := _point_at(mb.position)
			if i >= 0 and points.size() > 2:
				points.remove_at(i)
				_sort_and_emit()
			accept_event()
	elif event is InputEventMouseMotion:
		var mm := event as InputEventMouseMotion
		if _drag >= 0:
			points[_drag] = _to_curve(mm.position)
			_emit()
		else:
			var h := _point_at(mm.position)
			if h != _hover:
				_hover = h
				queue_redraw()


func _emit() -> void:
	_resample()
	changed.emit(points)


## Keep points ordered by x (the engine sorts them too) so indices stay stable.
func _sort_and_emit() -> void:
	var arr := Array(points)
	arr.sort_custom(func(a, b): return a.x < b.x)
	points = PackedVector2Array(arr)
	_emit()
