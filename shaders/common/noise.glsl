// Noise basis functions: a line-by-line f32 port of
// crates/terrain-nodes/src/noise/basis.rs (which is f64). Lattice hashes are
// identical; values differ only by f32 rounding. Needs hash.glsl.

const vec2 GRADIENTS[16] = vec2[16](
    vec2(1.0, 0.0),
    vec2(0.9238795325, 0.3826834324),
    vec2(0.7071067812, 0.7071067812),
    vec2(0.3826834324, 0.9238795325),
    vec2(0.0, 1.0),
    vec2(-0.3826834324, 0.9238795325),
    vec2(-0.7071067812, 0.7071067812),
    vec2(-0.9238795325, 0.3826834324),
    vec2(-1.0, 0.0),
    vec2(-0.9238795325, -0.3826834324),
    vec2(-0.7071067812, -0.7071067812),
    vec2(-0.3826834324, -0.9238795325),
    vec2(0.0, -1.0),
    vec2(0.3826834324, -0.9238795325),
    vec2(0.7071067812, -0.7071067812),
    vec2(0.9238795325, -0.3826834324)
);

vec2 lattice_gradient(int ix, int iy, uvec2 seed) {
    return GRADIENTS[hash2(ix, iy, seed).y >> 28u];
}

float fade(float t) {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

float perlin(vec2 p, uvec2 seed) {
    vec2 p0 = floor(p);
    ivec2 i = ivec2(p0);
    vec2 f = p - p0;
    float n00 = dot(lattice_gradient(i.x, i.y, seed), f);
    float n10 = dot(lattice_gradient(i.x + 1, i.y, seed), f - vec2(1.0, 0.0));
    float n01 = dot(lattice_gradient(i.x, i.y + 1, seed), f - vec2(0.0, 1.0));
    float n11 = dot(lattice_gradient(i.x + 1, i.y + 1, seed), f - vec2(1.0, 1.0));
    float u = fade(f.x);
    float v = fade(f.y);
    return mix(mix(n00, n10, u), mix(n01, n11, u), v) * 1.4142135624;
}

const float F2 = 0.3660254038;
const float G2 = 0.2113248654;
const float SIMPLEX_SCALE = 99.20433458;

float simplex_corner(int gx, int gy, vec2 d, uvec2 seed) {
    float t = 0.5 - dot(d, d);
    if (t <= 0.0) {
        return 0.0;
    }
    float t2 = t * t;
    return t2 * t2 * dot(lattice_gradient(gx, gy, seed), d);
}

float simplex(vec2 p, uvec2 seed) {
    float s = (p.x + p.y) * F2;
    float i = floor(p.x + s);
    float j = floor(p.y + s);
    float t = (i + j) * G2;
    vec2 d0 = p - vec2(i - t, j - t);
    ivec2 o = d0.x > d0.y ? ivec2(1, 0) : ivec2(0, 1);
    vec2 d1 = d0 - vec2(o) + G2;
    vec2 d2 = d0 - 1.0 + 2.0 * G2;
    int ii = int(i);
    int jj = int(j);
    float n = simplex_corner(ii, jj, d0, seed) + simplex_corner(ii + o.x, jj + o.y, d1, seed)
        + simplex_corner(ii + 1, jj + 1, d2, seed);
    return clamp(n * SIMPLEX_SCALE, -1.0, 1.0);
}

float value_noise(vec2 p, uvec2 seed) {
    vec2 p0 = floor(p);
    ivec2 i = ivec2(p0);
    float u = fade(p.x - p0.x);
    float w = fade(p.y - p0.y);
    float v00 = unit_float(hash2(i.x, i.y, seed)) * 2.0 - 1.0;
    float v10 = unit_float(hash2(i.x + 1, i.y, seed)) * 2.0 - 1.0;
    float v01 = unit_float(hash2(i.x, i.y + 1, seed)) * 2.0 - 1.0;
    float v11 = unit_float(hash2(i.x + 1, i.y + 1, seed)) * 2.0 - 1.0;
    return mix(mix(v00, v10, u), mix(v01, v11, u), w);
}

// Cellular noise: (F1, F2, cell value).
vec3 cellular(vec2 p, uvec2 seed, float jitter) {
    int ix = int(floor(p.x));
    int iy = int(floor(p.y));
    float d1 = 3.4e38;
    float d2 = 3.4e38;
    uvec2 id = uvec2(0u);
    for (int dy = -2; dy <= 2; dy++) {
        for (int dx = -2; dx <= 2; dx++) {
            int cx = ix + dx;
            int cy = iy + dy;
            uvec2 h = hash2(cx, cy, seed);
            vec2 q = vec2(float(cx) + 0.5 + jitter * (unit_float(h) - 0.5),
                float(cy) + 0.5 + jitter * (unit_float(mix64(h)) - 0.5));
            vec2 d = q - p;
            float dd = dot(d, d);
            if (dd < d1) {
                d2 = d1;
                d1 = dd;
                id = h;
            } else if (dd < d2) {
                d2 = dd;
            }
        }
    }
    return vec3(sqrt(d1), sqrt(d2), unit_float(mix64(id ^ uvec2(0x5bd1e995u, 0u))));
}

// Basis ids match noise::basis::Basis: 0 = Perlin, 1 = simplex, 2 = value.
float basis_sample(uint basis, vec2 p, uvec2 seed) {
    if (basis == 1u) {
        return simplex(p, seed);
    }
    if (basis == 2u) {
        return value_noise(p, seed);
    }
    return perlin(p, seed);
}

vec2 next_octave(vec2 p, float lacunarity) {
    vec2 r = vec2(0.8 * p.x - 0.6 * p.y, 0.6 * p.x + 0.8 * p.y);
    return r * lacunarity + vec2(17.31, 43.17);
}

float fbm(uint basis, vec2 p, uvec2 seed, uint octaves, float lacunarity, float gain) {
    float sum = 0.0;
    float norm = 0.0;
    float amp = 1.0;
    for (uint o = 0u; o < octaves; o++) {
        sum += amp * basis_sample(basis, p, derive(seed, o));
        norm += amp;
        amp *= gain;
        p = next_octave(p, lacunarity);
    }
    return norm > 0.0 ? sum / norm : 0.0;
}

float ridged(uint basis, vec2 p, uvec2 seed, uint octaves, float lacunarity, float gain) {
    float sum = 0.0;
    float norm = 0.0;
    float amp = 1.0;
    float weight = 1.0;
    for (uint o = 0u; o < octaves; o++) {
        float n = 1.0 - abs(basis_sample(basis, p, derive(seed, o)));
        float signal = n * n * weight;
        weight = clamp(signal * 2.0, 0.0, 1.0);
        sum += amp * signal;
        norm += amp;
        amp *= gain;
        p = next_octave(p, lacunarity);
    }
    return norm > 0.0 ? sum / norm : 0.0;
}

float billow(uint basis, vec2 p, uvec2 seed, uint octaves, float lacunarity, float gain) {
    float sum = 0.0;
    float norm = 0.0;
    float amp = 1.0;
    for (uint o = 0u; o < octaves; o++) {
        sum += amp * (abs(basis_sample(basis, p, derive(seed, o))) * 2.0 - 1.0);
        norm += amp;
        amp *= gain;
        p = next_octave(p, lacunarity);
    }
    return norm > 0.0 ? sum / norm : 0.0;
}

float warped_fbm(uint basis, vec2 p, uvec2 seed, uint octaves, float lacunarity, float gain, float warp) {
    uint warp_octaves = min(octaves, 4u);
    float qx = fbm(basis, p, derive(seed, 1001u), warp_octaves, lacunarity, gain);
    float qy = fbm(basis, p + vec2(5.2, 1.3), derive(seed, 1002u), warp_octaves, lacunarity, gain);
    return fbm(basis, p + warp * vec2(qx, qy), seed, octaves, lacunarity, gain);
}
