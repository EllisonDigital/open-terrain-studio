use super::*;
use terrain_core::NodeKind;

/// Grid water/sediment transport with bounded kinematic downhill velocity.
/// Each stage reads immutable state; no atomics or parallel floating sums.
pub struct Hydraulic {
    schema: NodeSchema,
}
impl Default for Hydraulic {
    fn default() -> Self {
        Self {
            schema: schema(
                "simulate.hydraulic",
                "Hydraulic erosion",
                "Rain transports sediment downhill. Strength protects terrain; hardness resists wear. Boundaries are closed.",
                &[
                    ("flow", "Flow"),
                    ("wear", "Wear"),
                    ("deposition", "Deposition"),
                    ("sediment", "Sediment"),
                ],
                vec![
                    duration(),
                    ParamDef::float("rainfall_m_s", "Rainfall", 0.05, 0.0, 1.0)
                        .unit("m/s")
                        .describe("Accelerated rainfall depth per second, uniform across the terrain."),
                    ParamDef::float("rock_softness", "Rock softness", 0.5, 0.0, 1.0).describe(
                        "Fraction of the capacity deficit eroded per second; multiplied by 1 - hardness.",
                    ),
                    ParamDef::float("sediment_capacity", "Sediment capacity", 2.0, 0.0, 10.0)
                        .unit("s/m")
                        .describe("Capacity = this coefficient × speed × downhill slope × water depth."),
                    ParamDef::float("deposition_rate", "Deposition", 0.5, 0.0, 1.0)
                        .unit("1/s")
                        .describe("Relaxation rate of excess suspended sediment onto the ground."),
                    ParamDef::float("evaporation_rate", "Evaporation", 0.02, 0.0, 1.0).unit("1/s"),
                    ParamDef::float("downcutting", "Downcutting", 1.0, 0.0, 4.0)
                        .describe("Multiplier on bedrock erosion rate. Zero stops new wear."),
                ],
            ),
        }
    }
}
#[derive(Clone, Copy, Default)]
struct Cell {
    height: f32,
    water: f32,
    sediment: f32,
    flow: f32,
    wear: f32,
    deposition: f32,
}
const MAX_SPEED: f32 = 10.0; // m/s on each face; bounds transport CFL.
impl NodeKind for Hydraulic {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        let d = Domain::new(ctx)?;
        let max_dt = (d.distance[0].min(d.distance[2]) / (4.0 * MAX_SPEED)).min(0.5);
        let (steps, dt) = d.steps(ctx.f64("duration_s"), max_dt, ctx)?;
        let rain = ctx.f32("rainfall_m_s") * dt;
        let evaporation = libm::expf(-ctx.f32("evaporation_rate") * dt);
        let erosion_rate = ctx.f32("rock_softness") * ctx.f32("downcutting");
        let deposition_rate = ctx.f32("deposition_rate");
        let capacity = ctx.f32("sediment_capacity");
        let rates: Vec<[f32; 2]> = (0..d.len)
            .into_par_iter()
            .map(|i| {
                [
                    1.0 - libm::expf(-erosion_rate * d.strength(i) * d.softness(i) * dt),
                    1.0 - libm::expf(-deposition_rate * d.strength(i) * dt),
                ]
            })
            .collect();
        let mut cells: Vec<Cell> = d
            .terrain
            .data
            .par_iter()
            .map(|&height| Cell {
                height,
                ..Cell::default()
            })
            .collect();
        let mut next = cells.clone();
        let mut flux = vec![[0.0f32; 4]; d.len]; // outgoing water depth this step
        ctx.report_progress(0.0);
        for step in 0..steps {
            check_cancel(ctx)?;
            flux.par_iter_mut().enumerate().for_each(|(i, out)| {
                let c = cells[i];
                let water = c.water + rain;
                for (k, j) in neighbours(i, d.width, d.len).into_iter().enumerate() {
                    let head = (c.height - cells[j].height) + (c.water - cells[j].water);
                    let slope = head.max(0.0) / d.distance[k];
                    let speed = MAX_SPEED * slope / (1.0 + slope);
                    // At most one quarter of available water per face.
                    // Equalising-head limiter prevents overshoot near pools.
                    out[k] = (water * speed * dt / d.distance[k]).min(head.max(0.0) * 0.125);
                }
            });
            check_cancel(ctx)?;
            next.par_iter_mut().enumerate().for_each(|(i, out)| {
                let c = cells[i];
                let water_before = c.water + rain;
                let concentration = if water_before > 0.0 {
                    c.sediment / water_before
                } else {
                    0.0
                };
                let mut water_delta = 0.0;
                let mut sediment_delta = 0.0;
                let mut discharge = 0.0;
                let mut slope = 0.0f32;
                let mut relief = 0.0f32;
                for (k, j) in neighbours(i, d.width, d.len).into_iter().enumerate() {
                    let outgoing = flux[i][k];
                    let incoming = if j == i { 0.0 } else { flux[j][OPPOSITE[k]] };
                    water_delta += incoming - outgoing;
                    let neighbour_water = cells[j].water + rain;
                    let neighbour_concentration = if neighbour_water > 0.0 {
                        cells[j].sediment / neighbour_water
                    } else {
                        0.0
                    };
                    sediment_delta += incoming * neighbour_concentration - outgoing * concentration;
                    discharge += outgoing * d.distance[k];
                    let drop = (c.height - cells[j].height).max(0.0);
                    slope = slope.max(drop / d.distance[k]);
                    relief = relief.max(drop);
                }
                let water = (water_before + water_delta).max(0.0) * evaporation;
                let mut sediment = (c.sediment + sediment_delta).max(0.0);
                let speed = if water_before > 0.0 && dt > 0.0 {
                    discharge / (water_before * dt)
                } else {
                    0.0
                };
                let target = capacity * speed * slope * water;
                let wear = ((target - sediment).max(0.0) * rates[i][0]).min(relief * 0.25);
                let deposited = (sediment - target).max(0.0) * rates[i][1];
                sediment += wear - deposited;
                *out = Cell {
                    height: c.height + (deposited - wear),
                    water,
                    sediment,
                    flow: c.flow + discharge,
                    wear: c.wear + wear,
                    deposition: c.deposition + deposited,
                };
            });
            std::mem::swap(&mut cells, &mut next);
            // Bound channel traffic independently of spatial resolution.
            if step % (steps / 100).max(1) == 0 {
                ctx.report_progress((step + 1) as f32 / steps as f32 * 0.99);
            }
        }
        check_cancel(ctx)?;
        let mut outputs = height_out(cells.par_iter().map(|c| c.height).collect(), ctx);
        for (key, scale, field) in [
            ("flow", 10.0, (|c: &Cell| c.flow) as fn(&Cell) -> f32),
            ("wear", 1.0, (|c: &Cell| c.wear) as fn(&Cell) -> f32),
            ("deposition", 1.0, (|c: &Cell| c.deposition) as fn(&Cell) -> f32),
            ("sediment", 1.0, (|c: &Cell| c.sediment) as fn(&Cell) -> f32),
        ] {
            check_cancel(ctx)?;
            outputs.insert(
                key.into(),
                mask_out(cells.par_iter().map(field).collect(), scale, ctx),
            );
        }
        check_cancel(ctx)?;
        ctx.report_progress(1.0);
        Ok(outputs)
    }
}
