//! Putting tiled                             out.extend(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));ytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)),uilds back together on disk, and writing images from them
//! a band of rows at a time, so a build never holds a whole-world image in
//! memory.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{CoreError, Result};
use crate::grid::GridSpec;

/// Rows read from disk at a time when streaming an image.
const BAND_ROWS: u32 = 64;

/// A rectangle of samples: columns `x0..x0 + w`, rows `y0..y0 + h`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
}

/// Split `n` samples into tiles of `size` that share their edge sample
/// (starts `0, size - 1, 2 (size - 1) …`); the last may be smaller.
/// Returns `(start, len)`.
fn shared_edge_runs(n: u32, size: u32) -> Vec<(u32, u32)> {
    let size = size.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    loop {
        let len = size.min(n - start);
        out.push((start, len));
        if start + len >= n {
            break;
        }
        start += size - 1;
    }
    out
}

/// The file tiles of a `width × height` image with tiles of `size` samples
/// sharing their edges: `([column, row], rect)`, row by row.
pub fn file_tiles(width: u32, height: u32, size: u32) -> Vec<([u32; 2], Rect)> {
    let (cols, rows) = (shared_edge_runs(width, size), shared_edge_runs(height, size));
    let mut out = Vec::new();
    for (ty, &(y0, h)) in rows.iter().enumerate() {
        for (tx, &(x0, w)) in cols.iter().enumerate() {
            out.push(([tx as u32, ty as u32], Rect { x0, y0, w, h }));
        }
    }
    out
}

/// Whether every file tile of an `n`-sample side has `size` samples.
pub fn tiles_even(n: u32, size: u32) -> bool {
    size >= 2 && n >= size && (n - 1).is_multiple_of(size - 1)
}

/// A whole-build image of `channels` f32 values per sample, kept in a
/// temporary file and filled tile by tile. The file is deleted on drop.
pub struct RawImage {
    path: PathBuf,
    file: Mutex<File>,
    pub width: u32,
    pub height: u32,
    pub channels: usize,
}

impl RawImage {
    pub fn create(path: PathBuf, width: u32, height: u32, channels: usize) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(width as u64 * height as u64 * channels as u64 * 4)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
            width,
            height,
            channels,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.file.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn offset(&self, x: u32, y: u32) -> u64 {
        (y as u64 * self.width as u64 + x as u64) * self.channels as u64 * 4
    }

    /// Store the samples of `window` (a window of the build grid).
    pub fn write_window(&self, window: GridSpec, data: &[f32]) -> Result<()> {
        let o = window.offset();
        let row_len = window.width as usize * self.channels;
        debug_assert_eq!(data.len(), row_len * window.height as usize);
        let mut file = self.lock();
        let mut bytes = Vec::with_capacity(row_len * 4);
        for (j, row) in data.chunks(row_len).enumerate() {
            bytes.clear();
            bytes.extend(row.iter().flat_map(|v| v.to_le_bytes()));
            file.seek(SeekFrom::Start(self.offset(o[0], o[1] + j as u32)))?;
            file.write_all(&bytes)?;
        }
        Ok(())
    }

    /// The samples of `rect`, row by row.
    pub fn read(&self, rect: Rect) -> Result<Vec<f32>> {
        let row_len = rect.w as usize * self.channels;
        let mut out = Vec::with_capacity(row_len * rect.h as usize);
        let mut bytes = vec![0u8; row_len * 4];
        let mut file = self.lock();
        for y in rect.y0..rect.y0 + rect.h {
            file.seek(SeekFrom::Start(self.offset(rect.x0, y)))?;
            file.read_exact(&mut bytes)?;
            out.extend(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
        }
        Ok(out)
    }
}

impl Drop for RawImage {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Write `rect` of `src` as a 32-bit float EXR with one channel (`Y`) or
/// four (`R`, `G`, `B`, `A`), a band of rows at a time.
pub fn write_exr_streamed(src: &RawImage, rect: Rect, path: &Path) -> Result<()> {
    use exr::prelude::*;
    let band = Mutex::new((u32::MAX, Vec::<f32>::new()));
    let failed = Mutex::new(None::<CoreError>);
    let ch = src.channels;
    // Called in increasing row order (scan lines, increasing, one thread).
    let sample = |x: usize, y: usize, c: usize| -> f32 {
        let mut band = band.lock().unwrap_or_else(|e| e.into_inner());
        let y = y as u32;
        if band.0 == u32::MAX || y < band.0 || y >= band.0 + BAND_ROWS.min(rect.h - band.0) {
            let h = BAND_ROWS.min(rect.h - y);
            match src.read(Rect {
                x0: rect.x0,
                y0: rect.y0 + y,
                w: rect.w,
                h,
            }) {
                Ok(data) => *band = (y, data),
                Err(e) => {
                    failed.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(e);
                    return 0.0;
                }
            }
        }
        band.1[((y - band.0) as usize * rect.w as usize + x) * ch + c]
    };
    let size = (rect.w as usize, rect.h as usize);
    let encoding = Encoding {
        compression: Compression::ZIP16,
        blocks: Blocks::ScanLines,
        line_order: LineOrder::Increasing,
    };
    let written = if ch == 1 {
        let channels = SpecificChannels::build()
            .with_channel("Y")
            .with_pixel_fn(|p: Vec2<usize>| (sample(p.0, p.1, 0),));
        Image::from_layer(Layer::new(
            size,
            LayerAttributes::named("height"),
            encoding,
            channels,
        ))
        .write()
        .non_parallel()
        .to_file(path)
    } else {
        let channels = SpecificChannels::build()
            .with_channel("R")
            .with_channel("G")
            .with_channel("B")
            .with_channel("A")
            .with_pixel_fn(|p: Vec2<usize>| {
                (
                    sample(p.0, p.1, 0),
                    sample(p.0, p.1, 1),
                    sample(p.0, p.1, 2),
                    sample(p.0, p.1, 3),
                )
            });
        Image::from_layer(Layer::new(
            size,
            LayerAttributes::named("colour"),
            encoding,
            channels,
        ))
        .write()
        .non_parallel()
        .to_file(path)
    };
    if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(e);
    }
    written.map_err(|e| CoreError::Image(e.to_string()))
}

/// Write `rect` of `src` as a greyscale (one channel) or RGBA (four) PNG of
/// 16 or 8 bits, a band of rows at a time. `map` turns a stored value into
/// 0..1 (clamped after).
pub fn write_png_streamed(
    src: &RawImage,
    rect: Rect,
    deep: bool,
    map: &(dyn Fn(f32) -> f32 + Sync),
    path: &Path,
) -> Result<()> {
    let png_err = |e: png::EncodingError| CoreError::Image(e.to_string());
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path)?), rect.w, rect.h);
    encoder.set_color(if src.channels == 1 {
        png::ColorType::Grayscale
    } else {
        png::ColorType::Rgba
    });
    encoder.set_depth(if deep {
        png::BitDepth::Sixteen
    } else {
        png::BitDepth::Eight
    });
    // As the `image` crate writes PNGs.
    encoder.set_compression(png::Compression::Balanced);
    encoder.set_filter(png::Filter::Adaptive);
    let mut writer = encoder.write_header().map_err(png_err)?;
    let mut stream = writer.stream_writer().map_err(png_err)?;
    let mut y = 0;
    let mut bytes = Vec::new();
    while y < rect.h {
        let h = BAND_ROWS.min(rect.h - y);
        let data = src.read(Rect {
            x0: rect.x0,
            y0: rect.y0 + y,
            w: rect.w,
            h,
        })?;
        bytes.clear();
        for v in data {
            let v = map(v).clamp(0.0, 1.0);
            if deep {
                bytes.extend(((v * 65535.0 + 0.5) as u16).to_be_bytes());
            } else {
                bytes.push((v * 255.0 + 0.5) as u8);
            }
        }
        stream.write_all(&bytes)?;
        y += h;
    }
    stream.finish().map_err(png_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tiles_share_edges() {
        // 4033 = 2 × (2017 - 1) + 1: two even tiles each way.
        let t = file_tiles(4033, 4033, 2017);
        assert_eq!(t.len(), 4);
        assert_eq!(
            t[1],
            (
                [1, 0],
                Rect {
                    x0: 2016,
                    y0: 0,
                    w: 2017,
                    h: 2017
                }
            )
        );
        assert!(tiles_even(4033, 2017));
        // 4096 isn't: the last tile is 64 samples (sharing its first).
        let t = file_tiles(4096, 10, 2017);
        assert_eq!(t.iter().map(|t| t.1.w).collect::<Vec<_>>(), vec![2017, 2017, 64]);
        assert!(!tiles_even(4096, 2017));
    }

    #[test]
    fn raw_images_round_trip_windows() {
        let dir = std::env::temp_dir().join(format!("ots-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = GridSpec::new(10, 6, [0.0, 0.0], [9.0, 5.0]);
        let raw = RawImage::create(dir.join("a.raw"), 10, 6, 1).unwrap();
        for (i0, w) in [(0, 4), (4, 6)] {
            let win = full.window(i0, 0, w, 6);
            let data: Vec<f32> = (0..win.len())
                .map(|k| (i0 + k as u32 % w) as f32 + 100.0 * (k as u32 / w) as f32)
                .collect();
            raw.write_window(win, &data).unwrap();
        }
        let r = raw
            .read(Rect {
                x0: 2,
                y0: 1,
                w: 5,
                h: 2,
            })
            .unwrap();
        assert_eq!(
            r,
            vec![
                102.0, 103.0, 104.0, 105.0, 106.0, 202.0, 203.0, 204.0, 205.0, 206.0
            ]
        );
        let path = raw.path.clone();
        drop(raw);
        assert!(!path.exists());
    }

    #[test]
    fn streamed_images_read_back() {
        use crate::import::{read_color_image, read_height_image};
        let dir = std::env::temp_dir().join(format!("ots-stream-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // More rows than one band, and a rectangle that isn't the whole image.
        let (w, h) = (37u32, 150u32);
        let full = GridSpec::new(w, h, [0.0, 0.0], [1.0, 1.0]);
        let rect = Rect {
            x0: 3,
            y0: 5,
            w: 30,
            h: 140,
        };
        let value = |i: u32, j: u32, c: usize| ((i * 7 + j * 3 + c as u32 * 11) % 97) as f32 / 96.0;
        for channels in [1usize, 4] {
            let raw = RawImage::create(dir.join(format!("{channels}.raw")), w, h, channels).unwrap();
            let data: Vec<f32> = (0..h)
                .flat_map(|j| (0..w).flat_map(move |i| (0..channels).map(move |c| value(i, j, c))))
                .collect();
            raw.write_window(full, &data).unwrap();
            let expect = |i: u32, j: u32, c: usize| value(rect.x0 + i, rect.y0 + j, c);

            let exr = dir.join(format!("{channels}.exr"));
            write_exr_streamed(&raw, rect, &exr).unwrap();
            let png = dir.join(format!("{channels}.png"));
            write_png_streamed(&raw, rect, true, &|v| v, &png).unwrap();
            if channels == 1 {
                let e = read_height_image(&exr).unwrap();
                let p = read_height_image(&png).unwrap();
                assert_eq!((e.width, e.height), (rect.w, rect.h));
                for j in 0..rect.h {
                    for i in 0..rect.w {
                        let k = (j * rect.w + i) as usize;
                        assert_eq!(e.data[k], expect(i, j, 0));
                        assert!((p.data[k] - expect(i, j, 0)).abs() < 1.0e-4);
                    }
                }
            } else {
                let e = read_color_image(&exr).unwrap();
                let p = read_color_image(&png).unwrap();
                for j in 0..rect.h {
                    for i in 0..rect.w {
                        for c in 0..4 {
                            let k = (j * rect.w + i) as usize * 4 + c;
                            assert_eq!(e.data[k], expect(i, j, c));
                            assert!((p.data[k] - expect(i, j, c)).abs() < 1.0e-4);
                        }
                    }
                }
            }
        }
    }
}
