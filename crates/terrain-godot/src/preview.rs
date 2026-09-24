use std::sync::Arc;

use godot::classes::Image;
use godot::classes::image::Format;
use godot::prelude::*;
use terrain_core::{Grid, PortType};

/// One evaluated node output, ready for the viewport.
#[derive(GodotClass)]
#[class(base=RefCounted, no_init)]
pub struct TerrainPreview {
    grid: Arc<Grid>,
    port_type: PortType,
    node_id: GString,
    port: GString,
    generation: i64,
    min: f32,
    max: f32,
    base: Base<RefCounted>,
}

impl TerrainPreview {
    pub(crate) fn create(
        grid: Arc<Grid>,
        port_type: PortType,
        node_id: &str,
        port: &str,
        generation: i64,
    ) -> Gd<Self> {
        let (min, max) = grid.min_max();
        Gd::from_init_fn(|base| Self {
            grid,
            port_type,
            node_id: node_id.into(),
            port: port.into(),
            generation,
            min,
            max,
            base,
        })
    }
}

#[godot_api]
impl TerrainPreview {
    /// The data as a single-channel 32-bit float image (FORMAT_RF).
    /// Heightfields are in metres; masks are 0..1.
    #[func]
    fn get_image(&self) -> Option<Gd<Image>> {
        let mut bytes = Vec::with_capacity(self.grid.data.len() * 4);
        for v in &self.grid.data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        Image::create_from_data(
            self.grid.spec.width as i32,
            self.grid.spec.height as i32,
            false,
            Format::RF,
            &PackedByteArray::from(bytes.as_slice()),
        )
    }

    #[func]
    fn get_resolution(&self) -> i32 {
        self.grid.spec.width as i32
    }

    /// Lowest value in the data (metres for heightfields).
    #[func]
    fn get_min(&self) -> f32 {
        self.min
    }

    #[func]
    fn get_max(&self) -> f32 {
        self.max
    }

    /// "heightfield" or "mask".
    #[func]
    fn get_port_type(&self) -> GString {
        match self.port_type {
            PortType::Heightfield => "heightfield".into(),
            PortType::Mask => "mask".into(),
        }
    }

    #[func]
    fn get_node_id(&self) -> GString {
        self.node_id.clone()
    }

    #[func]
    fn get_port(&self) -> GString {
        self.port.clone()
    }

    #[func]
    fn get_generation(&self) -> i64 {
        self.generation
    }

    /// Value at a world position in metres (bilinear), e.g. for a hover readout.
    #[func]
    fn sample(&self, x_m: f64, y_m: f64) -> f32 {
        self.grid.sample_bilinear_m(x_m, y_m)
    }
}
