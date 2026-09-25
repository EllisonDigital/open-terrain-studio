# Water and hydrology (v0.5)

Seven nodes describe where water runs, collects and freezes. All run on the CPU, are deterministic for
any number of threads, and give the same result at every preview and build resolution. The code is in
`crates/terrain-nodes/src/water.rs`; drainage shared with Hydraulic Erosion is in `hydro.rs`.

| Node | Category | Outputs |
| --- | --- | --- |
| Flow | Data | Flow (drained area), Direction, Basins |
| Wetness | Data | Wetness, Water distance |
| Rivers | Simulate | Height, Water surface, River, Riverbank |
| Lakes | Simulate | Height, Water surface, Lakes, Shore |
| Sea | Simulate | Height, Water surface, Sea, Shallows, Shoreline |
| Snow | Simulate | Height, Snow |

A typical chain is **terrain → Hydraulic Erosion → Rivers → Lakes → Sea → Snow**; the *River coast*
example (`app/examples/river_coast.otstudio`) is exactly that.

## Drainage

Flow, Rivers, Lakes and Wetness route water on a fixed grid set by **Detail size** (metres), like
Hydraulic Erosion: the terrain is low-pass filtered and resampled onto it, drained, and the result is
resampled back. Every resolution finer than twice the detail size therefore drains identically.

- **Routing:** Priority-Flood+ε (Barnes, Lehman & Mulla 2014) from the world's edges fills pits for
  routing only, with a small random ε so flow wanders across flats. Every cell drains to its steepest
  neighbour (D8). Water always leaves through the world's edges.
- **Accumulation:** drained area in m². Flow's *D-infinity* method (Tarboton 1997) splits each cell's
  water between the two neighbours on its steepest facet; *D8* sends it all one way.
- **Flow mask:** drained area on a log scale, black at 5,000 m², white at 50 km² (the same scale as
  Hydraulic Erosion's Flow). Streams are widened to about three drainage cells so they resample cleanly.
- **Direction:** flow angle / 360° (0° = +X, 90° = +Y, as for every angle in the app).
- **Basins:** every cell takes a fixed pseudo-random value of the outlet it drains to.

## Rivers

A river starts where the drained area reaches **Source area**. Each river cell draws a curve (a quadratic
Bézier) from the middle of its link to the middle of its receiver's link, so D8 steps become bends and
tributaries join smoothly. **Meander** shifts the centreline by a smooth random field (bends about 12 ×
Meander long), fading out at the world's edges so rivers still leave through them.

Along a river the water level is the routing surface, which never rises downstream. Width and depth
grow with drained area on a log scale, from a tenth (width) and 30 % (depth) at the source to full size
at 1,000 × the source area. At full resolution every pixel within reach of a river piece is lowered to

- inside the channel: `level − depth · (1 − (d / half width)²)` (a parabolic bed),
- on the bank: from the water level back up to the ground over **Bank width** (smoothstep),

taking the lowest result of all pieces, so carving never raises ground. The **River** mask is the share of
each pixel covered by water, so rivers narrower than a pixel still show; **Riverbank** is white at the
water's edge and fades over the bank width. **Water surface** is the water level where the bed is below it,
and the ground elsewhere.

Rivers cross hollows at their spill level; connect a **Lakes** node after Rivers to flood them.

## Lakes

Plain Priority-Flood (no ε) fills every depression flat to the level where it would spill. Connected
flooded cells at the same level form one lake; lakes shallower than **Min depth** or smaller than **Min
area** stay dry. At full resolution a pixel is lake where its own height is below its lake's level, so
shorelines are as sharp as the terrain.

- **Height:** the lake bed, with **Sediment infill** filling that share of each lake's depth to a flat floor.
- **Water surface:** the lake level over lakes, the ground elsewhere.
- **Lakes:** white on water (softened over the last 10 cm of depth).
- **Shore:** white at the waterline, fading over **Shore width** on both sides.

## Sea

Ground below **Sea level** is sea if the sea can reach it from the world's edges (4-neighbour flood fill),
or all of it with *Only from edges* off. Land up to **Beach height** above the sea and within **Shore width**
of it is worn towards sea level by **Coastal erosion**: most at the waterline, fading with height and
distance. **Shallows** is white at the waterline and fades to black at **Shallows depth**; **Shoreline** is
white at the waterline, fading over the shore width on both sides.

## Snow

Snow settles above **Snow line**, fading over **Transition**. On a slope facing the **Shaded side** the
snow line is up to **Shade effect** metres lower (and higher on slopes facing away), weighted by how
clearly the slope faces that way; flat ground counts as neither. Snow slides off ground steeper than
**Max slope** (±5°). The cover is softened over **Smoothing** metres, then **Melt** removes thin snow
first: `snow = (cover − melt) / (1 − melt)`. **Height** adds **Depth** × snow to the terrain.

The default shaded side, 270°, is up in the 2D view.

## Wetness

**Wetness** combines the topographic wetness index `ln(a / tan β)` (a: drained area per metre of
contour, β: slope; black at 4, white at 14, computed on the drainage grid with D-infinity) with
nearness to water. **Water distance** is black at water and white **Reach** metres away. Water is every
stream draining more than **Stream area**, plus the **Water** input where it's above 0.5 if connected (e.g.
a River, Lakes or Sea mask).

## 3D view

For a heightfield made with Rivers, Lakes or Sea, the 3D view draws water: the highest water surface of
those nodes where it's more than 1 cm above their own height, as a translucent surface that darkens with
depth. Masks are drawn without water, so overlays stay readable.

## Tests

`crates/terrain-nodes/tests/water.rs` checks every output for range and thread-count determinism, each
node's behaviour on known shapes, and that every output matches between 129² and 513² (mean < 1 % of the
range; 99th percentile < 10 %, or < 30 % for the Shore band and the River and Riverbank masks, whose
edges depend on the pixel size). `app/tests/water_test.gd` runs the nodes through the Godot extension and
checks the water the 3D view draws. The shared node tests also cover them (resolution independence of
the first output, golden hashes).

## Limits

- Routing is D8 on the drainage grid; on smooth, uneroded slopes rivers still follow its 45° steps, only
  softened by bends and meanders. Erode the terrain first for natural valleys.
- Rivers don't widen into lakes or deltas themselves, and don't erode; use Hydraulic Erosion for valleys.
- The Shore and Shoreline bands come from a distance transform on the output grid, exact to about one pixel.
- No GPU kernels yet; drainage is sequential along the drainage tree like Hydraulic Erosion.
