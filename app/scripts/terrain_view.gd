## 3D terrain viewport: shader-displaced grid, sun, sky and an orbit camera.
##
## Controls: left or right drag = orbit, middle drag (or Shift + drag) = pan,
## wheel = zoom, F = frame the whole terrain.
extends SubViewportContainer

const TERRAIN_SHADER := preload("res://shaders/terrain.gdshader")
const WATER_SHADER := preload("res://shaders/water.gdshader")
## Mesh vertices per side are capped; the fragment shader samples the full
## heightmap for lighting, so detail survives at higher preview resolutions.
const MAX_MESH_RES := 1024

var world_size := 8192.0
var height_min := 0.0
var height_max := 2000.0

var _viewport: SubViewport
var _env: Environment
var _camera: Camera3D
var _sun: DirectionalLight3D
var _terrain: MeshInstance3D
var _material: ShaderMaterial
var _texture: ImageTexture
var _overlay: ImageTexture
var _water: MeshInstance3D
var _water_material: ShaderMaterial
var _water_texture: ImageTexture
var _snow_texture: ImageTexture
var _color_texture: ImageTexture
var _mesh_res := 0
var _exaggeration := 1.0

# Orbit camera state.
var yaw := deg_to_rad(-35.0)
var pitch := deg_to_rad(-32.0)
var distance := 8000.0
var target := Vector3.ZERO
var _dragging := false
var _panning := false


func _ready() -> void:
	stretch = true
	mouse_filter = Control.MOUSE_FILTER_STOP
	focus_mode = Control.FOCUS_CLICK

	_viewport = SubViewport.new()
	_viewport.own_world_3d = true
	_viewport.msaa_3d = Viewport.MSAA_2X
	_viewport.positional_shadow_atlas_size = 0
	add_child(_viewport)

	var env := Environment.new()
	_env = env
	var sky_mat := ProceduralSkyMaterial.new()
	sky_mat.sky_top_color = Color(0.32, 0.50, 0.78)
	sky_mat.sky_horizon_color = Color(0.72, 0.78, 0.86)
	sky_mat.ground_horizon_color = Color(0.60, 0.62, 0.64)
	sky_mat.ground_bottom_color = Color(0.25, 0.25, 0.27)
	var sky := Sky.new()
	sky.sky_material = sky_mat
	env.background_mode = Environment.BG_SKY
	env.sky = sky
	env.ambient_light_source = Environment.AMBIENT_SOURCE_SKY
	env.ambient_light_energy = 0.45
	env.ambient_light_sky_contribution = 0.55
	env.ambient_light_color = Color(0.55, 0.55, 0.55)
	env.tonemap_mode = Environment.TONE_MAPPER_FILMIC
	env.fog_enabled = true
	env.fog_light_color = Color(0.70, 0.76, 0.84)
	env.fog_sky_affect = 0.0
	var world_env := WorldEnvironment.new()
	world_env.environment = env
	_viewport.add_child(world_env)

	_sun = DirectionalLight3D.new()
	_sun.rotation_degrees = Vector3(-38.0, -130.0, 0.0)
	_sun.light_energy = 1.6
	_sun.light_color = Color(1.0, 0.96, 0.90)
	_sun.shadow_enabled = true
	_sun.directional_shadow_mode = DirectionalLight3D.SHADOW_PARALLEL_4_SPLITS
	_viewport.add_child(_sun)

	_material = ShaderMaterial.new()
	_material.shader = TERRAIN_SHADER
	_terrain = MeshInstance3D.new()
	_terrain.material_override = _material
	_terrain.visible = false
	_viewport.add_child(_terrain)

	_water_material = ShaderMaterial.new()
	_water_material.shader = WATER_SHADER
	_water = MeshInstance3D.new()
	_water.material_override = _water_material
	_water.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	_water.visible = false
	_viewport.add_child(_water)

	_camera = Camera3D.new()
	_camera.near = 1.0
	_viewport.add_child(_camera)

	set_world(world_size, height_min, height_max)
	frame_all()


## Update the world dimensions (metres). Keeps the current heightmap.
func set_world(size_m: float, h_min: float, h_max: float) -> void:
	world_size = size_m
	height_min = h_min
	height_max = h_max
	_material.set_shader_parameter("world_size", world_size)
	_material.set_shader_parameter("height_min", height_min)
	_material.set_shader_parameter("height_max", height_max)
	_water_material.set_shader_parameter("world_size", world_size)
	_camera.far = world_size * 20.0
	# Light aerial haze, scaled so every world size looks the same.
	_env.fog_density = 0.12 / world_size
	_sun.directional_shadow_max_distance = world_size * 3.0
	if _mesh_res > 0:
		_build_mesh(_mesh_res)
	_update_camera()


## Show a TerrainPreview. Heightfields are shown as terrain. Masks are drawn
## as a false-colour overlay on the terrain they were computed from; a mask
## with no terrain upstream is shown as relief over the world height range.
## Colour maps are painted on the terrain they were computed from (flat
## ground if there is none).
func show_preview(preview: TerrainPreview) -> void:
	var img: Image = preview.get_image()
	if img == null:
		return
	var type := preview.get_port_type()
	var is_mask := type == "mask"
	var is_color := type == "color_map"
	var is_height := not is_mask and not is_color
	var base: Image = null if is_height else preview.get_base_image()
	var height_img: Image = base if base != null else img
	if is_color and base == null:
		height_img = Image.create(img.get_width(), img.get_height(), false, Image.FORMAT_RF)
	_texture = _update_texture(_texture, height_img)
	_material.set_shader_parameter("height_tex", _texture)
	if not is_height and base == null:
		# A mask as relief over the height range; a colour map flat at the bottom.
		_material.set_shader_parameter("value_scale", 0.0 if is_color else height_max - height_min)
		_material.set_shader_parameter("value_offset", height_min)
	else:
		_material.set_shader_parameter("value_scale", 1.0)
		_material.set_shader_parameter("value_offset", 0.0)
	_material.set_shader_parameter("show_overlay", is_mask)
	if is_mask:
		_overlay = _update_texture(_overlay, img)
		_material.set_shader_parameter("overlay_tex", _overlay)
	_material.set_shader_parameter("show_color", is_color)
	if is_color:
		_color_texture = _update_texture(_color_texture, img)
		_material.set_shader_parameter("color_tex", _color_texture)
	var res := mini(img.get_width(), MAX_MESH_RES)
	if res != _mesh_res:
		_build_mesh(res)
	_terrain.visible = true
	_show_water(preview.get_water_image() if is_height else null)
	_show_snow(preview.get_snow_image() if is_height else null)


## Snow cover from Snow nodes on the terrain shown (null: none).
func _show_snow(cover: Image) -> void:
	if cover != null:
		_snow_texture = _update_texture(_snow_texture, cover)
		_material.set_shader_parameter("snow_tex", _snow_texture)
	_material.set_shader_parameter("show_snow", cover != null)


## Water from Rivers, Lakes and Sea over the terrain shown (null: none).
func _show_water(level: Image) -> void:
	if level == null:
		_water.visible = false
		return
	_water_texture = _update_texture(_water_texture, level)
	_water_material.set_shader_parameter("water_tex", _water_texture)
	_water_material.set_shader_parameter("height_tex", _texture)
	_water.visible = true


func _update_texture(tex: ImageTexture, img: Image) -> ImageTexture:
	if tex != null and tex.get_width() == img.get_width() and tex.get_format() == img.get_format():
		tex.update(img)
		return tex
	return ImageTexture.create_from_image(img)


func clear() -> void:
	_terrain.visible = false
	_water.visible = false


func set_view_mode(mode: int) -> void:
	_material.set_shader_parameter("view_mode", mode)


func set_exaggeration(value: float) -> void:
	_exaggeration = value
	_material.set_shader_parameter("exaggeration", value)
	_water_material.set_shader_parameter("exaggeration", value)
	_terrain.set_custom_aabb(_terrain_aabb(value))
	_water.set_custom_aabb(_terrain_aabb(value))


func set_sun_angle(azimuth_deg: float, elevation_deg: float) -> void:
	_sun.rotation_degrees = Vector3(-elevation_deg, azimuth_deg, 0.0)


func frame_all() -> void:
	target = Vector3(0.0, (height_min + height_max) * 0.2, 0.0)
	distance = world_size * 0.95
	yaw = deg_to_rad(-35.0)
	pitch = deg_to_rad(-32.0)
	_update_camera()


func get_camera_state() -> Dictionary:
	return {"yaw": yaw, "pitch": pitch, "distance": distance, "target": [target.x, target.y, target.z]}


func set_camera_state(state: Dictionary) -> void:
	if state.is_empty():
		return
	yaw = float(state.get("yaw", yaw))
	pitch = float(state.get("pitch", pitch))
	distance = float(state.get("distance", distance))
	var t: Array = state.get("target", [])
	if t.size() == 3:
		target = Vector3(t[0], t[1], t[2])
	_update_camera()


func _build_mesh(res: int) -> void:
	# `res` vertices per side, exactly on the heightmap samples.
	var plane := PlaneMesh.new()
	plane.size = Vector2(world_size, world_size)
	plane.subdivide_width = res - 2
	plane.subdivide_depth = res - 2
	_terrain.mesh = plane
	_terrain.set_custom_aabb(_terrain_aabb(_exaggeration))
	_water.mesh = plane
	_water.set_custom_aabb(_terrain_aabb(_exaggeration))
	_mesh_res = res


func _terrain_aabb(exaggeration: float) -> AABB:
	# Vertex displacement happens on the GPU, so tell culling how tall we can get.
	var lo := minf(height_min, 0.0) * exaggeration - 100.0
	var hi := maxf(height_max, 0.0) * exaggeration * 4.0 + 100.0
	return AABB(Vector3(-world_size * 0.5, lo, -world_size * 0.5), Vector3(world_size, hi - lo, world_size))


func _update_camera() -> void:
	if _camera == null:
		return
	pitch = clampf(pitch, deg_to_rad(-89.0), deg_to_rad(-2.0))
	distance = clampf(distance, 10.0, world_size * 8.0)
	var dir := Vector3(cos(pitch) * sin(yaw), -sin(pitch), cos(pitch) * cos(yaw))
	_camera.position = target + dir * distance
	_camera.look_at(target, Vector3.UP)


func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton:
		var mb := event as InputEventMouseButton
		match mb.button_index:
			MOUSE_BUTTON_LEFT, MOUSE_BUTTON_RIGHT:
				_dragging = mb.pressed
				_panning = mb.pressed and mb.shift_pressed
			MOUSE_BUTTON_MIDDLE:
				_panning = mb.pressed
				_dragging = false
			MOUSE_BUTTON_WHEEL_UP:
				if mb.pressed:
					distance *= 0.88
					_update_camera()
			MOUSE_BUTTON_WHEEL_DOWN:
				if mb.pressed:
					distance /= 0.88
					_update_camera()
		accept_event()
	elif event is InputEventMouseMotion:
		var mm := event as InputEventMouseMotion
		if _panning:
			var basis := _camera.global_transform.basis
			var scale := distance * 0.0015
			var right := Vector3(basis.x.x, 0.0, basis.x.z).normalized()
			var forward := Vector3(basis.z.x, 0.0, basis.z.z).normalized()
			target -= (right * mm.relative.x + forward * mm.relative.y) * scale
			_update_camera()
		elif _dragging:
			yaw -= mm.relative.x * 0.006
			pitch -= mm.relative.y * 0.006
			_update_camera()
	elif event is InputEventKey and event.pressed and not event.echo:
		if (event as InputEventKey).keycode == KEY_F:
			frame_all()
			accept_event()
