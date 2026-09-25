//! GLSL compute kernels of the node library (sources in `shaders/`), one per
//! node family. See `shaders/README.md` for the conventions.

use terrain_core::gpu::{self, COMMON_GLSL, Kernel};

const HASH_GLSL: &str = include_str!("../../../shaders/common/hash.glsl");
const NOISE_GLSL: &str = include_str!("../../../shaders/common/noise.glsl");

/// Every noise node.
pub const NOISE: Kernel = Kernel {
    name: "noise",
    parts: &[
        COMMON_GLSL,
        HASH_GLSL,
        NOISE_GLSL,
        include_str!("../../../shaders/noise.comp"),
    ],
};

/// Blur/Sharpen blending, Transform and Warp.
pub const ADJUST: Kernel = Kernel {
    name: "adjust",
    parts: &[
        COMMON_GLSL,
        HASH_GLSL,
        NOISE_GLSL,
        include_str!("../../../shaders/adjust.comp"),
    ],
};

/// Slope, Aspect and Curvature masks.
pub const DATA: Kernel = Kernel {
    name: "data",
    parts: &[COMMON_GLSL, include_str!("../../../shaders/data.comp")],
};

/// Thermal erosion.
pub const THERMAL: Kernel = Kernel {
    name: "thermal",
    parts: &[COMMON_GLSL, include_str!("../../../shaders/thermal.comp")],
};

/// Every kernel the app uses, core ones included (to compile them all at
/// start-up and in self-tests).
pub fn all() -> Vec<Kernel> {
    let mut all = gpu::KERNELS.to_vec();
    all.extend([NOISE, ADJUST, DATA, THERMAL]);
    all
}
