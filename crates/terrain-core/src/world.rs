use serde::{Deserialize, Serialize};

/// The physical world a project describes. Every node works in these units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    /// Horizontal extent in metres, `[x, y]`.
    pub size_m: [f64; 2],
    /// Vertical extent in metres, `[min, max]`. Used to normalise heights for
    /// 16-bit export and for Heightfield <-> Mask conversion.
    pub height_range_m: [f32; 2],
    /// Project seed, combined with each node's id and seed parameter.
    pub seed: u64,
}

impl Default for World {
    fn default() -> Self {
        Self {
            size_m: [8192.0, 8192.0],
            height_range_m: [0.0, 2000.0],
            seed: 12345,
        }
    }
}

impl World {
    /// Height range span in metres (never zero).
    pub fn height_span(&self) -> f32 {
        let span = self.height_range_m[1] - self.height_range_m[0];
        if span.abs() < f32::EPSILON { 1.0 } else { span }
    }

    /// Map a height in metres to 0..1 over the world height range (unclamped).
    pub fn normalise(&self, h: f32) -> f32 {
        (h - self.height_range_m[0]) / self.height_span()
    }

    /// Map a 0..1 value to metres over the world height range.
    pub fn denormalise(&self, v: f32) -> f32 {
        self.height_range_m[0] + v * self.height_span()
    }

    /// Centre of the world in metres.
    pub fn centre(&self) -> [f64; 2] {
        [self.size_m[0] * 0.5, self.size_m[1] * 0.5]
    }

    /// Check the world is usable. (Negated comparisons are deliberate: they also reject NaN.)
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn validate(&self) -> Result<(), String> {
        if !(self.size_m[0] > 0.0 && self.size_m[1] > 0.0) {
            return Err("world size must be positive".into());
        }
        if !(self.height_range_m[1] > self.height_range_m[0]) {
            return Err("height range max must be greater than min".into());
        }
        Ok(())
    }
}
