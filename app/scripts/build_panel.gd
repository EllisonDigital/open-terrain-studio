## Build tab: every output marked for export, the build resolution and
## folder, and the Build button. Mark outputs in a node's settings (Export
## section) or with Build > Mark Viewed Output.
extends ScrollContainer

signal build_requested(resolution: int, folder: String)
signal view_requested(node_id: String)
signal export_toggled(node_id: String, port: String, format: String, on: bool)
signal builds_on_gpu_toggled(on: bool)

const RESOLUTIONS := [512, 1009, 1024, 2017, 2048, 4033, 4096, 8129, 8192, 16129, 16384]
const UNREAL_SIZES := [1009, 2017, 4033, 8129, 16129]
## Builds over this many samples per side are computed in tiles.
const TILED_ABOVE := 4097
const COMPUTE_TILES := [[1024, "1,024 (least memory)"], [2048, "2,048"], [4096, "4,096 (fastest)"]]
const FILE_TILE_SIZES := [505, 1009, 2017, 4033]
const FORMATS := [["exr32", "EXR"], ["png16", "PNG 16"], ["png8", "PNG 8"]]
const POINT_FORMATS := [["csv", "CSV"], ["json", "JSON"]]

var project: TerrainProject
## Compute builds on the GPU (a machine setting, owned by the main window).
var builds_on_gpu := false
var busy := false:
	set(v):
		busy = v
		if _build_button != null:
			_build_button.disabled = v or _count == 0

var _box: VBoxContainer
var _list: VBoxContainer
var _res: OptionButton
var _folder: LineEdit
var _build_button: Button
var _summary: Label
var _folder_dialog: FileDialog
var _compute_tiles: OptionButton
var _file_tiles: CheckBox
var _file_tile_size: OptionButton
var _pattern: LineEdit
var _tiles_note: Label
var _count := 0


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

	var title := Label.new()
	title.text = "Build"
	title.add_theme_font_size_override("font_size", 18)
	_box.add_child(title)
	_box.add_child(_note("Writes every marked output at the build resolution, plus a build.json with the world size, height range and Unreal import values."))

	_box.add_child(_label("Resolution (samples per side)"))
	_res = OptionButton.new()
	for r in RESOLUTIONS:
		_res.add_item("%d%s" % [r, "  (Unreal landscape size)" if r in UNREAL_SIZES else ""])
	_res.item_selected.connect(func(i):
		project.set_build_resolution(RESOLUTIONS[i])
		_update_tiles_note())
	_box.add_child(_res)

	_box.add_child(_label("Folder"))
	var fh := HBoxContainer.new()
	_folder = LineEdit.new()
	_folder.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_folder.tooltip_text = "Relative folders are inside the project's folder."
	_folder.text_changed.connect(func(t): project.set_build_folder(t))
	fh.add_child(_folder)
	var browse := Button.new()
	browse.text = "Browse…"
	browse.pressed.connect(func():
		var dir := project.get_project_dir()
		if dir != "":
			_folder_dialog.current_dir = dir
		_folder_dialog.popup_centered_ratio(0.6))
	fh.add_child(browse)
	_box.add_child(fh)

	_folder_dialog = FileDialog.new()
	_folder_dialog.file_mode = FileDialog.FILE_MODE_OPEN_DIR
	_folder_dialog.access = FileDialog.ACCESS_FILESYSTEM
	_folder_dialog.use_native_dialog = true
	_folder_dialog.dir_selected.connect(func(dir):
		var base := project.get_project_dir()
		if base != "" and dir.begins_with(base.path_join("")):
			dir = dir.substr(base.path_join("").length())
		_folder.text = dir
		project.set_build_folder(dir))
	add_child(_folder_dialog)

	var gpu := CheckBox.new()
	gpu.text = "Compute on the GPU"
	gpu.button_pressed = builds_on_gpu
	gpu.tooltip_text = "Faster. Results match the CPU within 0.01% of the value range but are not\nbit-identical, and can differ slightly between GPUs. Off = identical files on every machine."
	gpu.toggled.connect(func(on):
		builds_on_gpu = on
		builds_on_gpu_toggled.emit(on))
	_box.add_child(gpu)

	_box.add_child(HSeparator.new())
	_box.add_child(_label("Large builds"))
	_box.add_child(_note("Builds over %s samples are computed in tiles of" % _thousands(TILED_ABOVE)))
	_compute_tiles = OptionButton.new()
	for t in COMPUTE_TILES:
		_compute_tiles.add_item(t[1])
	_compute_tiles.tooltip_text = "Smaller tiles use less memory; larger ones repeat less work at their edges."
	_compute_tiles.item_selected.connect(func(i): project.set_build_tile_size(COMPUTE_TILES[i][0]))
	_box.add_child(_compute_tiles)

	_file_tiles = CheckBox.new()
	_file_tiles.text = "Write images as tiles"
	_file_tiles.tooltip_text = "One file per tile instead of one per image, e.g. for Unreal World Partition.\nNeighbouring tiles share their edge row and column of samples."
	_file_tiles.toggled.connect(func(_on): _store_file_tiles())
	_box.add_child(_file_tiles)
	var th := HBoxContainer.new()
	var size_label := _label("Tile size")
	size_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	th.add_child(size_label)
	_file_tile_size = OptionButton.new()
	for s in FILE_TILE_SIZES:
		_file_tile_size.add_item(_thousands(s))
	_file_tile_size.item_selected.connect(func(_i): _store_file_tiles())
	th.add_child(_file_tile_size)
	_box.add_child(th)
	_pattern = LineEdit.new()
	_pattern.custom_minimum_size = Vector2(0, 0)
	_pattern.tooltip_text = "File name: {name} is the usual name, {x} and {y} the tile's column and row from 0."
	_pattern.text_changed.connect(func(_t): _store_file_tiles())
	_box.add_child(_pattern)
	_tiles_note = _note("")
	_box.add_child(_tiles_note)

	_box.add_child(HSeparator.new())
	_summary = _label("")
	_box.add_child(_summary)
	_list = VBoxContainer.new()
	_list.add_theme_constant_override("separation", 6)
	_box.add_child(_list)

	_build_button = Button.new()
	_build_button.text = "Build"
	_build_button.tooltip_text = "Build all marked outputs (Ctrl+B)"
	_build_button.pressed.connect(func():
		build_requested.emit(RESOLUTIONS[_res.selected], _folder.text.strip_edges()))
	_box.add_child(_build_button)


func _label(text: String) -> Label:
	var l := Label.new()
	l.text = text
	l.modulate = Color(1, 1, 1, 0.7)
	return l


func _note(text: String) -> Label:
	var l := _label(text)
	l.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	l.custom_minimum_size = Vector2(280, 0)
	return l


## 16384 -> "16,384".
static func _thousands(n: int) -> String:
	var s := str(n)
	var out := ""
	while s.length() > 3:
		out = "," + s.right(3) + out
		s = s.left(s.length() - 3)
	return s + out


func _store_file_tiles() -> void:
	var pattern := _pattern.text.strip_edges()
	if pattern == "":
		pattern = "{name}_x{x}_y{y}"
	project.set_build_file_tiles(_file_tiles.button_pressed, FILE_TILE_SIZES[_file_tile_size.selected], pattern)
	_update_tiles_note()


## Explain what the tile settings will do at the chosen resolution.
func _update_tiles_note() -> void:
	var res: int = project.get_build_resolution()
	var lines: PackedStringArray = []
	if res > TILED_ABOVE:
		lines.append("This build is computed in tiles; images are put together in the output folder while it runs (about %s of free space per image)." \
				% _size_text(res * res * 4))
	var enabled := _file_tiles.button_pressed
	_file_tile_size.disabled = not enabled
	_pattern.editable = enabled
	if enabled:
		var size: int = FILE_TILE_SIZES[_file_tile_size.selected]
		var count := ceili(float(res - 1) / float(size - 1))
		if (res - 1) % (size - 1) == 0:
			lines.append("%d × %d tiles of %s samples: every tile the same size, ready for Unreal World Partition." % [count, count, _thousands(size)])
		else:
			var even := (res - 1) / (size - 1) * (size - 1) + 1
			lines.append("%d × %d tiles; the last row and column are smaller. For equal tiles use a resolution of n × (%s − 1) + 1, e.g. %s." \
					% [count, count, _thousands(size), _thousands(maxi(even, size))])
	_tiles_note.text = "\n".join(lines)
	_tiles_note.visible = lines.size() > 0


static func _size_text(bytes: int) -> String:
	if bytes >= 1 << 30:
		return "%.1f GB" % (bytes / float(1 << 30))
	return "%d MB" % (bytes >> 20)


## Reload everything from the project (after load, undo, or a mark change).
func refresh() -> void:
	_res.select(maxi(RESOLUTIONS.find(project.get_build_resolution()), 0))
	if not _folder.has_focus():
		_folder.text = project.get_build_folder()
	var tile := project.get_build_tile_size()
	var ci := 1
	for i in COMPUTE_TILES.size():
		if COMPUTE_TILES[i][0] == tile:
			ci = i
	_compute_tiles.select(ci)
	var ft: Dictionary = project.get_build_file_tiles()
	_file_tiles.set_pressed_no_signal(ft["enabled"])
	_file_tile_size.select(maxi(FILE_TILE_SIZES.find(int(ft["size"])), 2))
	if not _pattern.has_focus():
		_pattern.text = ft["pattern"]
	_update_tiles_note()
	for c in _list.get_children():
		_list.remove_child(c)
		c.queue_free()
	var exports := project.get_exports()
	_count = exports.size()
	_summary.text = "No outputs marked yet. Select a node and tick a format under Export in its settings." if _count == 0 \
			else "%d output%s marked:" % [_count, "" if _count == 1 else "s"]
	_summary.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_summary.custom_minimum_size = Vector2(280, 0)
	for e in exports:
		_list.add_child(_export_row(e))
	busy = busy


func _export_row(e: Dictionary) -> Control:
	var panel := PanelContainer.new()
	var v := VBoxContainer.new()
	panel.add_child(v)
	var head := HBoxContainer.new()
	var title := Button.new()
	title.flat = true
	title.text = "%s  ·  %s" % [e["label"], e["node"]]
	title.tooltip_text = "View this node"
	title.alignment = HORIZONTAL_ALIGNMENT_LEFT
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	var node_id: String = e["node"]
	title.pressed.connect(func(): view_requested.emit(node_id))
	head.add_child(title)
	var remove := Button.new()
	remove.text = "Remove"
	remove.tooltip_text = "Stop exporting this output"
	var port: String = e["port"]
	var formats: PackedStringArray = e["formats"]
	remove.pressed.connect(func():
		for f in formats:
			export_toggled.emit(node_id, port, f, false))
	head.add_child(remove)
	v.add_child(head)
	var h := HBoxContainer.new()
	var out := _label("output '%s'" % port)
	out.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	h.add_child(out)
	for f in (POINT_FORMATS if e.get("type", "") == "point_set" else FORMATS):
		var cb := CheckBox.new()
		cb.text = f[1]
		cb.button_pressed = formats.has(f[0])
		var format: String = f[0]
		cb.toggled.connect(func(on): export_toggled.emit(node_id, port, format, on))
		h.add_child(cb)
	v.add_child(h)
	return panel
