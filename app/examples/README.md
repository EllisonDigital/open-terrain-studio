# Bundled example projects

**Asterfall Crown — Hero World** is the living showcase project for OpenTerrainStudio. Open it from
**Project → Examples → Asterfall Crown — Hero World**. It is intentionally a substantial editable
graph, not a finished heightmap: future generators, biome tools, erosion systems and export features
should be demonstrated here as they are added.

The setting is a 16.4 × 16.4 km high alpine basin: a folded regional foundation, the Crown Spine,
three major massifs, a red mesa, the Glass Caldera and the Kingsroad Gorge. The finished terrain runs
through a subtle tectonic warp, highland-only grain, alternating bedrock, hydraulic river erosion and
thermal talus. The final heightfield and useful masks are marked for export: flow, wear, deposition,
sediment, debris/talus, steep rock, curvature and an alpine elevation band. The project opens on the
final landscape at 512² preview resolution and is set up for a 2,017² build.

Keep this project loadable and buildable with built-in nodes. When adding a major terrain feature to
the app, consider extending this world so the example grows with the product; preserve the recognizable
basin and existing export products unless a feature specifically supersedes them. The test suite loads
and builds every `.otstudio` in this directory.

**River coast** is the v0.5 water example: a mountain island, eroded, with a crater on its flank, then
Rivers, Lakes, Sea and Snow. Rivers run to the sea at the world's edges, the crater holds a lake, and the
River, Lakes, Sea, Shoreline and Snow masks are marked for export. See [water.md](../../docs/water.md).

The v0.6 **Colour** tab builds a colour map from grass, rock, sand, snow and water,
plus a four-layer splat map and normal map. Height and masks arrive from the
Terrain tab through portals. These new outputs are also marked for export;
see [colour.md](../../docs/colour.md).

**Forest valley** is the v0.7 vegetation example: an eroded mountain with Wetness and Snow, then three chained
populations. Scots pine grows high and dry; silver birch takes the damp valleys and keeps out of the pines;
hazel shrubs intermingle with both. Rocks gather on thermal-erosion talus and in the pines' dead zones. The
densities, pine dead zones and an RGBA pack of all four are marked for export as PNG 8, and every population's
points as CSV. **Asterfall Crown** has the same chain (crown pine, valley birch, krummholz, boulders) on its
finished landscape. Species presets for vegetation nodes are in `species/`. See
[vegetation.md](../../docs/vegetation.md).
