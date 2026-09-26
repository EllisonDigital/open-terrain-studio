use std::sync::Arc;
use terrain_core::error::{CoreError, Result};
use terrain_core::{EvalContext, Grid, NodeKind, NodeSchema, Outputs, ParamDef, PortDef, PortType, Value};

/// Horizontal rock beds sampled from terrain elevation, with thickness in metres.
/// The map is static during a simulation; chain nodes to expose new strata.
pub struct RockHardness {
    schema: NodeSchema,
}
impl Default for RockHardness {
    fn default() -> Self {
        Self { schema: NodeSchema {
            type_id: "data.rock_hardness".into(), type_version: 1,
            label: "Rock Hardness".into(), category: "Data".into(), gpu: false,
            description: "Alternating soft and hard horizontal beds. Connect to an erosion node's Rock hardness input.".into(),
            inputs: vec![PortDef::new("in", "Terrain", PortType::Heightfield)],
            outputs: vec![PortDef::new("out", "Hardness", PortType::Mask)],
            params: vec![
                ParamDef::metres("layer_thickness_m", "Layer thickness", 50.0, 0.01, 10_000.0)
                    .describe("Thickness of each soft or hard bed; one repeat contains two beds."),
                ParamDef::metres("offset_m", "Vertical offset", 0.0, -20_000.0, 20_000.0),
                ParamDef::float("soft_hardness", "Soft bed hardness", 0.1, 0.0, 1.0),
                ParamDef::float("hard_hardness", "Hard bed hardness", 0.9, 0.0, 1.0),
            ],
        } }
    }
}
impl NodeKind for RockHardness {
    fn schema(&self) -> &NodeSchema {
        &self.schema
    }
    fn reach(&self, _ctx: &EvalContext) -> terrain_core::Reach {
        crate::common::point_wise()
    }
    fn evaluate(&self, ctx: &EvalContext) -> Result<Outputs> {
        super::check_cancel(ctx)?;
        let input = ctx.input_grid("in")?;
        if input.spec != ctx.spec
            || input.data.len() != ctx.spec.len()
            || input.data.iter().any(|v| !v.is_finite())
        {
            return Err(CoreError::NodeFailed {
                node: ctx.node_id.into(),
                message: "rock hardness requires a finite terrain matching the evaluation grid".into(),
            });
        }
        let thickness = ctx.f64("layer_thickness_m");
        let offset = ctx.f64("offset_m");
        let soft = ctx.f32("soft_hardness");
        let hard = ctx.f32("hard_hardness");
        let grid: Grid = input.map(|height| {
            let wave = libm::sin((height as f64 - offset) / thickness * std::f64::consts::PI);
            let t = (wave as f32 * 2.0 + 0.5).clamp(0.0, 1.0);
            soft + (hard - soft) * t * t * (3.0 - 2.0 * t)
        });
        super::check_cancel(ctx)?;
        Ok(Outputs::from([("out".into(), Value::Mask(Arc::new(grid)))]))
    }
}
