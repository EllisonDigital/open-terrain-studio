use std::sync::Arc;

use godot::classes::Image;
use godot::classes::image::Format;
use godot::prelude::*;
use terrain_core::{Grid, PortType, Value};

/// What a preview job produces.
pub struct PreviewData {
    pub value: Value,
    /// For a mask or colour map: the terrain it was computed from, and that
    /// node's id.
    pub base: Option<(Arc<Grid>, String)>,
    /// For a heightfield: the water level over it in metres
    /// ([`crate::builder::DRY`] where there is no water), if any node it was
    /// made with adds water.
    pub water: Option<Arc<Grid>>,
    /// For a heightfield: its snow cover (0..1), if any node it was made
    /// with adds snow.
    pub snow: Option<Arc<Grid>>,
    /// Nodes computed (not taken from the cache) for this preview.
    pub computed: u64,
    pub millis: f64,
    /// The GPU used, if any (nodes without a kernel still ran on the CPU).
    pub gpu: Option<String>,
}

fn floats_image(width: u32, height: u32, data: &[f32], format: Format) -> Option<Gd<Image>> {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Image::create_from_data(
        width as i32,
        height as i32,
        false,
        format,
        &PackedByteArray::from(bytes.as_slice()),
    )
}

fn grid_image(grid: &Grid) -> Option<Gd<Image>> {
    let mut bytes = Vec::with_capacity(grid.data.len() * 4);
    for v in &grid.data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    Image::create_from_data(
        grid.spec.width as i32,
        grid.spec.height as i32,
        false,
        Format::RF,
        &PackedByteArray::from(bytes.as_slice()),
    )
}

/// One evaluated node output, ready for the viewport.
#[derive(GodotClass)]
#[class(base=RefCounted, no_init)]
pub struct TerrainPreview {
    data: PreviewData,
    node_id: GString,
    port: GString,
    generation: i64,
    min: f32,
    max: f32,
    base: Base<RefCounted>,
}

impl TerrainPreview {
    pub(crate) fn create(data: PreviewData, node_id: &str, port: &str, generation: i64) -> Gd<Self> {
        let (min, max) = data
            .value
            .samples()
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            });
        Gd::from_init_fn(|base| Self {
            data,
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
    /// The data as a 32-bit float image: single-channel (FORMAT_RF) with
    /// heightfields in metres and masks 0..1, or RGBA (FORMAT_RGBAF, sRGB
    /// 0..1) for a colour map.
    #[func]
    fn get_image(&self) -> Option<Gd<Image>> {
        match self.data.value.color() {
            Some(c) => floats_image(c.spec.width, c.spec.height, &c.data, Format::RGBAF),
            None => grid_image(self.data.value.grid()),
        }
    }

    /// For a mask: the terrain it was computed from (heights in metres,
    /// FORMAT_RF), to drape the mask over. Null otherwise.
    #[func]
    fn get_base_image(&self) -> Option<Gd<Image>> {
        self.data.base.as_ref().and_then(|(g, _)| grid_image(g))
    }

    /// For a heightfield made with Rivers, Lakes or Sea: the water level in
    /// metres (FORMAT_RF), -1,000,000 where it's dry. Null without water.
    #[func]
    fn get_water_image(&self) -> Option<Gd<Image>> {
        self.data.water.as_ref().and_then(|g| grid_image(g))
    }

    /// For a heightfield made with Snow: its snow cover, 0..1 (FORMAT_RF).
    /// Null without snow.
    #[func]
    fn get_snow_image(&self) -> Option<Gd<Image>> {
        self.data.snow.as_ref().and_then(|g| grid_image(g))
    }

    /// Node id of the base terrain, or "".
    #[func]
    fn get_base_node_id(&self) -> GString {
        self.data
            .base
            .as_ref()
            .map(|(_, id)| GString::from(id.as_str()))
            .unwrap_or_default()
    }

    #[func]
    fn get_resolution(&self) -> i32 {
        self.data.value.spec().width as i32
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

    /// "heightfield", "mask" or "color_map".
    #[func]
    fn get_port_type(&self) -> GString {
        match self.data.value.port_type() {
            PortType::Heightfield => "heightfield".into(),
            PortType::Mask => "mask".into(),
            PortType::ColorMap => "color_map".into(),
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

    /// Nodes that were computed for this preview (the rest came from the cache).
    #[func]
    fn get_computed_nodes(&self) -> i64 {
        self.data.computed as i64
    }

    /// Time the preview took, in milliseconds.
    #[func]
    fn get_millis(&self) -> f64 {
        self.data.millis
    }

    /// Name of the GPU the preview was computed with, or "" for CPU only.
    #[func]
    fn get_gpu_name(&self) -> GString {
        self.data.gpu.as_deref().unwrap_or_default().into()
    }

    /// Value at a world position in metres (bilinear), e.g. for a hover
    /// readout. For a colour map, its brightness (the mean of R, G and B).
    #[func]
    fn sample(&self, x_m: f64, y_m: f64) -> f32 {
        let c = self.sample_color(x_m, y_m);
        match self.data.value.port_type() {
            PortType::ColorMap => (c.r + c.g + c.b) / 3.0,
            _ => c.r,
        }
    }

    /// Colour at a world position in metres (bilinear, sRGB); for a
    /// heightfield or mask, the value in every channel.
    #[func]
    fn sample_color(&self, x_m: f64, y_m: f64) -> Color {
        match self.data.value.color() {
            Some(c) => {
                let [r, g, b, a] = c.sample_bilinear_m(x_m, y_m);
                Color::from_rgba(r, g, b, a)
            }
            None => {
                let v = self.data.value.grid().sample_bilinear_m(x_m, y_m);
                Color::from_rgba(v, v, v, 1.0)
            }
        }
    }

    /// Base terrain height at a world position (metres), or NaN without one.
    #[func]
    fn sample_base(&self, x_m: f64, y_m: f64) -> f32 {
        self.data
            .base
            .as_ref()
            .map(|(g, _)| g.sample_bilinear_m(x_m, y_m))
            .unwrap_or(f32::NAN)
    }
}
