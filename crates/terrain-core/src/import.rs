//! Reading heightmap images (for the File node).
//!
//! Supported: OpenEXR (first channel, 32/16-bit float, values taken as
//! metres) and PNG (8 or 16-bit, greyscale or colour; the first channel, as
//! 0..1). Other formats can be added here as they are needed.

use std::path::Path;

use crate::error::{CoreError, Result};

/// What an imported image's values mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageValues {
    /// Values are already heights in metres (EXR).
    Metres,
    /// Values are 0..1 (integer PNG).
    Normalised,
}

/// A decoded single-channel image, row 0 first.
#[derive(Clone, Debug)]
pub struct HeightImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
    pub values: ImageValues,
}

impl HeightImage {
    /// Bilinear sample at pixel coordinates (clamped to the edges).
    pub fn sample(&self, px: f64, py: f64) -> f32 {
        let (w, h) = (self.width as i64, self.height as i64);
        let x0 = px.floor();
        let y0 = py.floor();
        let tx = (px - x0) as f32;
        let ty = (py - y0) as f32;
        let at = |x: i64, y: i64| {
            let x = x.clamp(0, w - 1) as usize;
            let y = y.clamp(0, h - 1) as usize;
            self.data[y * self.width as usize + x]
        };
        let (x0, y0) = (x0 as i64, y0 as i64);
        let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
        top + (bottom - top) * ty
    }
}

/// Read a heightmap image. The format is chosen by file extension.
pub fn read_height_image(path: &Path) -> Result<HeightImage> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !path.is_file() {
        return Err(CoreError::Image(format!("file not found: {}", path.display())));
    }
    let img = match ext.as_str() {
        "exr" => read_exr(path)?,
        "png" => read_png(path)?,
        other => {
            return Err(CoreError::Image(format!(
                "unsupported image type '.{other}' (use .exr or .png)"
            )));
        }
    };
    if img.width < 2 || img.height < 2 {
        return Err(CoreError::Image("image must be at least 2 × 2 pixels".into()));
    }
    Ok(img)
}

fn read_exr(path: &Path) -> Result<HeightImage> {
    let img = exr::prelude::read_first_flat_layer_from_file(path)
        .map_err(|e| CoreError::Image(format!("{}: {e}", path.display())))?;
    let size = img.layer_data.size;
    let channels = &img.layer_data.channel_data.list;
    // Prefer a luminance or red channel; otherwise the first one.
    let channel = channels
        .iter()
        .find(|c| matches!(c.name.to_string().as_str(), "Y" | "R"))
        .or_else(|| channels.first())
        .ok_or_else(|| CoreError::Image("EXR has no channels".into()))?;
    let data: Vec<f32> = channel.sample_data.values_as_f32().collect();
    Ok(HeightImage {
        width: size.width() as u32,
        height: size.height() as u32,
        data,
        values: ImageValues::Metres,
    })
}

fn read_png(path: &Path) -> Result<HeightImage> {
    let img = image::open(path).map_err(|e| CoreError::Image(format!("{}: {e}", path.display())))?;
    let (width, height) = (img.width(), img.height());
    // First channel only: greyscale heightmaps, or red of an RGB(A) image.
    let data: Vec<f32> = match img {
        image::DynamicImage::ImageLuma8(b) => b.into_raw().into_iter().map(|v| v as f32 / 255.0).collect(),
        image::DynamicImage::ImageLumaA8(b) => b.pixels().map(|p| p.0[0] as f32 / 255.0).collect(),
        image::DynamicImage::ImageRgb8(b) => b.pixels().map(|p| p.0[0] as f32 / 255.0).collect(),
        image::DynamicImage::ImageRgba8(b) => b.pixels().map(|p| p.0[0] as f32 / 255.0).collect(),
        other => other
            .into_luma16()
            .into_raw()
            .into_iter()
            .map(|v| v as f32 / 65535.0)
            .collect(),
    };
    Ok(HeightImage {
        width,
        height,
        data,
        values: ImageValues::Normalised,
    })
}
