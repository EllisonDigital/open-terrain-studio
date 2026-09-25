// 2D map view (ARCHITECTURE.md §7): the raw map, hillshaded and coloured for
// heightfields, greyscale for masks, as painted for colour maps.
shader_type canvas_item;

uniform sampler2D map_tex : filter_nearest, repeat_disable;
// Heightfields: colour by height between these (metres) and hillshade.
uniform float value_min = 0.0;
uniform float value_max = 1.0;
uniform bool is_mask = false;
// Colour maps: sRGB colours, shown as they are.
uniform bool is_color = false;
// Metres per texel, for the hillshade slope.
uniform float cell_m = 8.0;
// 0 = natural, 1 = clay (grey), 2 = height colours (same as the 3D view).
uniform int view_mode = 0;

vec3 height_ramp(float t) {
	vec3 c = mix(vec3(0.10, 0.25, 0.60), vec3(0.20, 0.60, 0.30), smoothstep(0.0, 0.25, t));
	c = mix(c, vec3(0.90, 0.85, 0.35), smoothstep(0.25, 0.5, t));
	c = mix(c, vec3(0.75, 0.35, 0.20), smoothstep(0.5, 0.75, t));
	return mix(c, vec3(0.97), smoothstep(0.75, 1.0, t));
}

void fragment() {
	float v = texture(map_tex, UV).r;
	if (is_color) {
		COLOR = vec4(clamp(texture(map_tex, UV).rgb, 0.0, 1.0), 1.0);
	} else if (is_mask) {
		COLOR = vec4(vec3(clamp(v, 0.0, 1.0)), 1.0);
	} else {
		vec2 texel = 1.0 / vec2(textureSize(map_tex, 0));
		float hl = texture(map_tex, UV - vec2(texel.x, 0.0)).r;
		float hr = texture(map_tex, UV + vec2(texel.x, 0.0)).r;
		float hu = texture(map_tex, UV - vec2(0.0, texel.y)).r;
		float hd = texture(map_tex, UV + vec2(0.0, texel.y)).r;
		vec3 n = normalize(vec3(hl - hr, hu - hd, 2.0 * cell_m));
		float shade = clamp(dot(n, normalize(vec3(-0.5, -0.6, 0.62))), 0.0, 1.0) * 0.8 + 0.2;
		float t = clamp((v - value_min) / max(value_max - value_min, 0.001), 0.0, 1.0);
		vec3 base = view_mode == 2 ? height_ramp(t) : (view_mode == 1 ? vec3(0.62) : mix(vec3(0.42, 0.40, 0.36), vec3(0.85, 0.83, 0.80), t));
		COLOR = vec4(base * shade, 1.0);
	}
}
