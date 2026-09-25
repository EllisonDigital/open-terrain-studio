use super::*;
use terrain_core::NodeKind;

/// Conservative, slope-limited diffusion of material above the talus angle.
pub struct Thermal {
    schema: NodeSchema,
}
impl Default for Thermal {
    fn default() -> Self {
        Self {
            schema: schema(
                "simulate.thermal",
                "Thermal erosion",
                "Moves loose material down slopes steeper than the talus angle. Strength blocks transfer; hardness resists shedding.",
                &[("debris", "Debris / Talus")],
                vec![
                    duration(),
                    ParamDef::float("talus_angle_deg", "Talus angle", 35.0, 0.0, 89.0).unit("°"),
                    ParamDef::float("diffusivity_m2_s", "Transport rate", 10.0, 0.0, 100.0)
                        .unit("m²/s")
                        .describe("Diffusivity of loose material above the talus angle."),
                ],
            ),
        }
    }
}
impl NodeKind for Thermal {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let d = Domain::new(ctx)?;
        let diffusivity = ctx.f32("diffusivity_m2_s");
        let spacing = d.distance[0].min(d.distance[2]);
        let max_dt = if diffusivity > 0.0 {
            (0.2 * spacing * spacing / diffusivity).min(1.0)
        } else {
            1.0
        };
        let (steps, dt) = d.steps(ctx.f64("duration_s"), max_dt, ctx)?;
        let talus = libm::tanf(ctx.f32("talus_angle_deg") * (std::f32::consts::PI / 180.0));
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
}
