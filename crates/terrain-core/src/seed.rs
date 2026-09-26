//! Stable, platform-independent hashing for seeds.
//!
//! Never use `std::collections::hash_map::DefaultHasher` for anything that
//! affects results: its algorithm is not guaranteed stable between Rust
//! versions. Everything here is fixed forever, because changing it would change
//! every terrain ever made.

/// 64-bit FNV-1a hash of a byte string (Fowler, Noll & Vo; offset basis and
/// prime from the published FNV parameters).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// SplitMix64 finaliser: turns any 64-bit value into a well-mixed one
/// (Steele, Lea & Flood 2014, "Fast splittable pseudorandom number
/// generators", OOPSLA; constants from Stafford's "Mix13" variant).
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Seed for one node: project seed + the node's stable id + its own seed parameter.
pub fn node_seed(project_seed: u64, node_id: &str, seed_param: i64) -> u64 {
    mix64(mix64(project_seed ^ fnv1a64(node_id.as_bytes())) ^ mix64(seed_param as u64))
}

/// Derive a sub-seed (e.g. per noise octave) from a seed. The index offset is
/// the PCG multiplier (O'Neill 2014), used only as a well-mixed constant.
#[inline]
pub fn derive(seed: u64, index: u64) -> u64 {
    mix64(seed ^ mix64(index.wrapping_add(0x5851_f42d_4c95_7f2d)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn node_seed_depends_on_all_inputs() {
        let a = node_seed(1, "n_0001", 0);
        assert_ne!(a, node_seed(2, "n_0001", 0));
        assert_ne!(a, node_seed(1, "n_0002", 0));
        assert_ne!(a, node_seed(1, "n_0001", 1));
        assert_eq!(a, node_seed(1, "n_0001", 0));
    }
}
