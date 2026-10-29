//! Small vector helpers on `[f64; 3]`, in the zone's metres.

use folo_protocol::generated::Vec3 as ProtoVec3;

/// A point or vector in a zone, in metres: x right, y forward, z up.
pub type Vec3 = [f64; 3];

pub(crate) fn from_proto(v: &ProtoVec3) -> Vec3 {
    [v.x, v.y, v.z]
}

pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn scale(a: Vec3, k: f64) -> Vec3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub(crate) fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub(crate) fn length(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

pub(crate) fn lerp(a: Vec3, b: Vec3, t: f64) -> Vec3 {
    add(a, scale(sub(b, a), t))
}

/// The coefficients `(a, b)` of the orthographic projection of `p` onto the plane
/// `origin + a·u + b·v`, solved through the Gram matrix so `u` and `v` need not be
/// perpendicular. `None` when `u` and `v` are parallel or either has no length.
pub(crate) fn plane_coordinates(p: Vec3, origin: Vec3, u: Vec3, v: Vec3) -> Option<(f64, f64)> {
    let d = sub(p, origin);
    let (uu, uv, vv) = (dot(u, u), dot(u, v), dot(v, v));
    let det = uu * vv - uv * uv;
    // Relative to the product of the squared lengths, so tiny planes still work.
    if uu == 0.0 || vv == 0.0 || !det.is_finite() || det <= 1e-12 * uu * vv {
        return None;
    }
    let (du, dv) = (dot(d, u), dot(d, v));
    Some(((du * vv - dv * uv) / det, (dv * uu - du * uv) / det))
}

/// Whether the plane's edge vectors span a rectangle (neither is zero, and they are not parallel).
pub(crate) fn plane_is_valid(u: Vec3, v: Vec3) -> bool {
    plane_coordinates([0.0; 3], [0.0; 3], u, v).is_some()
}

/// A polyline with its cumulative arc lengths.
pub(crate) struct Polyline {
    points: Vec<Vec3>,
    /// `cumulative[i]` is the length from the first point to point `i`.
    cumulative: Vec<f64>,
}

impl Polyline {
    /// A polyline of at least two points.
    pub(crate) fn new(points: Vec<Vec3>) -> Self {
        debug_assert!(points.len() >= 2);
        let mut cumulative = Vec::with_capacity(points.len());
        let mut total = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                total += length(sub(*p, points[i - 1]));
            }
            cumulative.push(total);
        }
        Self { points, cumulative }
    }

    pub(crate) fn length(&self) -> f64 {
        self.cumulative.last().copied().unwrap_or(0.0)
    }

    /// The point at arc length `d` (clamped to the ends).
    pub(crate) fn point_at(&self, d: f64) -> Vec3 {
        let d = d.clamp(0.0, self.length());
        // The first segment whose end reaches `d`.
        let end = self
            .cumulative
            .partition_point(|&c| c < d)
            .clamp(1, self.points.len() - 1);
        let (c0, c1) = (self.cumulative[end - 1], self.cumulative[end]);
        let t = if c1 > c0 { (d - c0) / (c1 - c0) } else { 0.0 };
        lerp(self.points[end - 1], self.points[end], t)
    }

    /// The arc length of the point of the path nearest to `p`.
    pub(crate) fn nearest_arc_length(&self, p: Vec3) -> f64 {
        let mut best = (f64::INFINITY, 0.0);
        for (i, w) in self.points.windows(2).enumerate() {
            let seg = sub(w[1], w[0]);
            let len2 = dot(seg, seg);
            let t = if len2 > 0.0 {
                (dot(sub(p, w[0]), seg) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = add(w[0], scale(seg, t));
            let off = sub(p, q);
            let dist2 = dot(off, off);
            // `<` keeps the earlier segment on ties.
            if dist2 < best.0 {
                best = (dist2, self.cumulative[i] + t * len2.sqrt());
            }
        }
        best.1
    }
}
