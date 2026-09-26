//! Point sets: vegetation and debris instances (ARCHITECTURE.md §4, §8).
//!
//! A point is a position in world metres (x and y from the world origin, the
//! same corner as row 0 / column 0 of exported images; z is the terrain
//! height in metres), a rotation about the vertical axis in degrees (0° along
//! +X, 90° along +Y, like every direction in the app), a scale factor and a
//! species. See `docs/vegetation.md` for the export formats.

use crate::grid::{Grid, GridSpec};

/// Values stored per point in [`PointSet::data`].
pub const STRIDE: usize = 5;

/// One point, as read from a [`PointSet`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x_m: f32,
    pub y_m: f32,
    pub z_m: f32,
    pub rotation_deg: f32,
    pub scale: f32,
    /// Index into [`PointSet::species`].
    pub species: u16,
}

/// A list of points over a region of the world.
#[derive(Clone, Debug, PartialEq)]
pub struct PointSet {
    /// The region and resolution the points were generated for.
    pub spec: GridSpec,
    /// `x_m, y_m, z_m, rotation_deg, scale` per point, in generation order.
    pub data: Vec<f32>,
    /// Species of each point, as an index into `species`.
    pub species_index: Vec<u16>,
    /// Species names (e.g. `scots_pine`), as written to exported files.
    pub species: Vec<String>,
}

impl PointSet {
    /// An empty set with one species.
    pub fn new(spec: GridSpec, species: &str) -> Self {
        Self {
            spec,
            data: Vec::new(),
            species_index: Vec::new(),
            species: vec![species.to_string()],
        }
    }

    pub fn len(&self) -> usize {
        self.species_index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.species_index.is_empty()
    }

    pub fn push(&mut self, p: Point) {
        self.data
            .extend_from_slice(&[p.x_m, p.y_m, p.z_m, p.rotation_deg, p.scale]);
        self.species_index.push(p.species);
    }

    pub fn get(&self, i: usize) -> Point {
        let d = &self.data[i * STRIDE..i * STRIDE + STRIDE];
        Point {
            x_m: d[0],
            y_m: d[1],
            z_m: d[2],
            rotation_deg: d[3],
            scale: d[4],
            species: self.species_index[i],
        }
    }

    /// The points inside `spec`'s area (edges included), with `spec` as
    /// their region.
    pub fn within(&self, spec: GridSpec) -> PointSet {
        let (x0, y0) = (spec.origin_m[0], spec.origin_m[1]);
        let (x1, y1) = (x0 + spec.extent_m[0], y0 + spec.extent_m[1]);
        let mut out = PointSet {
            spec,
            data: Vec::new(),
            species_index: Vec::new(),
            species: self.species.clone(),
        };
        for p in self.iter() {
            let (x, y) = (p.x_m as f64, p.y_m as f64);
            if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                out.push(p);
            }
        }
        out
    }

    pub fn iter(&self) -> impl Iterator<Item = Point> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    /// Species name of point `i`.
    pub fn species_of(&self, i: usize) -> &str {
        &self.species[self.species_index[i] as usize]
    }

    /// Lowest and highest point z, in metres (`(0, 0)` when empty).
    pub fn z_range(&self) -> (f32, f32) {
        if self.is_empty() {
            return (0.0, 0.0);
        }
        self.iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p.z_m), hi.max(p.z_m))
            })
    }

    /// A mask of `spec` that is 1 at the sample nearest each point, for
    /// previewing a point set as an image.
    pub fn rasterise(&self, spec: GridSpec) -> Grid {
        let mut grid = Grid::filled(spec, 0.0);
        let (w, h) = (spec.width as i64, spec.height as i64);
        for p in self.iter() {
            let i = spec.column_at(p.x_m as f64).round() as i64;
            let j = spec.row_at(p.y_m as f64).round() as i64;
            if (0..w).contains(&i) && (0..h).contains(&j) {
                grid.data[(j * w + i) as usize] = 1.0;
            }
        }
        grid
    }

    /// The points as CSV: a header row `x,y,z,rotation_deg,scale,species`
    /// then one row per point; positions and heights in metres to the
    /// millimetre.
    pub fn to_csv(&self) -> String {
        let mut out = String::with_capacity(self.len() * 48 + 64);
        out.push_str("x,y,z,rotation_deg,scale,species\n");
        for (i, p) in self.iter().enumerate() {
            use std::fmt::Write as _;
            let _ = writeln!(
                out,
                "{:.3},{:.3},{:.3},{:.2},{:.4},{}",
                p.x_m,
                p.y_m,
                p.z_m,
                p.rotation_deg,
                p.scale,
                self.species_of(i)
            );
        }
        out
    }

    /// The points as JSON: `columns` names the values in each row of
    /// `points`, which holds numbers and the species name, as in the CSV.
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(self.len() * 56 + 256);
        out.push_str("{\n  \"generator\": \"OpenTerrainStudio\",\n  \"version\": 1,\n");
        out.push_str("  \"columns\": [\"x\", \"y\", \"z\", \"rotation_deg\", \"scale\", \"species\"],\n");
        let species: Vec<String> = self.species.iter().map(|s| json_string(s)).collect();
        out.push_str(&format!("  \"species\": [{}],\n", species.join(", ")));
        out.push_str(&format!("  \"count\": {},\n  \"points\": [", self.len()));
        for (i, p) in self.iter().enumerate() {
            use std::fmt::Write as _;
            let sep = if i == 0 { "\n    " } else { ",\n    " };
            let _ = write!(
                out,
                "{sep}[{:.3}, {:.3}, {:.3}, {:.2}, {:.4}, {}]",
                p.x_m, p.y_m, p.z_m, p.rotation_deg, p.scale, species[p.species as usize]
            );
        }
        out.push_str(if self.is_empty() { "]\n}\n" } else { "\n  ]\n}\n" });
        out
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    fn sample() -> PointSet {
        let spec = GridSpec::full_world(&World::default(), 5).unwrap();
        let mut s = PointSet::new(spec, "scots_pine");
        s.push(Point {
            x_m: 0.0,
            y_m: 2048.0,
            z_m: 100.5,
            rotation_deg: 90.0,
            scale: 1.25,
            species: 0,
        });
        s.push(Point {
            x_m: 8192.0,
            y_m: 8192.0,
            z_m: -3.0,
            rotation_deg: 359.5,
            scale: 0.5,
            species: 0,
        });
        s
    }

    #[test]
    fn csv_and_json_list_every_point() {
        let s = sample();
        assert_eq!(
            s.to_csv(),
            "x,y,z,rotation_deg,scale,species\n\
             0.000,2048.000,100.500,90.00,1.2500,scots_pine\n\
             8192.000,8192.000,-3.000,359.50,0.5000,scots_pine\n"
        );
        let json: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(json["count"], 2);
        assert_eq!(json["points"][1][0], 8192.0);
        assert_eq!(json["points"][0][5], "scots_pine");
        let empty = PointSet::new(s.spec, "x");
        let json: serde_json::Value = serde_json::from_str(&empty.to_json()).unwrap();
        assert_eq!(json["points"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn rasterise_marks_nearest_samples() {
        let s = sample();
        let g = s.rasterise(s.spec);
        assert_eq!(g.get(0, 1), 1.0);
        assert_eq!(g.get(4, 4), 1.0);
        assert_eq!(g.data.iter().filter(|v| **v > 0.0).count(), 2);
        assert_eq!(s.z_range(), (-3.0, 100.5));
    }
}
