// Water preview (ARCHITECTURE.md §7): a grid like the terrain's, displaced
// to the water level from Rivers, Lakes and Sea. Dry ground (level
// -1,000,000) is dropped just below the terrain and discarded.
shader_type spatial;
render_mode blend_mix, depth_draw_opaque, cull_disabled;

// Water level in metres; nearest filtering so dry texels never blend in.
uniform sampler2D water_tex : filter_nearest, repeat_disable;
// Terrain height in metres, for the water depth.
uniform sampler2D height_tex : filter_linear, repeat_disable;
uniform float world_size = 4096.0;
uniform float exaggeration = 1.0;
// Depth at which the water looks fully deep, in metres.
uniform float deep_m = 25.0;

varying float wet;
varying float depth_m;

void vertex() {
	vec2 uv = VERTEX.xz / world_size + 0.5;
	float level = textureLod(water_tex, uv, 0.0).r;
	float ground = textureLod(height_tex, uv, 0.0).r;
	wet = level > -1.0e5 ? 1.0 : 0.0;
	depth_m = wet > 0.5 ? max(level - ground, 0.0) : 0.0;
	VERTEX.y = (wet > 0.5 ? level : ground - 2.0) * exaggeration;
}

void fragment() {
	if (wet < 0.5) {
		discard;
	}
	float deep = clamp(depth_m / deep_m, 0.0, 1.0);
	ALBEDO = mix(vec3(0.10, 0.30, 0.30), vec3(0.01, 0.06, 0.12), deep);
	ALPHA = mix(0.6, 0.93, deep);
	ROUGHNESS = 0.06;
	SPECULAR = 0.6;
}
