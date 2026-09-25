//! GPU compute (ARCHITECTURE.md §6).
//!
//! This crate has no Godot dependency, so the GPU is reached through the
//! small [`GpuDevice`] trait: storage buffers of `f32`, GLSL compute kernels
//! and a fixed 128-byte block of parameters. `terrain-godot` implements it
//! with Godot's RenderingDevice.
//!
//! A node with a kernel sets `NodeSchema::gpu` and implements
//! [`NodeKind::evaluate_gpu`](crate::NodeKind::evaluate_gpu). Its outputs are
//! [`Value::Gpu`]: they stay on the GPU, so the next GPU node reads them
//! directly, and are only read back when a CPU node, the preview or an
//! export first asks for [`Value::grid`].
//!
//! GPU results match the CPU within a per-node tolerance, not bit for bit
//! (kernels use `f32` where the CPU uses `f64`), so they are cached under
//! their own keys and never mixed with CPU results.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use crate::error::{CoreError, Result};
use crate::grid::{Grid, GridSpec};
use crate::node::{EvalContext, PortType, Value, param_port_key};

/// Handle of a buffer on a device.
pub type BufferId = u64;

/// A compute device. Implementations must be usable from any thread.
///
/// Buffers hold `f32`s and are bound to kernels in the order given, as
/// `layout(set = 0, binding = n) buffer` blocks. Dispatches run in the order
/// they are recorded; [`GpuDevice::download`] and [`GpuDevice::flush`] wait
/// for everything recorded before them.
pub trait GpuDevice: Send + Sync {
    /// Human-readable device name, e.g. "Intel(R) Graphics (Vulkan)".
    fn name(&self) -> String;
    /// A new buffer holding `data`.
    fn upload(&self, data: &[f32]) -> Result<BufferId>;
    /// A new buffer of `len` floats. Its contents are undefined: kernels
    /// must write every element they later read.
    fn alloc(&self, len: usize) -> Result<BufferId>;
    fn download(&self, id: BufferId) -> Result<Vec<f32>>;
    /// Release a buffer (called when its [`GpuBuffer`] is dropped).
    fn free(&self, id: BufferId);
    /// Record one dispatch of `groups` work groups.
    fn dispatch(
        &self,
        kernel: &Kernel,
        buffers: &[BufferId],
        params: &Params,
        groups: [u32; 3],
    ) -> Result<()>;
    /// Submit everything recorded and wait for it. Long simulations call this
    /// between batches of steps, so no single submission runs long enough for
    /// the OS to reset the driver (about 2 s on Windows).
    fn flush(&self) -> Result<()>;
}

/// A GLSL compute shader: `#version 450` followed by `parts`, joined in order
/// (shared code first). `name` identifies it; compiled once per device.
#[derive(Clone, Copy, Debug)]
pub struct Kernel {
    pub name: &'static str,
    pub parts: &'static [&'static str],
}

impl Kernel {
    /// The complete shader source.
    pub fn source(&self) -> String {
        let mut s = String::from("#version 450\n");
        for part in self.parts {
            s.push_str(part);
            s.push('\n');
        }
        s
    }
}

/// Shared GLSL: the parameter block and grid helpers. Every kernel starts with it.
pub const COMMON_GLSL: &str = include_str!("../../../shaders/common/common.glsl");

/// Kernel parameters: 16 unsigned integers then 16 floats, 128 bytes, the
/// push-constant block declared in `shaders/common/common.glsl`. By
/// convention `u[0]` and `u[1]` are the grid width and height.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    pub u: [u32; 16],
    pub f: [f32; 16],
}

impl Params {
    /// Parameters for a kernel over `spec`: width and height set.
    pub fn grid(spec: GridSpec) -> Self {
        let mut p = Self::default();
        p.u[0] = spec.width;
        p.u[1] = spec.height;
        p
    }

    /// Set `u[i]`.
    pub fn u(mut self, i: usize, v: u32) -> Self {
        self.u[i] = v;
        self
    }

    /// Set `f[i]`.
    pub fn f(mut self, i: usize, v: f32) -> Self {
        self.f[i] = v;
        self
    }

    /// A 64-bit value (e.g. a seed) as two `u`s, low word first.
    pub fn u64(self, i: usize, v: u64) -> Self {
        self.u(i, v as u32).u(i + 1, (v >> 32) as u32)
    }

    /// The push-constant bytes (little-endian, as every supported GPU is).
    pub fn bytes(&self) -> [u8; 128] {
        let mut out = [0u8; 128];
        for (i, v) in self.u.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, v) in self.f.iter().enumerate() {
            out[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// A buffer on a device, freed when dropped.
pub struct GpuBuffer {
    id: BufferId,
    len: usize,
    device: Arc<dyn GpuDevice>,
}

impl GpuBuffer {
    pub fn id(&self) -> BufferId {
        self.id
    }
    /// Length in floats.
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn download(&self) -> Result<Vec<f32>> {
        self.device.download(self.id)
    }
    pub fn device(&self) -> &Arc<dyn GpuDevice> {
        &self.device
    }
}

impl Drop for GpuBuffer {
    fn drop(&mut self) {
        self.device.free(self.id);
    }
}

impl fmt::Debug for GpuBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GpuBuffer#{}[{}]", self.id, self.len)
    }
}

/// A grid whose data lives on the GPU, read back to the CPU on first use.
pub struct GpuGrid {
    pub spec: GridSpec,
    pub buffer: Arc<GpuBuffer>,
    cpu: OnceLock<Arc<Grid>>,
}

impl GpuGrid {
    pub fn new(spec: GridSpec, buffer: Arc<GpuBuffer>) -> Self {
        Self {
            spec,
            buffer,
            cpu: OnceLock::new(),
        }
    }

    /// The data on the CPU (downloaded once). A device that can no longer
    /// read back is unrecoverable here: this panics, and the job running it
    /// reports an internal error.
    pub fn cpu(&self) -> &Arc<Grid> {
        self.cpu.get_or_init(|| {
            let data = self
                .buffer
                .download()
                .unwrap_or_else(|e| panic!("reading a result back from the GPU failed: {e}"));
            assert_eq!(data.len(), self.spec.len(), "GPU buffer has the wrong size");
            Arc::new(Grid {
                spec: self.spec,
                data,
            })
        })
    }
}

impl fmt::Debug for GpuGrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GpuGrid({:?}, {:?})", self.spec, self.buffer)
    }
}

/// Counters for the status bar and tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuStats {
    /// Node evaluations that ran on the GPU.
    pub nodes: u64,
    /// Node evaluations that failed on the GPU and ran on the CPU instead.
    pub fallbacks: u64,
    /// The latest GPU failure, if any.
    pub last_error: Option<String>,
}

/// A device plus the helpers kernels use. Cheap to clone.
#[derive(Clone)]
pub struct Gpu {
    device: Arc<dyn GpuDevice>,
    stats: Arc<Mutex<GpuStats>>,
}

/// A drivable parameter on the GPU: `value` times an optional mask, clamped
/// to `min..max` (see [`crate::Field`]).
pub struct GpuField {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub mask: Option<Arc<GpuBuffer>>,
}

impl GpuField {
    /// `1` if a mask drives the value, else `0` (for a kernel flag).
    pub fn driven(&self) -> u32 {
        self.mask.is_some() as u32
    }
}

const GROUP: u32 = 8;

impl Gpu {
    pub fn new(device: Arc<dyn GpuDevice>) -> Self {
        Self {
            device,
            stats: Arc::default(),
        }
    }

    pub fn name(&self) -> String {
        self.device.name()
    }

    pub fn stats(&self) -> GpuStats {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(crate) fn record_node(&self) {
        self.stats.lock().unwrap_or_else(|e| e.into_inner()).nodes += 1;
    }

    pub(crate) fn record_fallback(&self, error: &CoreError) {
        let mut s = self.stats.lock().unwrap_or_else(|e| e.into_inner());
        s.fallbacks += 1;
        s.last_error = Some(error.to_string());
    }

    fn wrap(&self, id: BufferId, len: usize) -> Arc<GpuBuffer> {
        Arc::new(GpuBuffer {
            id,
            len,
            device: self.device.clone(),
        })
    }

    pub fn upload(&self, data: &[f32]) -> Result<Arc<GpuBuffer>> {
        Ok(self.wrap(self.device.upload(data)?, data.len()))
    }

    /// A buffer of `len` floats with undefined contents (see [`GpuDevice::alloc`]).
    pub fn alloc(&self, len: usize) -> Result<Arc<GpuBuffer>> {
        Ok(self.wrap(self.device.alloc(len.max(1))?, len.max(1)))
    }

    /// A one-float buffer for an unused binding (e.g. an undriven parameter).
    pub fn dummy(&self) -> Result<Arc<GpuBuffer>> {
        self.alloc(1)
    }

    pub fn dispatch(
        &self,
        kernel: &Kernel,
        buffers: &[&GpuBuffer],
        params: &Params,
        groups: [u32; 3],
    ) -> Result<()> {
        let ids: Vec<BufferId> = buffers.iter().map(|b| b.id).collect();
        self.device.dispatch(kernel, &ids, params, groups)
    }

    /// Dispatch one invocation per cell of `spec` (kernels use 8 × 8 groups).
    pub fn dispatch_grid(
        &self,
        kernel: &Kernel,
        buffers: &[&GpuBuffer],
        params: &Params,
        spec: GridSpec,
    ) -> Result<()> {
        self.dispatch(
            kernel,
            buffers,
            params,
            [spec.width.div_ceil(GROUP), spec.height.div_ceil(GROUP), 1],
        )
    }

    /// Dispatch `n` invocations of a kernel with 64-wide groups.
    pub fn dispatch_lines(
        &self,
        kernel: &Kernel,
        buffers: &[&GpuBuffer],
        params: &Params,
        n: u32,
    ) -> Result<()> {
        self.dispatch(kernel, buffers, params, [n.div_ceil(64), 1, 1])
    }

    pub fn flush(&self) -> Result<()> {
        self.device.flush()
    }

    fn same_device(&self, buffer: &GpuBuffer) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.device), Arc::as_ptr(&buffer.device))
    }

    /// The data of `value` on this device: its own buffer if it is already
    /// here, otherwise an upload of its CPU grid.
    pub fn buffer_of(&self, value: &Value) -> Result<Arc<GpuBuffer>> {
        match value {
            Value::Gpu(_, g) if self.same_device(&g.buffer) => Ok(g.buffer.clone()),
            other => self.upload(&other.grid().data),
        }
    }

    /// A required input on the GPU.
    pub fn input(&self, ctx: &EvalContext, key: &str) -> Result<Arc<GpuBuffer>> {
        match ctx.input(key) {
            Some(v) => self.buffer_of(v),
            None => Err(CoreError::MissingInput {
                node: ctx.node_id.into(),
                port: key.into(),
            }),
        }
    }

    /// An optional input on the GPU, `None` if unconnected.
    pub fn optional_input(&self, ctx: &EvalContext, key: &str) -> Result<Option<Arc<GpuBuffer>>> {
        ctx.input(key).map(|v| self.buffer_of(v)).transpose()
    }

    /// A drivable parameter (see [`EvalContext::field`]).
    pub fn field(&self, ctx: &EvalContext, key: &str) -> Result<GpuField> {
        let value = ctx.f32(key);
        let (min, max) = ctx.param_range(key);
        let mask = ctx
            .input(&param_port_key(key))
            .map(|v| self.buffer_of(v))
            .transpose()?;
        Ok(GpuField {
            value,
            min,
            max,
            mask,
        })
    }

    /// The mask of `field`, or a dummy buffer to bind in its place.
    pub fn field_buffer(&self, field: &GpuField) -> Result<Arc<GpuBuffer>> {
        match &field.mask {
            Some(m) => Ok(m.clone()),
            None => self.dummy(),
        }
    }

    pub fn value(&self, ty: PortType, spec: GridSpec, buffer: Arc<GpuBuffer>) -> Value {
        Value::Gpu(ty, Arc::new(GpuGrid::new(spec, buffer)))
    }

    /// `v × scale + offset` for every sample.
    pub fn affine(&self, spec: GridSpec, src: &GpuBuffer, scale: f32, offset: f32) -> Result<Arc<GpuBuffer>> {
        let out = self.alloc(spec.len())?;
        let p = Params::grid(spec).f(0, scale).f(1, offset);
        self.dispatch_grid(&AFFINE, &[src, &out], &p, spec)?;
        Ok(out)
    }

    /// Gaussian blur with standard deviation `sigma_m` metres, edges
    /// repeated: the GPU version of [`crate::ops::gaussian_blur`], using the
    /// same kernels (exact taps up to σ = 6 cells, three box blurs above).
    pub fn gaussian_blur(
        &self,
        spec: GridSpec,
        src: &Arc<GpuBuffer>,
        sigma_m: f64,
    ) -> Result<Arc<GpuBuffer>> {
        let cell = spec.cell_size_m();
        let rows = self.blur_axis(spec, src, sigma_m / cell[0], 0)?;
        self.blur_axis(spec, &rows, sigma_m / cell[1], 1)
    }

    /// Blur along x (`axis` 0) or y (1) by `sigma` cells.
    fn blur_axis(
        &self,
        spec: GridSpec,
        src: &Arc<GpuBuffer>,
        sigma: f64,
        axis: u32,
    ) -> Result<Arc<GpuBuffer>> {
        let n = if axis == 0 { spec.width } else { spec.height };
        if sigma < 0.2 || n < 2 {
            return Ok(src.clone());
        }
        if sigma <= 6.0 {
            let (r, weights) = crate::ops::gaussian_kernel(sigma);
            let weights: Vec<f32> = weights.iter().map(|&w| w as f32).collect();
            let wbuf = self.upload(&weights)?;
            let out = self.alloc(spec.len())?;
            let p = Params::grid(spec).u(2, axis).u(3, r as u32);
            self.dispatch_grid(&BLUR_TAPS, &[src, &out, &wbuf], &p, spec)?;
            return Ok(out);
        }
        let lines = if axis == 0 { spec.height } else { spec.width };
        let mut cur = src.clone();
        for size in crate::ops::box_sizes(sigma, 3) {
            let out = self.alloc(spec.len())?;
            let p = Params::grid(spec).u(2, axis).u(3, (size / 2) as u32);
            self.dispatch_lines(&BLUR_BOX, &[&cur, &out], &p, lines)?;
            cur = out;
        }
        Ok(cur)
    }
}

/// `out = in × f[0] + f[1]`.
pub const AFFINE: Kernel = Kernel {
    name: "core.affine",
    parts: &[COMMON_GLSL, include_str!("../../../shaders/core/affine.comp")],
};

/// Gaussian blur along one axis with explicit weights.
pub const BLUR_TAPS: Kernel = Kernel {
    name: "core.blur_taps",
    parts: &[COMMON_GLSL, include_str!("../../../shaders/core/blur_taps.comp")],
};

/// Box blur along one axis, one invocation per row or column.
pub const BLUR_BOX: Kernel = Kernel {
    name: "core.blur_box",
    parts: &[COMMON_GLSL, include_str!("../../../shaders/core/blur_box.comp")],
};

/// Every kernel in this crate (for warming up and self-tests).
pub const KERNELS: &[Kernel] = &[AFFINE, BLUR_TAPS, BLUR_BOX];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_layout() {
        let b = Params::default()
            .u(1, 7)
            .f(0, 1.5)
            .u64(2, 0x1122_3344_5566_7788)
            .bytes();
        assert_eq!(&b[4..8], &7u32.to_le_bytes());
        assert_eq!(&b[8..12], &0x5566_7788u32.to_le_bytes());
        assert_eq!(&b[12..16], &0x1122_3344u32.to_le_bytes());
        assert_eq!(&b[64..68], &1.5f32.to_le_bytes());
    }

    #[test]
    fn kernel_sources_start_with_version() {
        for k in KERNELS {
            let s = k.source();
            assert!(s.starts_with("#version 450\n"), "{}", k.name);
            assert!(s.contains("void main()"), "{}", k.name);
        }
    }
}
