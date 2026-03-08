use lyon_geom::{Box2D, CubicBezierSegment, LineSegment, Point, QuadraticBezierSegment, SvgArc};

use super::Turtle;

/// Generates a bounding box for all draw operations, used to properly apply [crate::ConversionConfig::origin]
#[derive(Debug, Default)]
pub struct PreprocessTurtle {
    pub bounding_box: Box2D<f64>,
}

impl Turtle for PreprocessTurtle {
    fn begin(&mut self) {}

    fn end(&mut self) {}

    fn comment(&mut self, _comment: String) {}

    fn move_to(&mut self, to: Point<f64>) {
        self.bounding_box = Box2D::from_points([self.bounding_box.min, self.bounding_box.max, to]);
    }

    fn line_to(&mut self, to: Point<f64>) {
        self.bounding_box = Box2D::from_points([self.bounding_box.min, self.bounding_box.max, to]);
    }

    fn arc(&mut self, svg_arc: SvgArc<f64>) {
        if svg_arc.is_straight_line() {
            self.line_to(svg_arc.to);
        } else {
            self.bounding_box = self.bounding_box.union(&svg_arc.to_arc().bounding_box());
        }
    }

    fn cubic_bezier(&mut self, cbs: CubicBezierSegment<f64>) {
        self.bounding_box = self.bounding_box.union(&cbs.bounding_box());
    }

    fn quadratic_bezier(&mut self, qbs: QuadraticBezierSegment<f64>) {
        self.bounding_box = self.bounding_box.union(&qbs.bounding_box());
    }

    fn set_layer_overrides(&mut self, _feedrate: Option<f64>, _power: Option<f64>) {
        // No-op: preprocessing only computes the bounding box
    }
}

/// A turtle that records all drawn geometry as a flat list of [`LineSegment`]s
/// (in whatever coordinate space it is driven from — typically millimeters when
/// wrapped by [`super::DpiConvertingTurtle`]).
///
/// Curves are flattened to line segments using the provided `tolerance` value
/// (same units as the coordinate space, e.g. mm).  The collected segments can
/// then be used for scanline fill intersection.
///
/// `move_to` calls are **not** recorded as segments — they represent "pen up"
/// travel moves that set the anchor for the next drawn segment.  Only `line_to`
/// and curve-flatten outputs become segments.
///
/// The current "pen" position is tracked internally so that each `line_to`
/// produces a segment from the previous position (set by either `move_to` or a
/// preceding `line_to`) to the new target.
///
/// Before any `move_to` has been issued the pen has no meaningful position, so
/// a `line_to` without a prior `move_to` is silently ignored.
#[derive(Debug)]
pub struct GeometryCollectorTurtle {
    /// All collected edge segments, in the order they were drawn.
    pub segments: Vec<LineSegment<f64>>,
    /// Tolerance used when flattening curves.
    pub tolerance: f64,
    /// Current pen position, or `None` if no `move_to` has been issued yet.
    current: Option<Point<f64>>,
}

impl GeometryCollectorTurtle {
    /// Create a new collector with the given curve-flattening `tolerance`.
    pub fn new(tolerance: f64) -> Self {
        Self {
            segments: Vec::new(),
            tolerance,
            current: None,
        }
    }

    /// Record a line segment from `self.current` to `to`, then advance `current`.
    /// If no `move_to` has been issued yet, the segment is silently ignored.
    fn push_segment(&mut self, to: Point<f64>) {
        if let Some(from) = self.current {
            if from != to {
                self.segments.push(LineSegment { from, to });
            }
        }
        self.current = Some(to);
    }
}

impl Turtle for GeometryCollectorTurtle {
    fn begin(&mut self) {}
    fn end(&mut self) {}
    fn comment(&mut self, _comment: String) {}

    fn move_to(&mut self, to: Point<f64>) {
        self.current = Some(to);
    }

    fn line_to(&mut self, to: Point<f64>) {
        self.push_segment(to);
    }

    fn arc(&mut self, svg_arc: SvgArc<f64>) {
        if svg_arc.is_straight_line() {
            self.push_segment(svg_arc.to);
        } else {
            // `Arc::flattened` yields the *endpoints* of each linear approximation
            // segment.  We treat each yielded point as a `line_to`.
            for pt in svg_arc.to_arc().flattened(self.tolerance) {
                self.push_segment(pt);
            }
        }
    }

    fn cubic_bezier(&mut self, cbs: CubicBezierSegment<f64>) {
        for pt in cbs.flattened(self.tolerance) {
            self.push_segment(pt);
        }
    }

    fn quadratic_bezier(&mut self, qbs: QuadraticBezierSegment<f64>) {
        self.cubic_bezier(qbs.to_cubic());
    }

    fn set_layer_overrides(&mut self, _feedrate: Option<f64>, _power: Option<f64>) {
        // No-op
    }
}

/// Compute scanline intersection spans for a set of edge segments at a given Y.
///
/// For each segment that crosses (or touches) `y`, the X coordinate of the
/// intersection is collected.  The list is sorted and returned.
///
/// Uses the **even-odd** fill rule: the caller should draw between x[0]→x[1],
/// x[2]→x[3], etc.
///
/// A horizontal segment is ignored because it lies *on* the scan line and
/// contributes infinitely many intersection points.
///
/// Endpoints are handled carefully so that a shared vertex between two segments
/// is counted only once (the upper-endpoint rule: the lower `y` endpoint of
/// each segment is excluded), which prevents double-counting at exact corners.
fn scanline_intersections(segments: &[LineSegment<f64>], y: f64) -> Vec<f64> {
    let mut xs = Vec::new();
    for seg in segments {
        let y0 = seg.from.y;
        let y1 = seg.to.y;

        // Skip horizontal segments — they lie on the scan line.
        if (y1 - y0).abs() < f64::EPSILON {
            continue;
        }

        let (y_min, y_max) = if y0 < y1 { (y0, y1) } else { (y1, y0) };

        // Upper-endpoint rule: include y_min, exclude y_max.
        // This prevents double-counting the shared point between adjacent segments.
        if y < y_min || y >= y_max {
            continue;
        }

        // Linear interpolation to find x at this y.
        let t = (y - y0) / (y1 - y0);
        let x = seg.from.x + t * (seg.to.x - seg.from.x);
        xs.push(x);
    }
    xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs
}

/// Generate hatch fill line segments that are clipped to the interior of the
/// shape described by `segments`.
///
/// `segments` should be the flattened edge segments produced by
/// [`GeometryCollectorTurtle`] (in mm, after DPI conversion).
///
/// `beam_width` is the spacing between scan lines in the same units.
///
/// Lines are returned in boustrophedon (alternating left-right / right-left)
/// order for efficient laser travel.
///
/// This uses the **even-odd fill rule**: for each scan line the intersection
/// X values are sorted and segments are drawn between pair (0,1), (2,3), etc.
pub fn scanline_fill_lines(
    segments: &[LineSegment<f64>],
    beam_width: f64,
) -> Vec<(Point<f64>, Point<f64>)> {
    use lyon_geom::point;

    if beam_width <= 0.0 || segments.is_empty() {
        return vec![];
    }

    // Compute bounding box from the segments.
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for seg in segments {
        for pt in [seg.from, seg.to] {
            if pt.x < min_x {
                min_x = pt.x;
            }
            if pt.x > max_x {
                max_x = pt.x;
            }
            if pt.y < min_y {
                min_y = pt.y;
            }
            if pt.y > max_y {
                max_y = pt.y;
            }
        }
    }
    if min_x >= max_x || min_y >= max_y {
        return vec![];
    }

    let mut result = Vec::new();
    let mut y = min_y + beam_width * 0.5;
    let mut left_to_right = true;

    while y <= max_y + beam_width * 0.5 {
        let scan_y = y.min(max_y);
        let xs = scanline_intersections(segments, scan_y);

        // Even-odd pairs: (xs[0], xs[1]), (xs[2], xs[3]), …
        let mut i = 0;
        while i + 1 < xs.len() {
            let x0 = xs[i];
            let x1 = xs[i + 1];
            // Skip degenerate zero-width spans.
            if (x1 - x0).abs() > f64::EPSILON {
                if left_to_right {
                    result.push((point(x0, scan_y), point(x1, scan_y)));
                } else {
                    result.push((point(x1, scan_y), point(x0, scan_y)));
                }
            }
            i += 2;
        }

        left_to_right = !left_to_right;
        y += beam_width;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyon_geom::point;

    /// A unit square boundary as four line segments, going clockwise.
    fn unit_square_segments() -> Vec<LineSegment<f64>> {
        vec![
            LineSegment {
                from: point(0., 0.),
                to: point(1., 0.),
            },
            LineSegment {
                from: point(1., 0.),
                to: point(1., 1.),
            },
            LineSegment {
                from: point(1., 1.),
                to: point(0., 1.),
            },
            LineSegment {
                from: point(0., 1.),
                to: point(0., 0.),
            },
        ]
    }

    /// Build a circle approximation as a polygon (N-sided) with radius `r`
    /// centred at (cx, cy).
    fn circle_polygon(cx: f64, cy: f64, r: f64, n: usize) -> Vec<LineSegment<f64>> {
        let pts: Vec<Point<f64>> = (0..n)
            .map(|i| {
                let angle = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
                point(cx + r * angle.cos(), cy + r * angle.sin())
            })
            .collect();
        let mut segs = Vec::with_capacity(n);
        for i in 0..n {
            segs.push(LineSegment {
                from: pts[i],
                to: pts[(i + 1) % n],
            });
        }
        segs
    }

    // ── scanline_intersections ────────────────────────────────────────────────

    #[test]
    fn intersections_on_unit_square_vertical_edges() {
        let segs = unit_square_segments();
        // y=0.5 should hit the left (x=0) and right (x=1) edges.
        let xs = scanline_intersections(&segs, 0.5);
        assert_eq!(xs.len(), 2, "expected 2 intersections, got {xs:?}");
        assert!((xs[0] - 0.0).abs() < 1e-10);
        assert!((xs[1] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn intersections_empty_for_y_above_shape() {
        let segs = unit_square_segments();
        let xs = scanline_intersections(&segs, 1.5);
        assert!(
            xs.is_empty(),
            "expected no intersections above shape, got {xs:?}"
        );
    }

    #[test]
    fn intersections_empty_for_y_below_shape() {
        let segs = unit_square_segments();
        let xs = scanline_intersections(&segs, -0.5);
        assert!(
            xs.is_empty(),
            "expected no intersections below shape, got {xs:?}"
        );
    }

    #[test]
    fn intersections_upper_endpoint_rule_no_double_count() {
        // A V-shape: two edges sharing a bottom vertex at (0.5, 0.0).
        // y = 0.0 is the *minimum* of both edges so both edges exclude y_max (the
        // top endpoints at y=1.0) and include y_min (y=0.0).  But at y=0.0 the
        // shared vertex at y_min is actually *included* (y >= y_min).
        // This test verifies we get exactly 2 intersections (the two top ends)
        // when the scan line passes through the interior (not the vertex).
        let segs = vec![
            LineSegment {
                from: point(0.0, 1.0),
                to: point(0.5, 0.0),
            },
            LineSegment {
                from: point(0.5, 0.0),
                to: point(1.0, 1.0),
            },
        ];
        let xs = scanline_intersections(&segs, 0.5);
        assert_eq!(xs.len(), 2, "expected 2, got {xs:?}");
    }

    // ── scanline_fill_lines ───────────────────────────────────────────────────

    #[test]
    fn fill_unit_square_all_x_within_bounds() {
        let segs = unit_square_segments();
        let lines = scanline_fill_lines(&segs, 0.1);
        assert!(!lines.is_empty(), "expected fill lines for unit square");
        for (from, to) in &lines {
            assert!(
                from.x >= -1e-9 && from.x <= 1.0 + 1e-9,
                "from.x {} out of range",
                from.x
            );
            assert!(
                to.x >= -1e-9 && to.x <= 1.0 + 1e-9,
                "to.x {} out of range",
                to.x
            );
            assert!(
                from.y >= -1e-9 && from.y <= 1.0 + 1e-9,
                "from.y {} out of range",
                from.y
            );
        }
    }

    #[test]
    fn fill_circle_all_x_within_radius() {
        let cx = 5.0_f64;
        let cy = 5.0_f64;
        let r = 4.0_f64;
        // Use a 256-gon as a good circle approximation.
        let segs = circle_polygon(cx, cy, r, 256);
        let lines = scanline_fill_lines(&segs, 0.2);

        assert!(!lines.is_empty(), "expected fill lines for circle");

        for (from, to) in &lines {
            // Each endpoint must lie within the circle (plus tiny epsilon for
            // polygon approximation error).
            let eps = 0.02; // ≈ polygon approximation error for 256 sides
            let dist_from = ((from.x - cx).powi(2) + (from.y - cy).powi(2)).sqrt();
            let dist_to = ((to.x - cx).powi(2) + (to.y - cy).powi(2)).sqrt();
            assert!(
                dist_from <= r + eps,
                "from point ({}, {}) is outside circle (dist={:.4})",
                from.x,
                from.y,
                dist_from
            );
            assert!(
                dist_to <= r + eps,
                "to point ({}, {}) is outside circle (dist={:.4})",
                to.x,
                to.y,
                dist_to
            );
        }
    }

    #[test]
    fn fill_empty_segments_produces_no_lines() {
        let lines = scanline_fill_lines(&[], 0.1);
        assert!(lines.is_empty());
    }

    #[test]
    fn fill_zero_beam_width_produces_no_lines() {
        let segs = unit_square_segments();
        let lines = scanline_fill_lines(&segs, 0.0);
        assert!(lines.is_empty());
    }

    #[test]
    fn fill_negative_beam_width_produces_no_lines() {
        let segs = unit_square_segments();
        let lines = scanline_fill_lines(&segs, -0.5);
        assert!(lines.is_empty());
    }

    #[test]
    fn fill_boustrophedon_ordering() {
        // With a square, consecutive scan lines should alternate direction.
        let segs = unit_square_segments();
        let lines = scanline_fill_lines(&segs, 0.25);
        // First line should go left-to-right (from.x < to.x).
        // Second line should go right-to-left (from.x > to.x).
        if lines.len() >= 2 {
            assert!(lines[0].0.x < lines[0].1.x, "first line should be L→R");
            assert!(lines[1].0.x > lines[1].1.x, "second line should be R→L");
        }
    }

    // ── GeometryCollectorTurtle ───────────────────────────────────────────────

    #[test]
    fn collector_move_to_does_not_add_segment() {
        let mut t = GeometryCollectorTurtle::new(0.01);
        t.move_to(point(1.0, 2.0));
        assert!(t.segments.is_empty());
    }

    #[test]
    fn collector_line_after_move_adds_segment() {
        let mut t = GeometryCollectorTurtle::new(0.01);
        t.move_to(point(0.0, 0.0));
        t.line_to(point(1.0, 0.0));
        assert_eq!(t.segments.len(), 1);
        assert_eq!(t.segments[0].from, point(0.0, 0.0));
        assert_eq!(t.segments[0].to, point(1.0, 0.0));
    }

    #[test]
    fn collector_multiple_lines_chain_correctly() {
        let mut t = GeometryCollectorTurtle::new(0.01);
        t.move_to(point(0.0, 0.0));
        t.line_to(point(1.0, 0.0));
        t.line_to(point(1.0, 1.0));
        t.line_to(point(0.0, 1.0));
        assert_eq!(t.segments.len(), 3);
        assert_eq!(t.segments[1].from, point(1.0, 0.0));
        assert_eq!(t.segments[2].to, point(0.0, 1.0));
    }

    #[test]
    fn collector_move_resets_pen_up() {
        let mut t = GeometryCollectorTurtle::new(0.01);
        t.move_to(point(0.0, 0.0));
        t.line_to(point(1.0, 0.0));
        // Second move lifts the pen; the next line_to should not connect.
        t.move_to(point(5.0, 5.0));
        t.line_to(point(6.0, 5.0));
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[1].from, point(5.0, 5.0));
    }

    #[test]
    fn collector_cubic_bezier_produces_segments() {
        let mut t = GeometryCollectorTurtle::new(0.01);
        t.move_to(point(0.0, 0.0));
        t.cubic_bezier(CubicBezierSegment {
            from: point(0.0, 0.0),
            ctrl1: point(0.33, 1.0),
            ctrl2: point(0.67, 1.0),
            to: point(1.0, 0.0),
        });
        assert!(
            !t.segments.is_empty(),
            "cubic bezier should produce flattened segments"
        );
        // All segment endpoints should be within the bezier's bounding box (with epsilon).
        for seg in &t.segments {
            assert!(seg.from.x >= -1e-6 && seg.from.x <= 1.0 + 1e-6);
            assert!(seg.from.y >= -1e-6 && seg.from.y <= 1.0 + 1e-6);
        }
    }
}
