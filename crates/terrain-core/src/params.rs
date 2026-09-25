use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// A stored parameter value. Serialised as a plain JSON value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    /// Anything else (kept verbatim, e.g. for node types this version doesn't know).
    Other(serde_json::Value),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Float(v) => Some(*v),
            ParamValue::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ParamValue::Int(v) => Some(*v),
            ParamValue::Float(v) if v.fract() == 0.0 => Some(*v as i64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ParamValue::Bool(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::Text(v) => Some(v),
            _ => None,
        }
    }
}

/// What kind of value a parameter holds, with its limits.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Float {
        min: f64,
        max: f64,
        /// Suggested slider step in the UI.
        step: f64,
    },
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    /// One of a fixed set of options: `(key, label)`.
    Enum {
        options: Vec<(String, String)>,
    },
    /// A response curve: control points `[x, y]` in 0..1, sorted by x.
    /// Stored as a JSON array of pairs.
    Curve,
    /// A colour gradient: stops `[t, r, g, b]` (t and sRGB colour in 0..1),
    /// sorted by t. Stored as a JSON array of stops.
    Gradient,
    /// A file path, relative to the project file or absolute.
    /// `filters` are file-dialog patterns such as `"*.png ; PNG image"`.
    File {
        filters: Vec<String>,
    },
}

/// Declaration of one node parameter. The Godot inspector is built from these.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParamDef {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    pub default: ParamValue,
    /// Display unit, e.g. "m" or "°". Empty for unitless values.
    pub unit: String,
    pub description: String,
    /// Can be driven per cell by a mask connected to a parameter port
    /// (ARCHITECTURE.md §5). The mask scales the value: black = 0, white = the
    /// value set, clamped to the parameter's range. Only float parameters whose
    /// node reads them with [`crate::EvalContext::field`] may be drivable.
    pub drivable: bool,
}

impl ParamDef {
    /// A float parameter.
    pub fn float(key: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        // A power of ten, so typed values like 0, 1.0 or 1200 are never snapped.
        let step = 10f64
            .powi(((max - min) / 1000.0).log10().floor() as i32)
            .clamp(0.001, 1.0);
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Float { min, max, step },
            default: ParamValue::Float(default),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    /// A float parameter in metres.
    pub fn metres(key: &str, label: &str, default: f64, min: f64, max: f64) -> Self {
        let mut p = Self::float(key, label, default, min, max).unit("m");
        p.kind = ParamKind::Float { min, max, step: 1.0 };
        p
    }

    pub fn int(key: &str, label: &str, default: i64, min: i64, max: i64) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Int { min, max },
            default: ParamValue::Int(default),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    pub fn bool(key: &str, label: &str, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Bool,
            default: ParamValue::Bool(default),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    pub fn choice(key: &str, label: &str, default: &str, options: &[(&str, &str)]) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Enum {
                options: options
                    .iter()
                    .map(|(k, l)| (k.to_string(), l.to_string()))
                    .collect(),
            },
            default: ParamValue::Text(default.into()),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    /// A response curve, default a straight line from (0, 0) to (1, 1).
    pub fn curve(key: &str, label: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Curve,
            default: ParamValue::Other(serde_json::json!([[0.0, 0.0], [1.0, 1.0]])),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    /// A colour gradient with default `stops` (`[t, r, g, b]`, sRGB 0..1).
    pub fn gradient(key: &str, label: &str, stops: &[[f64; 4]]) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::Gradient,
            default: Gradient::from_stops(stops.to_vec()).to_value(),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    /// A file path (empty by default).
    pub fn file(key: &str, label: &str, filters: &[&str]) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ParamKind::File {
                filters: filters.iter().map(|f| f.to_string()).collect(),
            },
            default: ParamValue::Text(String::new()),
            unit: String::new(),
            description: String::new(),
            drivable: false,
        }
    }

    /// Allow a mask to drive this (float) parameter per cell.
    pub fn drivable(mut self) -> Self {
        debug_assert!(
            matches!(self.kind, ParamKind::Float { .. }),
            "only floats are drivable"
        );
        self.drivable = true;
        self
    }

    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.into();
        self
    }

    pub fn describe(mut self, text: &str) -> Self {
        self.description = text.into();
        self
    }

    /// Coerce and clamp a value to this parameter's kind and limits.
    pub fn validate(&self, value: &ParamValue) -> Result<ParamValue> {
        let bad = |reason: &str| CoreError::InvalidParam {
            param: self.key.clone(),
            reason: reason.into(),
        };
        match &self.kind {
            ParamKind::Float { min, max, .. } => {
                let v = value.as_f64().ok_or_else(|| bad("expected a number"))?;
                if !v.is_finite() {
                    return Err(bad("must be finite"));
                }
                Ok(ParamValue::Float(v.clamp(*min, *max)))
            }
            ParamKind::Int { min, max } => {
                let v = value
                    .as_i64()
                    .or_else(|| value.as_f64().map(|f| f.round() as i64))
                    .ok_or_else(|| bad("expected a whole number"))?;
                Ok(ParamValue::Int(v.clamp(*min, *max)))
            }
            ParamKind::Bool => value
                .as_bool()
                .map(ParamValue::Bool)
                .ok_or_else(|| bad("expected true or false")),
            ParamKind::Enum { options } => {
                let s = value.as_str().ok_or_else(|| bad("expected an option name"))?;
                if options.iter().any(|(k, _)| k == s) {
                    Ok(ParamValue::Text(s.into()))
                } else {
                    Err(bad(&format!("unknown option '{s}'")))
                }
            }
            ParamKind::Curve => {
                let points = Curve::parse(value).map_err(|e| bad(&e))?;
                Ok(points.to_value())
            }
            ParamKind::Gradient => Ok(Gradient::parse(value).map_err(|e| bad(&e))?.to_value()),
            ParamKind::File { .. } => value
                .as_str()
                .map(|s| ParamValue::Text(s.trim().into()))
                .ok_or_else(|| bad("expected a file path")),
        }
    }
}

/// Most control points a curve may have.
pub const MAX_CURVE_POINTS: usize = 64;

/// A smooth response curve through control points in 0..1, evaluated with
/// monotone cubic Hermite interpolation (Fritsch & Carlson 1980): it passes
/// through every point and never overshoots between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    tangents: Vec<f64>,
}

impl Curve {
    /// Parse and normalise a stored curve: points clamped to 0..1, sorted by
    /// x, duplicates in x removed. Needs at least two distinct points.
    pub fn parse(value: &ParamValue) -> std::result::Result<Self, String> {
        let json = match value {
            ParamValue::Other(v) => v.clone(),
            _ => return Err("expected a list of [x, y] points".into()),
        };
        let arr = json.as_array().ok_or("expected a list of [x, y] points")?;
        if arr.len() > MAX_CURVE_POINTS {
            return Err(format!("at most {MAX_CURVE_POINTS} points"));
        }
        let mut pts = Vec::with_capacity(arr.len());
        for p in arr {
            let pair = p
                .as_array()
                .filter(|a| a.len() == 2)
                .ok_or("each point must be [x, y]")?;
            let x = pair[0].as_f64().ok_or("point x must be a number")?;
            let y = pair[1].as_f64().ok_or("point y must be a number")?;
            if !(x.is_finite() && y.is_finite()) {
                return Err("points must be finite".into());
            }
            pts.push((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        pts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-6);
        if pts.len() < 2 {
            return Err("a curve needs at least two points with different x".into());
        }
        Ok(Self::from_points(&pts))
    }

    fn from_points(pts: &[(f64, f64)]) -> Self {
        let xs: Vec<f64> = pts.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = pts.iter().map(|p| p.1).collect();
        let n = xs.len();
        let slopes: Vec<f64> = (0..n - 1)
            .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
            .collect();
        let mut t = vec![0.0; n];
        t[0] = slopes[0];
        t[n - 1] = slopes[n - 2];
        for i in 1..n - 1 {
            t[i] = if slopes[i - 1] * slopes[i] <= 0.0 {
                0.0
            } else {
                (slopes[i - 1] + slopes[i]) * 0.5
            };
        }
        // Fritsch–Carlson: limit tangents so each segment stays monotone.
        for i in 0..n - 1 {
            if slopes[i] == 0.0 {
                t[i] = 0.0;
                t[i + 1] = 0.0;
                continue;
            }
            let a = t[i] / slopes[i];
            let b = t[i + 1] / slopes[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let k = 3.0 / s.sqrt();
                t[i] = k * a * slopes[i];
                t[i + 1] = k * b * slopes[i];
            }
        }
        Self { xs, ys, tangents: t }
    }

    /// The control points.
    pub fn points(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.xs.iter().copied().zip(self.ys.iter().copied())
    }

    /// Store as a JSON list of pairs.
    pub fn to_value(&self) -> ParamValue {
        ParamValue::Other(serde_json::Value::Array(
            self.points().map(|(x, y)| serde_json::json!([x, y])).collect(),
        ))
    }

    /// Evaluate at `x` (clamped to the first/last point outside them).
    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        // Few points: a linear scan is fastest.
        let mut i = 0;
        while x > self.xs[i + 1] {
            i += 1;
        }
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        h00 * self.ys[i] + h10 * h * self.tangents[i] + h01 * self.ys[i + 1] + h11 * h * self.tangents[i + 1]
    }
}

/// Most stops a gradient may have.
pub const MAX_GRADIENT_STOPS: usize = 64;

/// A colour gradient: sRGB colours at positions 0..1, interpolated linearly
/// between stops (in sRGB, as painting apps do) and held beyond the ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    stops: Vec<[f64; 4]>,
}

impl Gradient {
    /// Parse and normalise stored stops: values clamped to 0..1, sorted by t.
    /// Needs at least one stop.
    pub fn parse(value: &ParamValue) -> std::result::Result<Self, String> {
        let bad = || "expected a list of [t, r, g, b] stops".to_string();
        let ParamValue::Other(json) = value else {
            return Err(bad());
        };
        let arr = json.as_array().ok_or_else(bad)?;
        if arr.is_empty() || arr.len() > MAX_GRADIENT_STOPS {
            return Err(format!("a gradient needs 1 to {MAX_GRADIENT_STOPS} stops"));
        }
        let mut stops = Vec::with_capacity(arr.len());
        for s in arr {
            let s = s
                .as_array()
                .filter(|a| a.len() == 4)
                .ok_or("each stop must be [t, r, g, b]")?;
            let mut stop = [0.0; 4];
            for (v, j) in stop.iter_mut().zip(s) {
                let x = j
                    .as_f64()
                    .filter(|x| x.is_finite())
                    .ok_or("stop values must be finite numbers")?;
                *v = x.clamp(0.0, 1.0);
            }
            stops.push(stop);
        }
        Ok(Self::from_stops(stops))
    }

    /// A gradient from stops `[t, r, g, b]` (sorted here; order kept for equal t).
    pub fn from_stops(mut stops: Vec<[f64; 4]>) -> Self {
        stops.sort_by(|a, b| a[0].total_cmp(&b[0]));
        Self { stops }
    }

    pub fn stops(&self) -> &[[f64; 4]] {
        &self.stops
    }

    /// Store as a JSON list of stops.
    pub fn to_value(&self) -> ParamValue {
        ParamValue::Other(serde_json::Value::Array(
            self.stops.iter().map(|s| serde_json::json!(s)).collect(),
        ))
    }

    /// The sRGB colour at `t`.
    pub fn eval(&self, t: f64) -> [f32; 3] {
        let s = &self.stops;
        let last = s.len() - 1;
        let rgb = |k: usize| [s[k][1] as f32, s[k][2] as f32, s[k][3] as f32];
        if t <= s[0][0] {
            return rgb(0);
        }
        if t >= s[last][0] {
            return rgb(last);
        }
        let i = s.iter().rposition(|p| p[0] <= t).unwrap_or(0).min(last - 1);
        let span = s[i + 1][0] - s[i][0];
        let u = if span > 0.0 {
            ((t - s[i][0]) / span) as f32
        } else {
            1.0
        };
        let (a, b) = (rgb(i), rgb(i + 1));
        std::array::from_fn(|c| a[c] + (b[c] - a[c]) * u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_is_plain_values() {
        let v: Vec<ParamValue> = serde_json::from_str(r#"[true, 3, 2.5, "perlin"]"#).unwrap();
        assert_eq!(
            v,
            vec![
                ParamValue::Bool(true),
                ParamValue::Int(3),
                ParamValue::Float(2.5),
                ParamValue::Text("perlin".into())
            ]
        );
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"[true,3,2.5,"perlin"]"#);
    }

    #[test]
    fn validate_coerces_and_clamps() {
        let p = ParamDef::metres("size", "Size", 100.0, 1.0, 1000.0);
        assert_eq!(
            p.validate(&ParamValue::Int(800)).unwrap(),
            ParamValue::Float(800.0)
        );
        assert_eq!(
            p.validate(&ParamValue::Float(5000.0)).unwrap(),
            ParamValue::Float(1000.0)
        );
        assert!(p.validate(&ParamValue::Text("x".into())).is_err());

        let e = ParamDef::choice("mode", "Mode", "add", &[("add", "Add"), ("max", "Max")]);
        assert!(e.validate(&ParamValue::Text("max".into())).is_ok());
        assert!(e.validate(&ParamValue::Text("nope".into())).is_err());
    }

    #[test]
    fn curve_validates_and_interpolates() {
        let def = ParamDef::curve("c", "Curve");
        let v: ParamValue = serde_json::from_str("[[1, 1], [0, 0], [0.5, 0.8], [2, 0]]").unwrap();
        // Sorted, clamped (2 -> 1 is a duplicate x of the first [1, 1], dropped).
        let stored = def.validate(&v).unwrap();
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            "[[0.0,0.0],[0.5,0.8],[1.0,1.0]]"
        );
        let c = Curve::parse(&stored).unwrap();
        assert_eq!(c.eval(0.0), 0.0);
        assert!((c.eval(0.5) - 0.8).abs() < 1e-12);
        assert_eq!(c.eval(1.0), 1.0);
        // Monotone data gives a monotone curve.
        let mut last = -1.0;
        for i in 0..=100 {
            let y = c.eval(i as f64 / 100.0);
            assert!(y >= last, "not monotone at {i}");
            last = y;
        }
        assert!(
            def.validate(&serde_json::from_str("[[0.5, 0.5]]").unwrap())
                .is_err()
        );
        assert!(def.validate(&ParamValue::Float(1.0)).is_err());
        // The default is the identity.
        let id = Curve::parse(&def.default).unwrap();
        assert!((id.eval(0.3) - 0.3).abs() < 1e-12);
    }

    #[test]
    fn gradient_validates_and_interpolates() {
        let def = ParamDef::gradient("g", "Gradient", &[[0.0, 0.0, 0.0, 0.0], [1.0, 1.0, 1.0, 1.0]]);
        let v: ParamValue = serde_json::from_str("[[1, 1, 0, 0], [0, 0, 0, 2], [0.5, 0, 1, 0]]").unwrap();
        let stored = def.validate(&v).unwrap();
        assert_eq!(
            serde_json::to_string(&stored).unwrap(),
            "[[0.0,0.0,0.0,1.0],[0.5,0.0,1.0,0.0],[1.0,1.0,0.0,0.0]]"
        );
        let g = Gradient::parse(&stored).unwrap();
        assert_eq!(g.eval(-1.0), [0.0, 0.0, 1.0]);
        assert_eq!(g.eval(0.25), [0.0, 0.5, 0.5]);
        assert_eq!(g.eval(0.75), [0.5, 0.5, 0.0]);
        assert_eq!(g.eval(2.0), [1.0, 0.0, 0.0]);
        assert!(def.validate(&serde_json::from_str("[]").unwrap()).is_err());
        assert!(
            def.validate(&serde_json::from_str("[[0, 1, 1]]").unwrap())
                .is_err()
        );
    }
}
