## 2D map view: the viewed output as an image, with the world position and
## value under the mouse. Wheel = zoom, middle or right drag = pan, F = fit.
extends Control

const MAP_SHADER := preload("res://shaders/map.gdshader")

## Emitted as the mouse moves over the map: world position (metres) and the
## text of the readout ("" when outside the map).
signal hovered(text: String)

var world_size := 8192.0
var height_min := 0.0
var height_max := 2000.0

var _preview: TerrainPreview
var _rect: TextureRect
var _material: ShaderMaterial
var _texture: ImageTexture
var _readout: Label
var _zoom := 1.0
var _pan := Vector2.ZERO
var _panning := false


func _ready() -> void:
	clip_contents = true
	mouse_filter = Control.MOUSE_FILTER_STOP
	focus_mode = Control.FOCUS_CLICK
	var bg := ColorRect.new()
	bg.color = Color(0.11, 0.11, 0.12)
	bg.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	bg.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(bg)
	_material = ShaderMaterial.new()
	_material.shader = MAP_SHADER
	_rect = TextureRect.new()
	_rect.material = _material
	_rect.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	_rect.stretch_mode = TextureRect.STRETCH_SCALE
	_rect.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_rect)
	_readout = Label.new()
	_readout.add_theme_color_override("font_shadow_color", Color(0, 0, 0, 0.9))
	_readout.add_theme_constant_override("shadow_offset_x", 1)
	_readout.add_theme_constant_override("shadow_offset_y", 1)
	_readout.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_readout)
	resized.connect(_layout)


func set_world(size_m: float, h_min: float, h_max: float) -> void:
	world_size = size_m
	height_min = h_min
	height_max = h_max
	_update_uniforms()


func set_view_mode(mode: int) -> void:
	_material.set_shader_parameter("view_mode", mode)


func show_preview(preview: TerrainPreview) -> void:
	_preview = preview
	var img: Image = preview.get_image()
	if img == null:
		return
	if _texture != null and _texture.get_width() == img.get_width() and _texture.get_format() == img.get_format():
		_texture.update(img)
	else:
		_texture = ImageTexture.create_from_image(img)
	_rect.texture = _texture
	_rect.visible = true
	_update_uniforms()
	_layout()


func clear() -> void:
	_preview = null
	_rect.visible = false
	_readout.text = ""


func fit() -> void:
	_zoom = 1.0
	_pan = Vector2.ZERO
	_layout()


func _update_uniforms() -> void:
	if _material == null:
		return
	# Points are shown as a mask of their positions.
	var is_mask := _preview != null and _preview.get_port_type() in ["mask", "point_set"]
	_material.set_shader_parameter("map_tex", _texture)
	_material.set_shader_parameter("is_mask", is_mask)
	_material.set_shader_parameter("is_color", _preview != null and _preview.get_port_type() == "color_map")
	_material.set_shader_parameter("value_min", height_min)
	_material.set_shader_parameter("value_max", height_max)
	if _preview != null:
		_material.set_shader_parameter("cell_m", world_size / maxf(_preview.get_resolution() - 1, 1))


## The map is square, fitted to the smaller side, then zoomed and panned.
func _map_rect() -> Rect2:
	var side := minf(size.x, size.y) * 0.96 * _zoom
	var pos := (size - Vector2(side, side)) * 0.5 + _pan
	return Rect2(pos, Vector2(side, side))


func _layout() -> void:
	if _rect == null:
		return
	var r := _map_rect()
	_rect.position = r.position
	_rect.size = r.size
	_readout.position = Vector2(8, size.y - 28)


## World position (metres) under a point in this control, or null if off the map.
func world_at(local: Vector2) -> Variant:
	var r := _map_rect()
	var uv := (local - r.position) / r.size
	if uv.x < 0.0 or uv.y < 0.0 or uv.x > 1.0 or uv.y > 1.0:
		return null
	# Samples sit on the world's corners: pixel i of n covers i/n..(i+1)/n of
	# the image, centred on world x = i / (n - 1) * size.
	var n := float(_preview.get_resolution()) if _preview != null else 2.0
	var f := (uv * n - Vector2(0.5, 0.5)) / (n - 1.0)
	return f.clamp(Vector2.ZERO, Vector2.ONE) * world_size


func readout_at(local: Vector2) -> String:
	if _preview == null:
		return ""
	var p: Variant = world_at(local)
	if p == null:
		return ""
	var v := _preview.sample(p.x, p.y)
	var text := "x %.0f m   y %.0f m   " % [p.x, p.y]
	if _preview.get_port_type() == "color_map":
		var c := _preview.sample_color(p.x, p.y)
		text += "colour #%s   R %.2f  G %.2f  B %.2f" % [c.to_html(false), c.r, c.g, c.b]
	elif _preview.get_port_type() == "point_set":
		text += "%d points%s" % [_preview.get_point_count(), "   (point here)" if v > 0.0 else ""]
	elif _preview.get_port_type() == "mask":
		text += "mask %.3f" % v
		var base := _preview.sample_base(p.x, p.y)
		if not is_nan(base):
			text += "   (terrain %.1f m)" % base
	else:
		text += "height %.1f m" % v
	return text


func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		var mb := event as InputEventMouseButton
		match mb.button_index:
			MOUSE_BUTTON_WHEEL_UP, MOUSE_BUTTON_WHEEL_DOWN:
				if mb.pressed:
					var factor := 1.15 if mb.button_index == MOUSE_BUTTON_WHEEL_UP else 1.0 / 1.15
					_zoom_at(mb.position, factor)
			MOUSE_BUTTON_MIDDLE, MOUSE_BUTTON_RIGHT:
				_panning = mb.pressed
		accept_event()
	elif event is InputEventMouseMotion:
		var mm := event as InputEventMouseMotion
		if _panning:
			_pan += mm.relative
			_layout()
		_readout.text = readout_at(mm.position)
		hovered.emit(_readout.text)
	elif event is InputEventKey and event.pressed and not event.echo:
		if (event as InputEventKey).keycode == KEY_F:
			fit()
			accept_event()


func _zoom_at(point: Vector2, factor: float) -> void:
	var new_zoom := clampf(_zoom * factor, 0.25, 64.0)
	factor = new_zoom / _zoom
	# Keep the point under the mouse fixed.
	var centre := size * 0.5 + _pan
	_pan += (point - centre) * (1.0 - factor)
	_zoom = new_zoom
	_layout()
