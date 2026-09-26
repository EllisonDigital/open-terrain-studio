## Node graph editor. Draws the graph owned by the Rust core and forwards every
## edit to it; after each structural change it redraws from the core, so the
## screen can never disagree with the project.
extends GraphEdit

signal node_activated(id: String)   ## a node was selected (it becomes the viewed node)
signal selection_cleared
signal graph_edited                 ## structure changed (nodes/links)
signal layout_edited                ## nodes moved (no effect on the terrain)
signal status(message: String)

var project: TerrainProject         ## for grouping multi-node edits into one undo step

const PORT_COLORS := {
	"heightfield": Color(0.95, 0.72, 0.30),
	"mask": Color(0.55, 0.75, 0.95),
	"color_map": Color(0.92, 0.45, 0.75),
	"point_set": Color(0.45, 0.85, 0.40),
}
## Colour maps (slot type 2) only connect to colour inputs, points (3) to points.
const PORT_SLOT_TYPES := {"heightfield": 0, "mask": 1, "color_map": 2, "point_set": 3}
## Parameter ports (masks driving a value) get their own colour.
const PARAM_PORT_COLOR := Color(0.55, 0.9, 0.6)
const EXPORT_BADGE_COLOR := Color(0.55, 0.9, 0.6)
const GPU_BADGE_COLOR := Color(0.5, 0.8, 1.0)
const CPU_BADGE_COLOR := Color(1, 1, 1, 0.35)

var graph: TerrainGraph
## Editor tab shown: "terrain" or "colour". Links to nodes in the other tab
## are hidden; Colour-tab portals show where their data comes from.
var tab := "terrain"
## A GPU is in use: badge each node with where it computes.
var gpu_active := false
var viewed_id := ""
var viewed_port := ""

# node id -> {"inputs": [keys], "outputs": [keys]}
var _ports := {}
var _add_menu: PopupMenu
var _search_popup: PopupPanel
var _search_field: LineEdit
var _search_results: ItemList
var _add_position := Vector2.ZERO
var _type_ids: Array[String] = []
var _search_types: Array[Dictionary] = []
var _selection_modifier := false
var _selection_drag := false
var _selection_start := Vector2.ZERO


func _ready() -> void:
	right_disconnects = true
	show_zoom_label = true
	minimap_enabled = false
	# Heightfields and masks convert into each other automatically.
	add_valid_connection_type(0, 1)
	add_valid_connection_type(1, 0)

	connection_request.connect(_on_connection_request)
	disconnection_request.connect(_on_disconnection_request)
	delete_nodes_request.connect(_on_delete_nodes_request)
	popup_request.connect(_on_popup_request)
	node_selected.connect(_on_node_selected)
	node_deselected.connect(_on_node_deselected)
	end_node_move.connect(_on_end_node_move)

	_add_menu = PopupMenu.new()
	add_child(_add_menu)
	_add_menu.id_pressed.connect(_on_add_menu_pressed)
	_search_popup = PopupPanel.new()
	add_child(_search_popup)
	var search_box := VBoxContainer.new()
	search_box.custom_minimum_size = Vector2(300, 250)
	_search_popup.add_child(search_box)
	_search_field = LineEdit.new()
	_search_field.placeholder_text = "Search nodes…"
	_search_field.text_changed.connect(_update_search_results)
	_search_field.text_submitted.connect(func(_text): _choose_search_result())
	search_box.add_child(_search_field)
	_search_results = ItemList.new()
	_search_results.size_flags_vertical = Control.SIZE_EXPAND_FILL
	_search_results.item_activated.connect(func(index: int): _choose_search_result(index))
	search_box.add_child(_search_results)

	var add_button := Button.new()
	add_button.text = "Add node"
	add_button.tooltip_text = "Add a node (or right-click the graph)"
	add_button.pressed.connect(func(): _open_add_menu(size * 0.5))
	get_menu_hbox().add_child(add_button)


func set_graph(g: TerrainGraph) -> void:
	graph = g
	_build_add_menu()
	rebuild()
	frame_all.call_deferred()


## Show another editor tab ("terrain", "vegetation" or "colour").
func set_tab(t: String) -> void:
	tab = t
	if graph == null:
		return
	_build_add_menu()
	rebuild()
	frame_all()


## Zoom and scroll so every node is visible (at most 100% zoom).
func frame_all() -> void:
	# Node sizes are only known after a layout pass.
	await get_tree().process_frame
	var bounds := Rect2()
	var first := true
	for child in get_children():
		if child is GraphNode:
			var r := Rect2(child.position_offset, child.size)
			bounds = r if first else bounds.merge(r)
			first = false
	if first or size.x <= 0.0:
		return
	bounds = bounds.grow(40.0)
	var fit := minf(size.x / bounds.size.x, size.y / bounds.size.y)
	zoom = clampf(fit, 0.35, 1.0)
	scroll_offset = bounds.get_center() * zoom - size * 0.5


## Redraw every node and link from the core.
func rebuild() -> void:
	var selected := _selected_ids()
	clear_connections()
	for child in get_children():
		if child is GraphNode:
			remove_child(child)
			child.queue_free()
	_ports.clear()
	if graph == null:
		return
	var nodes: Array = graph.get_nodes()
	var links: Array = graph.get_links()
	for node in nodes:
		if node.get("tab", "terrain") == tab:
			_add_graph_node(node, selected.has(node["id"]), _portal_source(node, nodes, links),
					_portal_origin(node, nodes, links))
	for link in links:
		var from_idx: int = _ports.get(link["from"], {}).get("outputs", []).find(link["from_port"])
		var to_idx: int = _ports.get(link["to"], {}).get("inputs", []).find(link["to_port"])
		if from_idx >= 0 and to_idx >= 0:
			connect_node(link["from"], from_idx, link["to"], to_idx)
	_mark_viewed()


## Mark the viewed node (and, for nodes with several outputs, the viewed output).
func set_viewed(id: String, port := "") -> void:
	viewed_id = id
	viewed_port = port
	_mark_viewed()


## Select a node on screen (and view it).
func select_node(id: String) -> void:
	for child in get_children():
		if child is GraphNode:
			child.selected = child.name == id


## Add a node of `type_id` at a graph position; returns its id.
func add_node_at(type_id: String, graph_pos: Vector2) -> String:
	var id := graph.add_node_in_tab(type_id, graph_pos, tab)
	if id == "":
		status.emit("Could not add node: %s" % graph.get_last_error())
		return ""
	rebuild()
	graph_edited.emit()
	select_node(id)
	node_activated.emit(id)
	return id


## For a portal: the tab its source is in ("Terrain"…), or "".
func _portal_origin(node: Dictionary, nodes: Array, links: Array) -> String:
	if not String(node["type"]).begins_with("portal."):
		return ""
	for link in links:
		if link["to"] == node["id"] and link["to_port"] == "in":
			for n in nodes:
				if n["id"] == link["from"]:
					return String(n.get("tab", "terrain")).capitalize()
	return ""


## For a portal: "<node> · <output>" it brings from another tab, or "".
func _portal_source(node: Dictionary, nodes: Array, links: Array) -> String:
	if not String(node["type"]).begins_with("portal."):
		return ""
	for link in links:
		if link["to"] == node["id"] and link["to_port"] == "in":
			for n in nodes:
				if n["id"] == link["from"]:
					var port_label: String = link["from_port"]
					for o in n["outputs"]:
						if o["key"] == link["from_port"]:
							port_label = o["label"]
					return "%s · %s" % [n["label"], port_label]
	return "(source deleted)"


func _add_graph_node(node: Dictionary, selected: bool, portal_source := "", portal_origin := "") -> void:
	var gn := GraphNode.new()
	gn.name = node["id"]
	gn.title = node["label"] if portal_source == "" else "⇠ " + portal_source
	gn.position_offset = node["pos"]
	gn.tooltip_text = "%s (%s)" % [node["label"], node["id"]]
	gn.custom_minimum_size = Vector2(160, 0)
	gn.selected = selected
	if not node["known"]:
		gn.self_modulate = Color(1.0, 0.45, 0.45)
		gn.tooltip_text += "\nThis node type is not available in this version. It is kept so saving loses nothing."

	var inputs: Array = node["inputs"]
	var outputs: Array = node["outputs"]
	var rows := maxi(maxi(inputs.size(), outputs.size()), 1)
	for i in rows:
		var row := HBoxContainer.new()
		var left := Label.new()
		var right := Label.new()
		left.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		right.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
		var is_param: bool = i < inputs.size() and inputs[i].has("param")
		if i < inputs.size():
			if is_param:
				left.text = inputs[i]["label"]
				left.add_theme_color_override("font_color", PARAM_PORT_COLOR)
				left.tooltip_text = "Mask port: scales %s" % inputs[i]["label"]
				left.mouse_filter = Control.MOUSE_FILTER_PASS
			else:
				left.text = inputs[i]["label"] + ("" if not inputs[i]["optional"] else " (opt.)")
		if i < outputs.size():
			right.text = outputs[i]["label"]
			right.set_meta("port", outputs[i]["key"])
			right.set_meta("label", outputs[i]["label"])
		row.add_child(left)
		row.add_child(right)
		gn.add_child(row)
		var has_in := i < inputs.size() and portal_source == ""
		if portal_source != "":
			left.text = "from " + portal_origin if portal_origin != "" else "from another tab"
			left.add_theme_color_override("font_color", Color(1, 1, 1, 0.5))
		var has_out := i < outputs.size()
		var in_type: String = inputs[i]["type"] if has_in else "heightfield"
		var out_type: String = outputs[i]["type"] if has_out else "heightfield"
		gn.set_slot(i, has_in, PORT_SLOT_TYPES[in_type], PARAM_PORT_COLOR if is_param else PORT_COLORS[in_type],
				has_out, PORT_SLOT_TYPES[out_type], PORT_COLORS[out_type])

	if not Array(node.get("exported", PackedStringArray())).is_empty():
		var badge := Label.new()
		badge.text = "EXPORT"
		badge.tooltip_text = "Marked for export: written by Build"
		badge.mouse_filter = Control.MOUSE_FILTER_PASS
		badge.add_theme_font_size_override("font_size", 10)
		badge.add_theme_color_override("font_color", EXPORT_BADGE_COLOR)
		gn.get_titlebar_hbox().add_child(badge)

	if gpu_active and node["known"]:
		var on_gpu: bool = node.get("gpu", false)
		var badge := Label.new()
		badge.text = "GPU" if on_gpu else "CPU"
		badge.tooltip_text = "Computed on the GPU" if on_gpu else "Computed on the CPU (no GPU version yet)"
		badge.mouse_filter = Control.MOUSE_FILTER_PASS
		badge.add_theme_font_size_override("font_size", 10)
		badge.add_theme_color_override("font_color", GPU_BADGE_COLOR if on_gpu else CPU_BADGE_COLOR)
		gn.get_titlebar_hbox().add_child(badge)

	_ports[node["id"]] = {
		"inputs": inputs.map(func(p): return p["key"]),
		"outputs": outputs.map(func(p): return p["key"]),
	}
	add_child(gn)


func _mark_viewed() -> void:
	for child in get_children():
		if child is GraphNode:
			var gn := child as GraphNode
			var label := gn.title.trim_prefix("▶ ")
			gn.title = ("▶ " + label) if gn.name == viewed_id else label
			# Several outputs: point at the one being viewed.
			var outs := gn.find_children("*", "Label", true, false).filter(func(l): return l.has_meta("port"))
			for l in outs:
				var viewed: bool = outs.size() > 1 and gn.name == viewed_id and l.get_meta("port") == viewed_port
				l.text = ("▶ " if viewed else "") + String(l.get_meta("label"))


func _selected_ids() -> Array[String]:
	var ids: Array[String] = []
	for child in get_children():
		if child is GraphNode and child.selected:
			ids.append(String(child.name))
	return ids


# ---- add-node menu --------------------------------------------------------

func _build_add_menu() -> void:
	for child in _add_menu.get_children():
		child.queue_free()
	_add_menu.clear()
	_type_ids.clear()
	_search_types.clear()
	var by_category := {}
	for t in graph.get_node_types():
		# Portals are made with "Send to … tab". Colour work lives in the Colour
		# tab, plants in the Vegetation tab (which gets terrain through portals).
		var hidden: Array = {
			"terrain": ["Colour", "Vegetation"],
			"vegetation": ["Colour", "Terrain", "Simulate"],
			"colour": ["Vegetation"],
		}.get(tab, [])
		if t["category"] == "Portal" or hidden.has(t["category"]):
			continue
		by_category.get_or_add(t["category"], []).append(t)
	var order := ["Primitives", "Noise", "Terrain", "Adjust", "Combine", "Data", "Simulate", "Colour", "Output"]
	if tab == "vegetation":
		order.push_front("Vegetation")
	var categories: Array = by_category.keys()
	categories.sort_custom(func(a, b):
		var ia := order.find(a)
		var ib := order.find(b)
		return (ia if ia >= 0 else 99) < (ib if ib >= 0 else 99))
	for cat in categories:
		var sub := PopupMenu.new()
		sub.name = "cat_" + String(cat)
		for t in by_category[cat]:
			sub.add_item(t["label"], _type_ids.size())
			sub.set_item_tooltip(sub.item_count - 1, t["description"])
			_type_ids.append(t["type_id"])
			_search_types.append(t)
		sub.id_pressed.connect(func(idx: int): add_node_at(_type_ids[idx], _add_position))
		_add_menu.add_child(sub)
		_add_menu.add_submenu_node_item(cat, sub)
	_add_menu.add_separator()
	_add_menu.add_item("Search nodes…", 10000)


func _on_add_menu_pressed(id: int) -> void:
	if id == 10000:
		_open_search.call_deferred()


func _open_search() -> void:
	_search_field.clear()
	_update_search_results("")
	_search_popup.popup(Rect2i(_add_menu.position, Vector2i(320, 270)))
	_search_field.grab_focus.call_deferred()


func _update_search_results(query: String) -> void:
	_search_results.clear()
	var terms := query.strip_edges().to_lower().split(" ", false)
	for t in _search_types:
		var haystack := (String(t["label"]) + " " + String(t["category"]) + " " + String(t["type_id"]) + " " + String(t["description"])).to_lower()
		var matches := true
		for term in terms:
			if not haystack.contains(term):
				matches = false
				break
		if matches:
			var index := _search_results.item_count
			_search_results.add_item("%s  ·  %s" % [t["label"], t["category"]])
			_search_results.set_item_metadata(index, t["type_id"])
			_search_results.set_item_tooltip(index, t["description"])
	if _search_results.item_count > 0:
		_search_results.select(0)


func _choose_search_result(index := -1) -> void:
	if index < 0:
		var selected := _search_results.get_selected_items()
		if selected.is_empty():
			return
		index = selected[0]
	if index >= _search_results.item_count:
		return
	var type_id: String = _search_results.get_item_metadata(index)
	_search_popup.hide()
	add_node_at(type_id, _add_position)


func _input(event: InputEvent) -> void:
	if _search_popup.visible and event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_DOWN or event.keycode == KEY_UP:
			var selected := _search_results.get_selected_items()
			var current := selected[0] if not selected.is_empty() else 0
			var next := clampi(current + (1 if event.keycode == KEY_DOWN else -1), 0, _search_results.item_count - 1)
			if _search_results.item_count > 0:
				_search_results.select(next)
			get_viewport().set_input_as_handled()


func _open_add_menu(at_position: Vector2) -> void:
	_add_position = (scroll_offset + at_position) / zoom
	_add_menu.position = Vector2i(get_screen_position() + at_position)
	_add_menu.reset_size()
	_add_menu.popup()


# ---- GraphEdit signal handlers --------------------------------------------

func _on_popup_request(at_position: Vector2) -> void:
	_open_add_menu(at_position)


func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		_selection_modifier = event.shift_pressed or event.ctrl_pressed or event.meta_pressed
		_selection_start = event.position
	elif event is InputEventMouseButton and not event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
		_selection_modifier = false
		_selection_drag = false
	if event is InputEventMouseMotion and Input.is_mouse_button_pressed(MOUSE_BUTTON_LEFT) and event.position.distance_to(_selection_start) > 4.0:
		_selection_drag = true


func _on_connection_request(from_node: StringName, from_port: int, to_node: StringName, to_port: int) -> void:
	var from_key: String = _ports[String(from_node)]["outputs"][from_port]
	var to_key: String = _ports[String(to_node)]["inputs"][to_port]
	var err := graph.connect_ports(from_node, from_key, to_node, to_key)
	if err != "":
		status.emit(err)
		return
	rebuild()
	graph_edited.emit()


func _on_disconnection_request(_from_node: StringName, _from_port: int, to_node: StringName, to_port: int) -> void:
	var to_key: String = _ports[String(to_node)]["inputs"][to_port]
	graph.disconnect_input(to_node, to_key)
	rebuild()
	graph_edited.emit()


func _on_delete_nodes_request(nodes: Array[StringName]) -> void:
	if nodes.is_empty():
		return
	if project != null and nodes.size() > 1:
		project.begin_edit_group("Delete %d nodes" % nodes.size())
	for id in nodes:
		graph.remove_node(id)
	if project != null:
		project.end_edit_group()
	rebuild()
	graph_edited.emit()
	selection_cleared.emit()


func _on_node_selected(node: Node) -> void:
	# GraphEdit emits this for each node added by Shift/Ctrl selection and box
	# selection. Neither operation should retarget the preview.
	if not _selection_modifier and not _selection_drag and _selected_ids().size() == 1:
		node_activated.emit(String(node.name))


func _on_node_deselected(_node: Node) -> void:
	# Wait a frame: when clicking from one node to another, the new selection
	# arrives right after this deselection.
	await get_tree().process_frame
	if is_instance_valid(self) and _selected_ids().is_empty():
		selection_cleared.emit()


func _on_end_node_move() -> void:
	if project != null:
		project.begin_edit_group("Move nodes")
	for child in get_children():
		if child is GraphNode and child.selected:
			graph.set_node_position(child.name, child.position_offset)
	if project != null:
		project.end_edit_group()
	layout_edited.emit()
