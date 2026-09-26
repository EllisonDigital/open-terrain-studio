# Vegetation (v0.7)

Vegetation follows Gaea's model ([ARCHITECTURE.md §8](ARCHITECTURE.md#8-vegetation-and-ecosystems)).
Each **Trees**, **Shrubs** or **Grass** node is one population. It outputs where the plants grow as a density
mask, one point per plant, and masks for chaining the next population. **Debris / Rocks** scatters rocks from
talus and dead zones, and **Pack Masks (RGBA)** packs four masks into one image for engines.

Vegetation nodes live in the **Vegetation** graph tab (since v0.7.5), fed from the Terrain tab through portals.
The *Forest valley* example (**File → Open Example**) chains pine, birch and shrubs on an eroded mountain, with
rocks below the cliffs. The *Asterfall Crown* example has the same chain on its finished landscape.

## Nodes

| Node | Inputs | Outputs |
| --- | --- | --- |
| Trees, Shrubs, Grass | Terrain (required); Water, Snow, Allowed area, Occupied (optional masks) | Density, Points, Dead zones, Occupied, Water influence |
| Debris / Rocks | Terrain (required); Talus, Dead zones, Allowed area (optional masks) | Density, Points |
| Pack Masks (RGBA) | Red, Green, Blue, Alpha (optional masks) | Packed (RGBA colour map) |

Trees, Shrubs and Grass share their settings and differ only in defaults. Trees are sparse (7 m spacing), grow
up to 1,400 m and avoid peaks. Shrubs are denser (3 m) and hardier. Grass has 2 m spacing and grows almost
anywhere that isn't too steep.

### Growth model

A population's density is the product of three groups of factors, as in Gaea.

**Growth and health.**
- *Coverage* (mask-drivable) is the density where conditions are ideal.
- *Water preference* above 0 seeks water and below 0 avoids it. Water is the **Water** input (connect a
  Wetness mask, or a River, Lakes or Sea mask). Without that input, valleys count as wet: ground lower than its
  surroundings within about 120 m.
- *Spread* blurs the terrain factors, so plants reach a little beyond the ground that suits them.
- *Health* sets how well plants cope with poor ground. At 1 the density follows the habitat. Lower values leave
  only the best spots covered.
- *Patches* and *Patch size* add clumps and clearings. *Randomness* thins the cover at about four times the
  spacing.

**Inhibitors.**
- *Slope min/max* and *Altitude min/max* each have a falloff.
- *Avoid peaks* thins cover on exposed crests.
- *Snow tolerance* sets how much snow (the **Snow** input) the plants survive.
- The **Allowed area** mask multiplies the density.

**Occupied (chaining).** Connect an earlier population's **Occupied** output to the next one's **Occupied**
input. The *Occupied* setting picks how the new population responds:
- *Avoid* keeps it out of taken ground.
- *Intermingle* shares taken ground at half density.
- *Grow near* clusters it around the edges of taken ground.

Each population's Occupied output is its input plus its own taken ground, which counts as fully taken from a
density of 0.5. A chain therefore accumulates everything grown so far.

**Dead zones.** Slopes steeper than *Dead zone slope* (scree) and the run-out below them go bare. With a Snow
input, steep snowy ground (slide paths) does too. *Dead zones* sets how much. The **Dead zones** output feeds
Debris / Rocks.

**Water influence** is the water signal the population responded to: the Water input, or the valley estimate.

### Debris / Rocks

Rocks gather on talus: the **Talus** input (e.g. Thermal Erosion's *Debris / Talus* output) or, without one,
the ground below slopes steeper than *Shedding slope*, reaching *Reach* metres out. They also gather in the
**Dead zones** input. *Talus* and *Dead zones* weight the two sources.

## Points

Points are sampled from the density mask with a minimum distance between them (*Spacing*, in metres).

- The world is divided into cells of `spacing / √2`, fixed in world coordinates. Each cell holds at most one
  point.
- A cell tries for a point with probability equal to the density at its centre. It gets up to three random
  positions inside the cell to find one at least *Spacing* away from every point already placed. The check
  covers the 5 × 5 cells around it.
- Cells are processed in nine interleaved phases, so cells handled at the same time are too far apart to
  conflict. The result is the same for any number of threads.
- Every random number comes from the node's seed and the cell's world coordinates. Points therefore stay in
  the same place at every resolution: in the tests, over 90% of points sit at identical positions at 513² and
  2,049². The rest change with small differences in the density mask.
- Density 1 packs points about as tightly as the spacing allows: about 0.6 points per square of side *Spacing*.
- Each point gets a random rotation between *Rotation min* and *max*, and a random scale between *Scale min* and
  *max*. Its height is the terrain's, sampled bilinearly.

A population may sample at most 40 million cells (about 350 MB while sampling). If the spacing is too small for
the world, the node reports an error asking for a larger spacing. On an 8 km world the smallest spacing is
about 1.8 m. Grass is usually exported as a density mask for the engine's own grass tools; its points are
clumps.

**Speed.** 2.5 million grass points take 0.4 s on the default 8 km world at 1,025² (32-thread Ryzen 9 8940HX).
The whole *Forest valley* graph, erosion included, with 1.5 million pine, birch and shrub points, evaluates
in about 1.6 s at 1,025².

Point counts follow the density, and the density follows slopes. Slopes are steeper at higher resolutions,
especially on eroded terrain, so previews at low resolution can show somewhat more plants than the build.
The *Forest valley* pines number 38,158 at 257² and 36,210 at 1,024². Check counts at the build resolution.

## Species presets

A species preset is a saved parameter set for one node type, stored as a small YAML file. Choose one under
**Species preset** in a vegetation node's settings. *Load…* applies a file and *Save…* writes the node's
current settings. Applying a preset is one undo step.

Seventeen presets ship in `app/examples/species/`:

| Biome | Trees | Shrubs | Grass | Rocks |
| --- | --- | --- | --- | --- |
| Temperate | Scots pine, Norway spruce, Silver birch, European beech | Hazel | Meadow grass | |
| Alpine | Swiss stone pine | Dwarf mountain pine, Alpine rose | Alpine meadow | Scree and boulders |
| Desert | Saguaro | Creosote bush | Desert bunchgrass | |
| Tropical | Coconut palm, Kapok | Tree fern understorey | | |

Altitudes are in metres for a world about 0–2,000 m high. Adjust them to your world's height range.

The format is flat `key: value` lines, a subset of YAML:
- `name`, `node` (the node type, required), `biome` and `description` describe the preset.
- Every other key is a parameter of that node, e.g. `spacing_m: 7`.
- Values are numbers, `true`/`false`, or plain or quoted text, with `#` comments.
- Nested maps and lists are rejected.
- Unknown keys are reported but don't stop the rest of the preset applying.

```yaml
name: Scots pine
node: vegetation.trees
biome: temperate
description: Hardy pine of dry, sandy and rocky ground up to the tree line, in open stands.
species: scots_pine
spacing_m: 7
height_max_m: 1500
water_preference: -0.2
```

## In the editor

- **Vegetation tab** (since v0.7.5): vegetation nodes live in their own graph tab, between Terrain and Colour.
  To use a terrain there, select its node in the Terrain tab and press **→ Vegetation** next to an output
  under *Send to another tab*. A Height or Mask Portal appears in the Vegetation tab; connect it to the
  populations' Terrain, Water or Snow inputs.
- A population's masks can go on to the Colour tab the same way (**→ Colour**), for example to colour forest
  floors.
- Projects saved by v0.7 open with their vegetation moved into this tab, fed by portals, with identical results.
- **Plants** (viewport toolbar) draws the points of the viewed node, and of every vegetation node upstream of it,
  as placeholder shapes at real size: cones for trees, balls for shrubs, tufts for grass, lumps for rocks.
- At most 150,000 are drawn. Each node gets an equal share, trees first, and a node that needs fewer passes the
  rest on. A thinned node keeps every n-th point.
- The status bar shows the total and how many are drawn.
- Viewing a **Points** output shows each point as a white dot on the mask overlay, and its count.
- **Data view** shows a population's ecosystem forces as colours over the terrain: red = dead zones,
  green = density, blue = water influence.

## Export

Mark outputs in the inspector (or with **Build → Mark Viewed Output for Export**) and **Build**:

| Output | Formats | Notes |
| --- | --- | --- |
| Density, Dead zones, Occupied, Water influence | PNG 8, PNG 16, EXR | Greyscale 0–1, one image per mask |
| Pack Masks (RGBA) | PNG 8 (usually), PNG 16, EXR | Four masks in R, G, B, A, values unchanged |
| Points | CSV, JSON | One row per point |

**CSV** has a header row, then one row per point, in metres to the millimetre:

```text
x,y,z,rotation_deg,scale,species
1033.412,2877.090,642.118,211.37,1.0841,scots_pine
```

**JSON** holds the same rows, with the column names and species listed once:

```json
{
  "generator": "OpenTerrainStudio",
  "version": 1,
  "columns": ["x", "y", "z", "rotation_deg", "scale", "species"],
  "species": ["scots_pine"],
  "count": 1,
  "points": [
    [1033.412, 2877.090, 642.118, 211.37, 1.0841, "scots_pine"]
  ]
}
```

- `x` and `y` are metres from the world origin, the corner at row 0 / column 0 of every exported image.
- `z` is the terrain height in metres, in the same units as the EXR heightmap.
- `rotation_deg` turns the plant about the vertical axis: 0° faces +X and 90° faces +Y, like every direction
  in the app.
- `scale` multiplies the mesh as modelled.
- `species` is the node's *Species* text, so an import script can pick the matching mesh.

In `build.json`, each point file has `"data": "point_set"`, `"format": "csv"` or `"json"`, `"points"` (the
count), `"species"`, and an `encoding` that spells out the columns. `data_min`/`data_max` are the lowest and
highest point heights.

### Placing points in an engine

These are the same conventions as the heightmaps in the [export guides](export-guides.md). With the terrain
placed as described there, point `(x, y, z, rotation_deg, scale)` goes at:

| Engine | Position | Rotation |
| --- | --- | --- |
| Godot (terrain centred on the origin) | `Vector3(x − size/2, z, y − size/2)` | `-deg_to_rad(rotation_deg)` about +Y |
| Unreal (landscape at location 0, 0) | `(x × 100, y × 100, z × 100)` cm | Yaw = `rotation_deg` |
| Blender (centred grid) | `(x − size/2, size/2 − y, z)` | `-rotation_deg` about +Z |

For example, as a Godot `MultiMesh` built from the CSV:

```gdscript
var f := FileAccess.open("res://terrain/trees-n_0006_points_2049.csv", FileAccess.READ)
f.get_csv_line()  # header
var transforms: Array[Transform3D] = []
while not f.eof_reached():
    var r := f.get_csv_line()
    if r.size() < 6:
        continue
    var basis := Basis(Vector3.UP, -deg_to_rad(float(r[3]))).scaled(Vector3.ONE * float(r[4]))
    transforms.append(Transform3D(basis, Vector3(float(r[0]) - size / 2, float(r[2]), float(r[1]) - size / 2)))
```

`app/tests/vegetation_test.gd` exports the *Forest valley* points and places them this way. Every point lands
within 1 mm of where the app's preview puts it (the millimetre rounding of the CSV). Dedicated Blender, Godot
and Unreal importers that scatter meshes from these files are part of the import helpers ([ROADMAP.md,
v0.9](ROADMAP.md)).

## Limits

- All vegetation nodes run on the CPU.
- Points are sampled over the whole grid being built. Tiled builds (v0.8) will need a margin so points at tile
  edges agree.
- Positions are stored as 32-bit floats: exact to about 1 mm on an 8 km world, and 8 mm on a 100 km one.
- The Water input's streams come from the Wetness node's D8 routing. On smooth, uneroded slopes they can run in
  straight lines, and damp-loving populations follow them.
