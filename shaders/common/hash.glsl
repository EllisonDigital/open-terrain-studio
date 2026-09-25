// The seed hashes of terrain_core::seed, bit for bit. 64-bit integers are
// uvec2(low word, high word): only 32-bit integer maths is needed.

uvec2 add64(uvec2 a, uvec2 b) {
    uint carry;
    uint lo = uaddCarry(a.x, b.x, carry);
    return uvec2(lo, a.y + b.y + carry);
}

// Low 64 bits of a × b (wrapping, like Rust's wrapping_mul).
uvec2 mul64(uvec2 a, uvec2 b) {
    uint hi;
    uint lo;
    umulExtended(a.x, b.x, hi, lo);
    return uvec2(lo, hi + a.x * b.y + a.y * b.x);
}

// a >> n for 0 < n < 32.
uvec2 shr64(uvec2 a, uint n) {
    return uvec2((a.x >> n) | (a.y << (32u - n)), a.y >> n);
}

// SplitMix64 finaliser (seed::mix64).
uvec2 mix64(uvec2 z) {
    z = add64(z, uvec2(0x7f4a7c15u, 0x9e3779b9u));
    z = mul64(z ^ shr64(z, 30u), uvec2(0x1ce4e5b9u, 0xbf58476du));
    z = mul64(z ^ shr64(z, 27u), uvec2(0x133111ebu, 0x94d049bbu));
    return z ^ shr64(z, 31u);
}

// A signed integer as a 64-bit value (sign-extended, like `i as i64 as u64`).
uvec2 from_int(int v) {
    return uvec2(uint(v), v < 0 ? 0xffffffffu : 0u);
}

// seed::derive
uvec2 derive(uvec2 seed, uint index) {
    return mix64(seed ^ mix64(add64(uvec2(index, 0u), uvec2(0x4c957f2du, 0x5851f42du))));
}

// A uniform value in 0..1 from a hash: its top 24 bits (the CPU uses 53).
float unit_float(uvec2 h) {
    return float(h.y >> 8u) * (1.0 / 16777216.0);
}

// Hash of an integer lattice point (noise::basis::hash2).
uvec2 hash2(int ix, int iy, uvec2 seed) {
    return mix64(mix64(seed ^ mul64(from_int(ix), uvec2(0x7f4a7c15u, 0x9e3779b9u)))
        ^ mul64(from_int(iy), uvec2(0x27d4eb4fu, 0xc2b2ae3du)));
}
