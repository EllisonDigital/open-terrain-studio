// Shared by every kernel: the parameter block and grid indexing.
// Grids are row-major f32 storage buffers, exactly like Grid::data on the CPU.

// Matches terrain_core::gpu::Params: 16 uints then 16 floats (128 bytes).
layout(push_constant, std430) uniform Params {
    uint u[16];
    float f[16];
} pc;

#define WIDTH pc.u[0]
#define HEIGHT pc.u[1]

// Flat index of column i, row j.
uint cell(uint i, uint j) {
    return j * WIDTH + i;
}

// Flat index with coordinates clamped to the grid: edges repeat.
uint cell_clamped(int i, int j) {
    return cell(uint(clamp(i, 0, int(WIDTH) - 1)), uint(clamp(j, 0, int(HEIGHT) - 1)));
}

// Hermite smoothstep that, like terrain_core::ops::smoothstep, allows e0 > e1
// and treats e0 == e1 as a step.
float smoothstep_any(float e0, float e1, float x) {
    if (e0 == e1) {
        return x < e0 ? 0.0 : 1.0;
    }
    float t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// 1 inside lo..hi, fading to 0 over `falloff` outside (terrain_core::ops::soft_range).
float soft_range(float x, float lo, float hi, float falloff) {
    float a = min(lo, hi);
    float b = max(lo, hi);
    if (falloff <= 0.0) {
        return (x >= a && x <= b) ? 1.0 : 0.0;
    }
    return smoothstep_any(a - falloff, a, x) * (1.0 - smoothstep_any(b, b + falloff, x));
}

// atan accurate to about 1e-7 rad (Cephes atanf). The built-in atan may be
// off by 4096 ulp under Vulkan's precision rules, which threshold masks
// (slope, aspect) magnify.
float atan_precise(float x) {
    float a = abs(x);
    float base = 0.0;
    if (a > 2.414213562) {
        base = 1.570796327;
        a = -1.0 / a;
    } else if (a > 0.414213562) {
        base = 0.785398163;
        a = (a - 1.0) / (a + 1.0);
    }
    float z = a * a;
    float r = base + ((((8.05374449538e-2 * z - 1.38776856032e-1) * z + 1.99777106478e-1) * z
        - 3.33329491539e-1) * z * a + a);
    return x < 0.0 ? -r : r;
}
