use super::*;
use crate::kernels;
use terrain_core::{Gpu, NodeKind, Params};

/// Time steps per GPU submission: short enough that no submission runs long
/// enough for the OS to reset the driver, long enough to keep the GPU busy.
const STEPS_PER_SUBMIT: usize = 64;

/// Conservative, slope-limited diffusion of material above the talus angle.
pub struct Thermal {
    schema: NodeSchema,
}
impl Default for Thermal {
    fn default() -> Self {
        let mut schema = schema(
            "simulate.thermal",
            "Thermal Erosion",
            "Moves loose material down slopes steeper than the talus angle. Strength blocks transfer; hardness resists shedding.",
            &[("debris", "Debris / Talus")],
            vec![
                duration(),
                ParamDef::float("talus_angle_deg", "Talus angle", 35.0, 0.0, 89.0).unit("°"),
                ParamDef::float("diffusivity_m2_s", "Transport rate", 10.0, 0.0, 100.0)
                    .unit("m²/s")
                    .describe("Diffusivity of loose material above the talus angle."),
            ],
        );
        schema.gpu = true;
        Self { schema }
    }
}

/// Talus slope, diffusivity and the time steps shared by both implementations.
fn setup(ctx: &EvalContext, dx: f32, dy: f32) -> Result<(f32, f32, usize, f32)> {
    let diffusivity = ctx.f32("diffusivity_m2_s");
    let spacing = dx.min(dy);
    let max_dt = if diffusivity > 0.0 {
        (0.2 * spacing * spacing / diffusivity).min(1.0)
    } else {
        1.0
    };
    let (steps, dt) = time_steps(ctx.f64("duration_s"), max_dt, ctx)?;
    let talus = libm::tanf(ctx.f32("talus_angle_deg") * (std::f32::consts::PI / 180.0));
    Ok((talus, diffusivity, steps, dt))
}
impl NodeKind for Thermal {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let d = Domain::new(ctx)?;
        let (talus, diffusivity, steps, dt) = setup(ctx, d.distance[0], d.distance[2])?;
        let mut height = d.terrain.data.clone();
        let mut next = height.clone();
        let mut flux = vec![[0.0f32; 4]; d.len];
        ctx.report_progress(0.0);
        for step in 0..steps {
            check_cancel(ctx)?;
            flux.par_iter_mut().enumerate().for_each(|(i, out)| {
                for (k, j) in neighbours(i, d.width, d.len).into_iter().enumerate() {
                    let excess = (height[i] - height[j] - talus * d.distance[k]).max(0.0);
                    // Face strength preserves a zero-mask cell exactly, including deposition.
                    out[k] = excess * diffusivity * dt / (d.distance[k] * d.distance[k])
                        * d.strength(i).min(d.strength(j))
                        * d.softness(i);
                }
            });
            check_cancel(ctx)?;
            next.par_iter_mut().enumerate().for_each(|(i, out)| {
                let mut delta = 0.0;
                for (k, j) in neighbours(i, d.width, d.len).into_iter().enumerate() {
                    let incoming = if j == i { 0.0 } else { flux[j][OPPOSITE[k]] };
                    delta += incoming - flux[i][k];
                }
                *out = height[i] + delta;
            });
            std::mem::swap(&mut height, &mut next);
            if step % (steps / 100).max(1) == 0 {
                ctx.report_progress((step + 1) as f32 / steps as f32 * 0.99);
            }
        }
        check_cancel(ctx)?;
        let debris = height
            .par_iter()
            .zip(&d.terrain.data)
            .map(|(a, b)| (a - b).max(0.0))
            .collect();
        let mut outputs = height_out(height, ctx);
        outputs.insert("debris".into(), mask_out(debris, 1.0, ctx));
        ctx.report_progress(1.0);
        Ok(outputs)
    }

    fn evaluate_gpu(&self, ctx: &EvalContext, gpu: &Gpu) -> Result<Outputs> {
        let s = ctx.spec;
        let [dx, dy] = s.cell_size_m().map(|v| v as f32);
        // Unusual grids: let the CPU version report the problem.
        if !(0.01..=1.0e8).contains(&dx) || !(0.01..=1.0e8).contains(&dy) {
            return Err(CoreError::GpuUnsupported);
        }
        let (talus, diffusivity, steps, dt) = setup(ctx, dx, dy)?;
        let terrain = gpu.input(ctx, "in")?;
        let mask = gpu.optional_input(ctx, "mask")?;
        let hardness = gpu.optional_input(ctx, "hardness")?;
        let dummy = gpu.dummy()?;
        let buffers = [gpu.alloc(s.len())?, gpu.alloc(s.len())?];
        let p = Params::grid(s)
            .u(5, mask.is_some() as u32)
            .u(6, hardness.is_some() as u32)
            .f(0, dx)
            .f(1, dy)
            .f(2, talus)
            .f(3, diffusivity)
            .f(4, dt);
        let (mask, hardness) = (
            mask.as_ref().unwrap_or(&dummy),
            hardness.as_ref().unwrap_or(&dummy),
        );
        ctx.report_progress(0.0);
        let mut height = terrain.clone();
        for step in 0..steps {
            let next = &buffers[step % 2];
            gpu.dispatch_grid(
                &kernels::THERMAL,
                &[&height, next, mask, hardness, &dummy],
                &p.u(4, 0),
                s,
            )?;
            height = next.clone();
            if (step + 1) % STEPS_PER_SUBMIT == 0 {
                gpu.flush()?;
                check_cancel(ctx)?;
                ctx.report_progress((step + 1) as f32 / steps as f32 * 0.99);
            }
        }
        let debris = gpu.alloc(s.len())?;
        gpu.dispatch_grid(
            &kernels::THERMAL,
            &[&height, &debris, mask, hardness, &terrain],
            &p.u(4, 1),
            s,
        )?;
        gpu.flush()?;
        check_cancel(ctx)?;
        ctx.report_progress(1.0);
        Ok(Outputs::from([
            ("height".into(), gpu.value(PortType::Heightfield, s, height)),
            ("debris".into(), gpu.value(PortType::Mask, s, debris)),
        ]))
    }
}
