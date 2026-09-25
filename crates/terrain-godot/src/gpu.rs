//! The GPU device: Godot's RenderingDevice behind `terrain_core::GpuDevice`.
//!
//! A local RenderingDevice only works on the thread that created it, so one
//! dedicated thread owns it and every evaluation job sends it commands over a
//! channel. It uses its own device rather than the renderer's, so compute
//! never stalls the viewport; results come back through `download` (the
//! viewport uploads the preview image as before).
//!
//! There is no device on the Compatibility renderer or in `--headless` runs:
//! then everything runs on the CPU, exactly as before v0.4.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use godot::classes::rendering_device::{ShaderLanguage, ShaderStage, UniformType};
use godot::classes::{RdShaderSource, RdUniform, RenderingDevice, RenderingServer};
use godot::prelude::*;
use terrain_core::error::{CoreError, Result};
use terrain_core::gpu::BufferId;
use terrain_core::{Gpu, GpuDevice, Kernel, Params};

enum Cmd {
    Upload(Vec<f32>, Sender<Result<BufferId>>),
    Alloc(usize, Sender<Result<BufferId>>),
    Download(BufferId, Sender<Result<Vec<f32>>>),
    Free(BufferId),
    Dispatch {
        kernel: Kernel,
        buffers: Vec<BufferId>,
        params: [u8; 128],
        groups: [u32; 3],
        reply: Sender<Result<()>>,
    },
    Flush(Sender<Result<()>>),
    Shutdown(Sender<()>),
}

/// The command side of the GPU thread.
struct RdDevice {
    name: String,
    tx: Mutex<Sender<Cmd>>,
}

fn gpu_err(message: impl Into<String>) -> CoreError {
    CoreError::Gpu(message.into())
}

impl RdDevice {
    fn send(&self, cmd: Cmd) -> Result<()> {
        self.tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(cmd)
            .map_err(|_| gpu_err("the GPU thread has stopped"))
    }

    /// Send a command and wait for its reply.
    fn call<T>(&self, make: impl FnOnce(Sender<Result<T>>) -> Cmd) -> Result<T> {
        let (tx, rx) = channel();
        self.send(make(tx))?;
        rx.recv().map_err(|_| gpu_err("the GPU thread has stopped"))?
    }
}

impl GpuDevice for RdDevice {
    fn name(&self) -> String {
        self.name.clone()
    }
    fn upload(&self, data: &[f32]) -> Result<BufferId> {
        self.call(|r| Cmd::Upload(data.to_vec(), r))
    }
    fn alloc(&self, len: usize) -> Result<BufferId> {
        self.call(|r| Cmd::Alloc(len, r))
    }
    fn download(&self, id: BufferId) -> Result<Vec<f32>> {
        self.call(|r| Cmd::Download(id, r))
    }
    fn free(&self, id: BufferId) {
        // After shutdown there is nothing left to free.
        let _ = self.send(Cmd::Free(id));
    }
    fn dispatch(
        &self,
        kernel: &Kernel,
        buffers: &[BufferId],
        params: &Params,
        groups: [u32; 3],
    ) -> Result<()> {
        self.call(|reply| Cmd::Dispatch {
            kernel: *kernel,
            buffers: buffers.to_vec(),
            params: params.bytes(),
            groups,
            reply,
        })
    }
    fn flush(&self) -> Result<()> {
        self.call(Cmd::Flush)
    }
}

/// State owned by the GPU thread.
struct Worker {
    rd: Gd<RenderingDevice>,
    buffers: HashMap<BufferId, (Rid, usize)>,
    next_id: BufferId,
    /// Compiled kernels by name: (shader, pipeline).
    pipelines: HashMap<&'static str, (Rid, Rid)>,
    /// Work recorded since the last submit.
    recorded: bool,
    /// Resources to free once the recorded work has finished.
    after_sync: Vec<Rid>,
}

impl Worker {
    /// A buffer of `len` floats, filled with `data` if given.
    fn add_buffer(&mut self, data: Option<&[f32]>, len: usize) -> Result<BufferId> {
        let size = (len * 4) as u32;
        let rid = match data {
            Some(data) => {
                let bytes = PackedFloat32Array::from(data).to_byte_array();
                self.rd.storage_buffer_create_ex(size).data(&bytes).done()
            }
            None => self.rd.storage_buffer_create(size),
        };
        if !rid.is_valid() {
            return Err(gpu_err(format!("could not create a {} MB buffer", size >> 20)));
        }
        self.next_id += 1;
        self.buffers.insert(self.next_id, (rid, len));
        Ok(self.next_id)
    }

    fn rid(&self, id: BufferId) -> Result<Rid> {
        self.buffers
            .get(&id)
            .map(|b| b.0)
            .ok_or_else(|| gpu_err(format!("unknown buffer {id}")))
    }

    fn pipeline(&mut self, kernel: &Kernel) -> Result<(Rid, Rid)> {
        if let Some(p) = self.pipelines.get(kernel.name) {
            return Ok(*p);
        }
        let mut source = RdShaderSource::new_gd();
        source.set_language(ShaderLanguage::GLSL);
        source.set_stage_source(ShaderStage::COMPUTE, &kernel.source());
        let spirv = self
            .rd
            .shader_compile_spirv_from_source(&source)
            .ok_or_else(|| gpu_err(format!("kernel '{}' did not compile", kernel.name)))?;
        let error = spirv.get_stage_compile_error(ShaderStage::COMPUTE);
        if !error.is_empty() {
            return Err(gpu_err(format!(
                "kernel '{}' did not compile: {error}",
                kernel.name
            )));
        }
        let shader = self.rd.shader_create_from_spirv(&spirv);
        if !shader.is_valid() {
            return Err(gpu_err(format!(
                "kernel '{}' was rejected by the driver",
                kernel.name
            )));
        }
        let pipeline = self.rd.compute_pipeline_create(shader);
        if !pipeline.is_valid() {
            self.rd.free_rid(shader);
            return Err(gpu_err(format!("no pipeline for kernel '{}'", kernel.name)));
        }
        self.pipelines.insert(kernel.name, (shader, pipeline));
        Ok((shader, pipeline))
    }

    fn dispatch(
        &mut self,
        kernel: &Kernel,
        buffers: &[BufferId],
        params: &[u8; 128],
        groups: [u32; 3],
    ) -> Result<()> {
        let (shader, pipeline) = self.pipeline(kernel)?;
        let mut uniforms: Array<Gd<RdUniform>> = Array::new();
        for (binding, id) in buffers.iter().enumerate() {
            let mut u = RdUniform::new_gd();
            u.set_uniform_type(UniformType::STORAGE_BUFFER);
            u.set_binding(binding as i32);
            u.add_id(self.rid(*id)?);
            uniforms.push(&u);
        }
        let set = self.rd.uniform_set_create(&uniforms, shader, 0);
        if !set.is_valid() {
            return Err(gpu_err(format!("bad buffers for kernel '{}'", kernel.name)));
        }
        let push = PackedByteArray::from(&params[..]);
        let list = self.rd.compute_list_begin();
        self.rd.compute_list_bind_compute_pipeline(list, pipeline);
        self.rd.compute_list_bind_uniform_set(list, set, 0);
        self.rd
            .compute_list_set_push_constant(list, &push, push.len() as u32);
        self.rd
            .compute_list_dispatch(list, groups[0], groups[1], groups[2]);
        self.rd.compute_list_end();
        self.recorded = true;
        self.after_sync.push(set);
        Ok(())
    }

    /// Submit recorded work, wait for it, then free what was waiting on it.
    fn flush(&mut self) {
        if self.recorded {
            self.rd.submit();
            self.rd.sync();
            self.recorded = false;
        }
        for rid in self.after_sync.drain(..) {
            self.rd.free_rid(rid);
        }
    }

    fn download(&mut self, id: BufferId) -> Result<Vec<f32>> {
        self.flush();
        let (rid, len) = *self
            .buffers
            .get(&id)
            .ok_or_else(|| gpu_err(format!("unknown buffer {id}")))?;
        let floats = self.rd.buffer_get_data(rid).to_float32_array();
        if floats.len() < len {
            return Err(gpu_err("reading a buffer back returned too little data"));
        }
        Ok(floats.as_slice()[..len].to_vec())
    }

    fn free(&mut self, id: BufferId) {
        if let Some((rid, _)) = self.buffers.remove(&id) {
            if self.recorded {
                self.after_sync.push(rid);
            } else {
                self.rd.free_rid(rid);
            }
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Upload(data, reply) => {
                    let _ = reply.send(self.add_buffer(Some(&data), data.len()));
                }
                Cmd::Alloc(len, reply) => {
                    let _ = reply.send(self.add_buffer(None, len));
                }
                Cmd::Download(id, reply) => {
                    let _ = reply.send(self.download(id));
                }
                Cmd::Free(id) => self.free(id),
                Cmd::Dispatch {
                    kernel,
                    buffers,
                    params,
                    groups,
                    reply,
                } => {
                    let _ = reply.send(self.dispatch(&kernel, &buffers, &params, groups));
                }
                Cmd::Flush(reply) => {
                    self.flush();
                    let _ = reply.send(Ok(()));
                }
                Cmd::Shutdown(done) => {
                    self.shutdown();
                    let _ = done.send(());
                    return;
                }
            }
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.flush();
        let buffers: Vec<Rid> = self.buffers.drain().map(|(_, (rid, _))| rid).collect();
        for rid in buffers {
            self.rd.free_rid(rid);
        }
        let pipelines: Vec<(Rid, Rid)> = self.pipelines.drain().map(|(_, p)| p).collect();
        for (shader, pipeline) in pipelines {
            self.rd.free_rid(pipeline);
            self.rd.free_rid(shader);
        }
        self.rd.clone().free();
    }
}

/// The device, once started: `Ok` or the reason there is none.
static DEVICE: OnceLock<std::result::Result<Gpu, String>> = OnceLock::new();
static SENDER: OnceLock<Mutex<Sender<Cmd>>> = OnceLock::new();
static FORCE_CPU: AtomicBool = AtomicBool::new(false);
static BUILDS_ON_GPU: AtomicBool = AtomicBool::new(false);

/// Start the GPU thread and wait until the device is ready (or known to be
/// missing). Call from worker threads; the main thread uses [`start`].
pub fn device() -> &'static std::result::Result<Gpu, String> {
    DEVICE.get_or_init(|| {
        let (tx, rx) = channel::<Cmd>();
        let (ready_tx, ready_rx) = channel::<std::result::Result<String, String>>();
        let spawned = std::thread::Builder::new()
            .name("terrain-gpu".into())
            .spawn(move || {
                let rd = RenderingServer::singleton().create_local_rendering_device();
                let Some(rd) = rd else {
                    let _ = ready_tx.send(Err(
                        "no compute device (Compatibility renderer or headless mode)".into()
                    ));
                    return;
                };
                let name = format!("{} ({})", rd.get_device_name(), rd.get_device_vendor_name());
                let mut worker = Worker {
                    rd,
                    buffers: HashMap::new(),
                    next_id: 0,
                    pipelines: HashMap::new(),
                    recorded: false,
                    after_sync: Vec::new(),
                };
                // Compile every kernel now, so a broken one shows up at once
                // and the first preview doesn't wait for the compiler.
                for kernel in terrain_nodes::kernels::all() {
                    if let Err(e) = worker.pipeline(&kernel) {
                        let _ = ready_tx.send(Err(e.to_string()));
                        worker.shutdown();
                        return;
                    }
                }
                let _ = ready_tx.send(Ok(name));
                worker.run(rx);
            });
        if let Err(e) = spawned {
            return Err(format!("could not start the GPU thread: {e}"));
        }
        let name = ready_rx
            .recv()
            .unwrap_or_else(|_| Err("the GPU thread stopped while starting".into()))?;
        let _ = SENDER.set(Mutex::new(tx.clone()));
        Ok(Gpu::new(Arc::new(RdDevice {
            name,
            tx: Mutex::new(tx),
        })))
    })
}

/// Start the device in the background (non-blocking).
pub fn start() {
    if DEVICE.get().is_none() {
        std::thread::spawn(|| {
            let _ = device();
        });
    }
}

/// The device if it has finished starting, without waiting.
pub fn status() -> Option<&'static std::result::Result<Gpu, String>> {
    DEVICE.get()
}

/// The device for previews, unless "Force CPU" is on or there is none.
pub fn for_preview() -> Option<Gpu> {
    if FORCE_CPU.load(Ordering::Relaxed) {
        return None;
    }
    device().as_ref().ok().cloned()
}

/// The device for builds and exports: only when builds on the GPU are
/// enabled (by default builds are bit-exact CPU results).
pub fn for_build() -> Option<Gpu> {
    if !BUILDS_ON_GPU.load(Ordering::Relaxed) {
        return None;
    }
    for_preview()
}

pub fn set_force_cpu(on: bool) {
    FORCE_CPU.store(on, Ordering::Relaxed);
}

pub fn force_cpu() -> bool {
    FORCE_CPU.load(Ordering::Relaxed)
}

pub fn set_builds_on_gpu(on: bool) {
    BUILDS_ON_GPU.store(on, Ordering::Relaxed);
}

pub fn builds_on_gpu() -> bool {
    BUILDS_ON_GPU.load(Ordering::Relaxed)
}

/// Free every GPU resource and the device before Godot shuts down.
pub fn shutdown() {
    let Some(sender) = SENDER.get() else { return };
    let (tx, rx) = channel();
    let sent = sender
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Cmd::Shutdown(tx));
    if sent.is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }
}
