# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 EllisonDigital
@tool
extends Node3D
## Masks and full-float height textures are embedded in the imported scene.
@export var source_build: String
@export var world_size_m: Vector2
@export var resolution: Vector2i
@export var height_textures: Dictionary = {}
@export var masks: Dictionary = {}
## Paste one of the keys in Masks to preview it on all terrain outputs.
@export var preview_mask: String = "":
	set(value):
		preview_mask = value
		_apply_mask()

func _ready() -> void:
	_apply_mask()

func _apply_mask() -> void:
	for child in get_children():
		if child is MeshInstance3D and child.material_override is ShaderMaterial:
			child.material_override.set_shader_parameter("show_mask", masks.has(preview_mask))
			if masks.has(preview_mask):
				child.material_override.set_shader_parameter("mask_map", masks[preview_mask])
