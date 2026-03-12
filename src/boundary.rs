use glam::{Vec2, Vec3};

/// The closest point on a finite line segment `(a, b)` to point `p`.
fn closest_point_on_segment(a: Vec2, b: Vec2, p: Vec2) -> Vec2 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-10 {
        return a;
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    a + ab * t
}

/// Signed distance from point `p` to the **edge** of the polygon.
///
/// Returns a **positive** value when `p` is inside and **negative** when outside.
/// The magnitude is the distance to the nearest edge.
pub fn signed_distance_to_polygon(polygon: &[Vec2], p: Vec2) -> f32 {
    if polygon.len() < 3 {
        return f32::NEG_INFINITY;
    }

    // Minimum distance to any edge
    let mut min_dist = f32::MAX;
    let n = polygon.len();
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        let closest = closest_point_on_segment(a, b, p);
        let d = p.distance(closest);
        if d < min_dist {
            min_dist = d;
        }
    }

    // Point-in-polygon (ray casting) to determine sign
    let inside = point_in_polygon(polygon, p);
    if inside {
        min_dist
    } else {
        -min_dist
    }
}

/// Ray-casting point-in-polygon test.
fn point_in_polygon(polygon: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let n = polygon.len();
    let mut j = n - 1;
    for i in 0..n {
        let vi = polygon[i];
        let vj = polygon[j];
        if ((vi.y > p.y) != (vj.y > p.y))
            && (p.x < (vj.x - vi.x) * (p.y - vi.y) / (vj.y - vi.y) + vi.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Extract the X/Z components of a 3-D position (drops Y / height).
pub fn xz(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// This should calculate how visible the barrier should be based on proximity of the
/// position to the polygon
pub fn visibility_factor(polygon: &[Vec2], position: Vec3, fade_start: f32, fade_end: f32) -> f32 {
    let dist = signed_distance_to_polygon(polygon, xz(position));

    if dist <= fade_end {
        1.0
    } else if dist >= fade_start {
        0.0
    } else {
        1.0 - (dist - fade_end) / (fade_start - fade_end)
    }
}

/// Compute the maximum visibility factor across all tracked poses.
pub fn max_visibility(polygon: &[Vec2], positions: &[Vec3], fade_start: f32, fade_end: f32) -> f32 {
    positions
        .iter()
        .map(|&p| visibility_factor(polygon, p, fade_start, fade_end))
        .fold(0.0_f32, f32::max)
}