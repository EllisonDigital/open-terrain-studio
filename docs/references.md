# References

The published sources each algorithm is built from. Every one is a clean-room implementation from the
paper or article (ARCHITECTURE.md, *Clean-room rule*); where the code simplifies or departs from the source,
the doc comment at the listed location says how.

## Noise and hashing

| Algorithm | Code | Source |
| --- | --- | --- |
| Perlin gradient noise, quintic fade | `noise/basis.rs` `perlin` | K. Perlin, "An Image Synthesizer", SIGGRAPH 1985; K. Perlin, "Improving Noise", SIGGRAPH 2002 |
| Simplex noise | `noise/basis.rs` `simplex` | K. Perlin, "Noise Hardware", SIGGRAPH 2001 course notes; S. Gustavson, "Simplex noise demystified", 2005 |
| Value noise | `noise/basis.rs` `value` | J. P. Lewis, "Algorithms for solid noise synthesis", SIGGRAPH 1989 |
| Cellular (Worley) noise | `noise/basis.rs` `cellular` | S. Worley, "A Cellular Texture Basis Function", SIGGRAPH 1996 |
| fBm | `noise/basis.rs` `fbm` | B. Mandelbrot & J. Van Ness, "Fractional Brownian motions, fractional noises and applications", SIAM Review 10(4), 1968; F. K. Musgrave in Ebert et al., *Texturing and Modeling*, 1994 |
| Ridged multifractal (simplified) | `noise/basis.rs` `ridged` | F. K. Musgrave, *Methods for Realistic Landscape Imaging*, PhD thesis, Yale 1993; Ebert et al. 1994 |
| Billow | `noise/basis.rs` `billow` | libnoise `Billow` module (J. Bevins) |
| Domain warping | `noise/basis.rs` `warped_fbm` | I. Quilez, "Domain Warping", iquilezles.org, 2002 |
| SplitMix64 | `terrain-core/src/seed.rs` `mix64` | G. Steele, D. Lea & C. Flood, "Fast splittable pseudorandom number generators", OOPSLA 2014 |
| FNV-1a | `terrain-core/src/seed.rs` `fnv1a64` | G. Fowler, L. C. Noll & K.-P. Vo, FNV hash |

## Erosion

| Algorithm | Code | Source |
| --- | --- | --- |
| Stream-power law, m = 0.5, n = 1 | `erosion/hydraulic.rs` | A. Howard, "A detachment-limited model of drainage basin evolution", WRR 30(7), 1994; K. Whipple & G. Tucker, JGR 104(B8), 1999 |
| Implicit O(n) stream-power solver | `erosion/hydraulic.rs` | J. Braun & S. Willett, "A very efficient O(n), implicit and parallel method to solve the stream power equation", Geomorphology 180–181, 2013 |
| Sediment-flux-dependent erosion (not yet used; see the doc comment) | `erosion/hydraulic.rs` | K. Whipple & G. Tucker, "Implications of sediment-flux-dependent river incision models for landscape evolution", JGR 107(B2), 2002; P. Davy & D. Lague, "Fluvial erosion/transport equation of landscape evolution models revisited", JGR 114, 2009 |
| Hillslope creep (linear diffusion) | `erosion/hydraulic.rs` | W. Culling, "Analytical theory of erosion", J. Geology 68, 1960; creep rates: N. Fernandes & W. Dietrich, WRR 33(6), 1997 |
| Talus (thermal) erosion | `erosion/thermal.rs` | F. K. Musgrave, C. Kolb & R. Mace, "The synthesis and rendering of eroded fractal terrains", SIGGRAPH 1989; J. Olsen, "Realtime Procedural Terrain Generation", 2004 |
| Isotropic nine-point stencil weights (thermal) | `erosion/thermal.rs` | M. Patra & M. Karttunen, "Stencils with isotropic discretization error for differential operators", Numer. Methods PDE 22(4), 2006 |

## Drainage and water

| Algorithm | Code | Source |
| --- | --- | --- |
| Priority-Flood and Priority-Flood+ε | `hydro.rs` `route`, `fill_depressions` | R. Barnes, C. Lehman & D. Mulla, "Priority-Flood: An optimal depression-filling and watershed-labeling algorithm for digital elevation models", Computers & Geosciences 62, 2014 |
| D8 flow directions | `hydro.rs` | J. O'Callaghan & D. Mark, "The extraction of drainage networks from digital elevation data", CVGIP 28, 1984 |
| D-infinity flow | `water.rs` `dinf` | D. Tarboton, "A new method for the determination of flow directions and upslope areas in grid digital elevation models", WRR 33(2), 1997 |
| Topographic wetness index | `water.rs` `Wetness` | K. Beven & M. Kirkby, "A physically based, variable contributing area model of basin hydrology", Hydrol. Sci. Bull. 24(1), 1979 |

## Grid operations

| Algorithm | Code | Source |
| --- | --- | --- |
| Box-filter Gaussian approximation | `terrain-core/src/ops.rs` `box_sizes` | P. Kovesi, "Fast Almost-Gaussian Filtering", DICTA 2010; W. Wells, IEEE PAMI 8(2), 1986 |
| Euclidean distance transform | `terrain-core/src/ops.rs` `distance_to` | P. Felzenszwalb & D. Huttenlocher, "Distance Transforms of Sampled Functions", Theory of Computing 8, 2012 |
| Monotone curve interpolation | `terrain-core/src/params.rs` `Curve` | F. Fritsch & R. Carlson, "Monotone Piecewise Cubic Interpolation", SIAM J. Numer. Anal. 17(2), 1980 |
| Topographic position index (Curvature node) | `data.rs` `Curvature` | A. Weiss, "Topographic Position and Landforms Analysis", ESRI User Conference poster, 2001; A. Guisan, S. Weiss & A. Weiss, Plant Ecology 143, 1999 |

## Vegetation

| Algorithm | Code | Source |
| --- | --- | --- |
| Background grid for Poisson-disk sampling | `vegetation.rs` `scatter` | R. Bridson, "Fast Poisson Disk Sampling in Arbitrary Dimensions", SIGGRAPH 2007 sketch |
| Phase groups for parallel, deterministic sampling | `vegetation.rs` `scatter` | L.-Y. Wei, "Parallel Poisson Disk Sampling", SIGGRAPH 2008 |
| Multiplicative habitat suitability | `vegetation.rs` | J. Hammes, "Modeling of ecosystems as a data source for real-time terrain rendering", DEM 2001; O. Deussen et al., "Realistic modeling and rendering of plant ecosystems", SIGGRAPH 1998 |

## Not physical models

Mountain, Ridge, Canyon, Crater, Plateau, Dunes, Rivers (channel shape and meanders), Sea (beaches), Snow,
Occlusion and the colour blend modes are authoring approximations. Where a physical reference would help
calibrate them: dune slip faces near the angle of repose (R. Bagnold, *The Physics of Blown Sand and Desert
Dunes*, 1941); crater depth/diameter (R. Pike, 1977) and ejecta thickness ∝ r⁻³ (T. McGetchin et al., EPSL
20, 1973); river hydraulic geometry w ∝ Q^0.5, d ∝ Q^0.4 (L. Leopold & T. Maddock, USGS PP 252, 1953);
sky visibility (Ž. Zakšek, K. Oštir & Ž. Kokalj, "Sky-View Factor as a Relief Visualization Technique",
Remote Sensing 3, 2011).
