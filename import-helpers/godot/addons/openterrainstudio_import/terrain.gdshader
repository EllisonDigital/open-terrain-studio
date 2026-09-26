// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 EllisonDigital
shader_type spatial;
uniform sampler2D mask_map : filter_nearest, repeat_disable;
uniform bool show_mask = false;
void fragment() {
    ALBEDO = show_mask ? vec3(texture(mask_map, UV).r) : vec3(0.36, 0.42, 0.28);
    ROUGHNESS = 0.9;
}
