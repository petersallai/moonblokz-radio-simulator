//! Geometry calculations for line-of-sight and obstacle intersection.
//!
//! Contains helper functions for:
//! - Point-in-shape tests (rectangles, circles)
//! - Segment-shape intersection tests
//! - Segment-segment intersection with collinear handling
//! - Distance calculations (squared distance to avoid sqrt in hot paths)

use super::types::{CirclePos, Obstacle, Point, RectPos};

/// Squared Euclidean distance in world units (avoids a sqrt in hot paths).
///
/// Using squared distance is a common optimization when comparing distances,
/// as we can compare d1² vs d2² without computing the expensive square root.
/// This is heavily used in range checks: is node B within range of node A?
///
/// # Parameters
///
/// * `a` - First point
/// * `b` - Second point
///
/// # Returns
///
/// The squared distance (dx² + dy²) as a float.
///
/// # Safety
/// Assumes world coordinates in range 0..=10000. For larger coordinates,
/// use f64 or switched to squared-distance comparisons throughout.
pub fn distance2(a: &Point, b: &Point) -> f32 {
    let dx = a.x as f32 - b.x as f32;
    let dy = a.y as f32 - b.y as f32;
    dx * dx + dy * dy
}

/// Convert squared distance back to distance (only when needed for RSSI calc).
pub fn distance_from_d2(d2: f32) -> f32 {
    d2.sqrt()
}

/// Check if a straight line between two points intersects any obstacle.
///
/// This is the main line-of-sight check used by the radio propagation model.
/// A transmission from point1 to point2 is blocked if the straight line between
/// them intersects any obstacle (circle or rectangle).
///
/// ## Degenerate Case Handling
///
/// If point1 == point2 (degenerate segment), treats it as a point-inside-obstacle
/// test rather than a segment intersection test.
///
/// # Parameters
///
/// * `point1` - Start point of the line segment (transmitter position)
/// * `point2` - End point of the line segment (receiver position)
/// * `obstacles` - List of all obstacles in the scene
///
/// # Returns
///
/// `true` if the line intersects any obstacle (line-of-sight blocked),
/// `false` if clear line-of-sight exists.
pub fn is_intersect(point1: &Point, point2: &Point, obstacles: &[Obstacle]) -> bool {
    // Early out if degenerate segment
    if point1.x == point2.x && point1.y == point2.y {
        // Treat as a point: intersects if the point is inside any obstacle
        for obs in obstacles {
            match obs {
                Obstacle::Rectangle { position, .. } => {
                    if point_in_rect(point1, &position) {
                        return true;
                    }
                }
                Obstacle::Circle { position, .. } => {
                    if point_in_circle(point1, &position) {
                        return true;
                    }
                }
            }
        }
        return false;
    }

    for obs in obstacles {
        match obs {
            Obstacle::Rectangle { position, .. } => {
                if segment_intersects_rect(point1, point2, &position) {
                    return true;
                }
            }
            Obstacle::Circle { position, .. } => {
                if segment_intersects_circle(point1, point2, &position) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------- Geometry helpers ----------

/// Normalize rectangle corners to (left,right,top,bottom) tuple.
fn rect_bounds(rect: &RectPos) -> (f64, f64, f64, f64) {
    let left = rect.top_left.x.min(rect.bottom_right.x);
    let right = rect.top_left.x.max(rect.bottom_right.x);
    let top = rect.top_left.y.min(rect.bottom_right.y);
    let bottom = rect.top_left.y.max(rect.bottom_right.y);
    (left, right, top, bottom)
}

/// Inclusive point-in-rectangle test.
pub fn point_in_rect(p: &Point, rect: &RectPos) -> bool {
    let (left, right, top, bottom) = rect_bounds(rect);
    p.x >= left && p.x <= right && p.y >= top && p.y <= bottom
}

/// Point-inside-circle test using integer-safe math internally.
pub fn point_in_circle(p: &Point, circle: &CirclePos) -> bool {
    let dx = p.x - circle.center.x;
    let dy = p.y - circle.center.y;
    let r2 = circle.radius * circle.radius;
    dx * dx + dy * dy <= r2
}

/// Segment vs. axis-aligned rectangle intersection test.
fn segment_intersects_rect(p1: &Point, p2: &Point, rect: &RectPos) -> bool {
    // Inside check
    if point_in_rect(p1, rect) || point_in_rect(p2, rect) {
        return true;
    }

    let (left, right, top, bottom) = rect_bounds(rect);
    let lt = Point { x: left, y: top };
    let rt = Point { x: right, y: top };
    let rb = Point { x: right, y: bottom };
    let lb = Point { x: left, y: bottom };

    // Check segment against each rectangle edge
    segments_intersect(p1, p2, &lt, &rt) || segments_intersect(p1, p2, &rt, &rb) || segments_intersect(p1, p2, &rb, &lb) || segments_intersect(p1, p2, &lb, &lt)
}

/// Segment vs. circle intersection using projection and clamped parameter t.
fn segment_intersects_circle(p1: &Point, p2: &Point, circle: &CirclePos) -> bool {
    // Distance from circle center to segment <= radius
    let x1 = p1.x as f32;
    let y1 = p1.y as f32;
    let x2 = p2.x as f32;
    let y2 = p2.y as f32;
    let cx = circle.center.x as f32;
    let cy = circle.center.y as f32;
    let r = circle.radius as f32;

    let dx = x2 - x1;
    let dy = y2 - y1;
    if dx == 0.0 && dy == 0.0 {
        let ddx = x1 - cx;
        let ddy = y1 - cy;
        return ddx * ddx + ddy * ddy <= r * r;
    }
    let t = ((cx - x1) * dx + (cy - y1) * dy) / (dx * dx + dy * dy);
    let t_clamped = t.max(0.0).min(1.0);
    let closest_x = x1 + t_clamped * dx;
    let closest_y = y1 + t_clamped * dy;
    let ddx = closest_x - cx;
    let ddy = closest_y - cy;
    ddx * ddx + ddy * ddy <= r * r
}

/// Orientation of ordered triplet (a,b,c): returns 1 if clockwise, -1 if
/// counter-clockwise, and 0 if collinear.
fn orientation(a: &Point, b: &Point, c: &Point) -> i32 {
    let ax = a.x;
    let ay = a.y;
    let bx = b.x;
    let by = b.y;
    let cx = c.x;
    let cy = c.y;
    let val = (by - ay) * (cx - bx) - (bx - ax) * (cy - by);
    if val > 0.0 {
        1
    } else if val < 0.0 {
        -1
    } else {
        0
    }
}

/// True if point b lies on segment a–c, assuming collinearity.
fn on_segment(a: &Point, b: &Point, c: &Point) -> bool {
    // Is b on segment a-c (assuming collinear)
    let min_x = a.x.min(c.x);
    let max_x = a.x.max(c.x);
    let min_y = a.y.min(c.y);
    let max_y = a.y.max(c.y);
    b.x >= min_x && b.x <= max_x && b.y >= min_y && b.y <= max_y
}

/// Robust segment–segment intersection including collinear overlap.
///
/// Uses the orientation-based algorithm which handles all cases correctly:
/// - Proper crossing intersection (segments cross at an interior point)
/// - Endpoint touching (segments meet at an endpoint)
/// - Collinear overlap (segments lie on the same line and overlap)
///
/// The algorithm works by computing orientations of point triplets to determine
/// if segments are on opposite sides of each other (proper intersection) or if
/// they have collinear points that lie on the opposite segment.
///
/// # Parameters
///
/// * `p1`, `q1` - Endpoints of the first segment
/// * `p2`, `q2` - Endpoints of the second segment
///
/// # Returns
///
/// `true` if the segments intersect or touch, `false` if they are disjoint.
pub fn segments_intersect(p1: &Point, q1: &Point, p2: &Point, q2: &Point) -> bool {
    let o1 = orientation(p1, q1, p2);
    let o2 = orientation(p1, q1, q2);
    let o3 = orientation(p2, q2, p1);
    let o4 = orientation(p2, q2, q1);

    if o1 != o2 && o3 != o4 {
        return true; // Proper intersection
    }
    // Special cases: collinear and overlapping endpoints
    (o1 == 0 && on_segment(p1, p2, q1)) || (o2 == 0 && on_segment(p1, q2, q1)) || (o3 == 0 && on_segment(p2, p1, q2)) || (o4 == 0 && on_segment(p2, q1, q2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn geometry_point_in_rect_and_circle() {
        let rect = RectPos {
            top_left: p(10.0, 10.0),
            bottom_right: p(20.0, 20.0),
        };
        assert!(point_in_rect(&p(10.0, 10.0), &rect));
        assert!(point_in_rect(&p(15.0, 15.0), &rect));
        assert!(point_in_rect(&p(20.0, 20.0), &rect));
        assert!(!point_in_rect(&p(9.0, 10.0), &rect));

        let circle = CirclePos {
            center: p(50.0, 50.0),
            radius: 10.0,
        };
        assert!(point_in_circle(&p(50.0, 50.0), &circle));
        assert!(point_in_circle(&p(60.0, 50.0), &circle));
        assert!(!point_in_circle(&p(61.0, 50.0), &circle));
    }

    #[test]
    fn geometry_segments_intersect_basic_cases() {
        let a = p(0.0, 0.0);
        let b = p(10.0, 10.0);
        let c = p(0.0, 10.0);
        let d = p(10.0, 0.0);
        assert!(segments_intersect(&a, &b, &c, &d));

        // Collinear overlap
        let e = p(0.0, 0.0);
        let f = p(10.0, 0.0);
        let g = p(5.0, 0.0);
        let h = p(15.0, 0.0);
        assert!(segments_intersect(&e, &f, &g, &h));

        // Disjoint
        let i = p(0.0, 0.0);
        let j = p(1.0, 1.0);
        let k = p(2.0, 2.0);
        let l = p(3.0, 3.0);
        assert!(!segments_intersect(&i, &j, &k, &l));
    }

    #[test]
    fn is_intersect_handles_degenerate_segment() {
        let obstacles = vec![Obstacle::Rectangle {
            position: RectPos {
                top_left: p(0.0, 0.0),
                bottom_right: p(10.0, 10.0),
            },
        }];
        // Point inside rectangle → considered intersecting
        assert!(is_intersect(&p(5.0, 5.0), &p(5.0, 5.0), &obstacles));
        // Point outside → not intersecting
        assert!(!is_intersect(&p(20.0, 20.0), &p(20.0, 20.0), &obstacles));
    }
}
