// Terrain preview: a flat grid displaced in the vertex shader from a 32-bit
// float height texture (ARCHITECTURE.md §7). Parameter edits only swap the
// texture; the mesh is never rebuilt.
shader_type spatial;
render_mode cull_back, depth_draw_opaque;

uniform sampler2D height_tex : filter_linear, repeat_disable;
// World width/depth in metres (square world).
uniform float world_size = 4096.0;
// value_in_metres = texel * value_scale + value_offset (1/0 for heightfields;
// masks are stretched over the world height range).
uniform float value_scale = 1.0;
uniform float value_offset = 0.0;
uniform float exaggeration = 1.0;
// World height range, for colouring.
uniform float height_min = 0.0;
uniform float height_max = 2400.0;
// 0 = natural, 1 = clay, 2 = height ramp.
uniform int view_mode = 0;
// Mask overlay (ARCHITECTURE.md §7): a 0..1 mask drawn as a false-colour
// tint over the terrain, e.g. a Slope mask over the terrain it came from.
uniform sampler2D overlay_tex : filter_linear, repeat_disable;
uniform bool show_overlay = false;
uniform float overlay_opacity = 0.85;

varying vec2 terrain_uv;
varying float height_m;

float height_at(vec2 uv) {
	return texture(height_tex, uv).r * value_scale + value_offset;
}

void vertex() {
	// Mesh spans -world_size/2..+world_size/2. uv (0,0) = world origin = image row/col 0.
	terrain_uv = VERTEX.xz / world_size + 0.5;
	height_m = height_at(terrain_uv);
	VERTEX.y = height_m * exaggeration;
}

// Dark blue (0) -> teal -> yellow -> red (1), in linear albedo.
vec3 mask_ramp(float m) {
	vec3 c = mix(vec3(0.02, 0.03, 0.12), vec3(0.02, 0.35, 0.40), smoothstep(0.0, 0.35, m));
	c = mix(c, vec3(0.80, 0.70, 0.08), smoothstep(0.35, 0.7, m));
	return mix(c, vec3(0.85, 0.12, 0.05), smoothstep(0.7, 1.0, m));
}

vec3 height_ramp(float t) {
	// Blue -> green -> yellow -> red -> white.
	vec3 c = mix(vec3(0.10, 0.25, 0.60), vec3(0.20, 0.60, 0.30), smoothstep(0.0, 0.25, t));
	c = mix(c, vec3(0.90, 0.85, 0.35), smoothstep(0.25, 0.5, t));
	c = mix(c, vec3(0.75, 0.35, 0.20), smoothstep(0.5, 0.75, t));
	return mix(c, vec3(0.97), smoothstep(0.75, 1.0, t)) * 0.55;
}

void fragment() {
	// Per-pixel normal from neighbouring texels, so lighting stays sharp even
	// when the mesh is coarser than the heightmap.
	vec2 texel = 1.0 / vec2(textureSize(height_tex, 0));
	float cell = world_size * texel.x;
	float hl = height_at(terrain_uv - vec2(texel.x, 0.0));
	float hr = height_at(terrain_uv + vec2(texel.x, 0.0));
	float hd = height_at(terrain_uv - vec2(0.0, texel.y));
	float hu = height_at(terrain_uv + vec2(0.0, texel.y));
	vec3 n = normalize(vec3((hl - hr) * exaggeration, 2.0 * cell, (hd - hu) * exaggeration));
	NORMAL = normalize((VIEW_MATRIX * vec4(n, 0.0)).xyz);

	float t = clamp((height_m - height_min) / max(height_max - height_min, 0.001), 0.0, 1.0);
	float slope = 1.0 - n.y; // 0 = flat, ~1 = cliff

	vec3 albedo;
	if (view_mode == 1) {
		albedo = vec3(0.30, 0.29, 0.28);
	} else if (view_mode == 2) {
		albedo = height_ramp(t);
	} else {
		// Linear albedos in the range of real ground (grass ~0.1, rock ~0.2, snow ~0.8).
		vec3 low = vec3(0.09, 0.13, 0.05);
		vec3 high = vec3(0.17, 0.15, 0.10);
		vec3 rock = vec3(0.20, 0.19, 0.17);
		vec3 snow = vec3(0.80, 0.82, 0.86);
		albedo = mix(low, high, smoothstep(0.2, 0.7, t));
		albedo = mix(albedo, rock, smoothstep(0.22, 0.45, slope));
		albedo = mix(albedo, snow, smoothstep(0.78, 0.9, t) * (1.0 - smoothstep(0.25, 0.45, slope)));
	}
	if (show_overlay) {
		float m = clamp(texture(overlay_tex, terrain_uv).r, 0.0, 1.0);
		albedo = mix(vec3(0.30, 0.29, 0.28), mask_ramp(m), overlay_opacity);
	}
	ALBEDO = albedo;
	ROUGHNESS = 0.92;
	SPECULAR = 0.2;
}
