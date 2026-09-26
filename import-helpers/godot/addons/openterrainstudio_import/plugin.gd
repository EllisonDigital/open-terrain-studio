# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
@tool
extends EditorPlugin
const Importer = preload("importer.gd")
var source_dialog: EditorFileDialog
var destination_dialog: EditorFileDialog
var message: AcceptDialog
var source_path := ""

func _enter_tree() -> void:
	source_dialog = EditorFileDialog.new()
	source_dialog.access = EditorFileDialog.ACCESS_FILESYSTEM
	source_dialog.file_mode = EditorFileDialog.FILE_MODE_OPEN_FILE
	source_dialog.add_filter("*.json", "OpenTerrainStudio build.json")
	source_dialog.file_selected.connect(_source_selected)
	get_editor_interface().get_base_control().add_child(source_dialog)
	destination_dialog = EditorFileDialog.new()
	destination_dialog.access = EditorFileDialog.ACCESS_RESOURCES
	destination_dialog.file_mode = EditorFileDialog.FILE_MODE_SAVE_FILE
	destination_dialog.add_filter("*.scn", "Binary terrain scene")
	destination_dialog.add_filter("*.tscn", "Text terrain scene")
	destination_dialog.file_selected.connect(_import_to)
	get_editor_interface().get_base_control().add_child(destination_dialog)
	message = AcceptDialog.new()
	get_editor_interface().get_base_control().add_child(message)
	add_tool_menu_item("Import OpenTerrainStudio build…", _choose_build)

func _choose_build() -> void:
	source_dialog.popup_centered_ratio(0.7)

func _source_selected(path: String) -> void:
	source_path = path
	destination_dialog.current_file = "terrain.scn"
	destination_dialog.popup_centered_ratio(0.7)

func _import_to(path: String) -> void:
	var result := Importer.import_build(source_path)
	if result.has("error"):
		message.dialog_text = result.error
		message.popup_centered()
		return
	var error := Importer.save_scene(result.scene, path)
	result.scene.free()
	if error != OK:
		message.dialog_text = "Could not save terrain scene: " + error_string(error)
		message.popup_centered()
		return
	get_editor_interface().get_resource_filesystem().scan()
	get_editor_interface().open_scene_from_path(path)

func _exit_tree() -> void:
	remove_tool_menu_item("Import OpenTerrainStudio build…")
	for dialog in [source_dialog, destination_dialog, message]:
		if is_instance_valid(dialog):
			dialog.queue_free()
