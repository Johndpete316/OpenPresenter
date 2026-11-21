//! "Sane" snapping (docs/EDITOR-PLAN.md, Interaction Rules → Snapping).
//!
//! - Only while dragging or resizing with the mouse (arrow nudges never snap).
//! - The threshold is in screen points, so it feels the same at any zoom.
//! - Targets, in priority order: canvas centre lines, canvas edges, safe-area
//!   edges, other elements' edges and centres, equal spacing, then the grid (only
//!   when it's on).
//! - At most one snap per axis: the nearest target wins; at the same distance the
//!   higher priority one does, so targets never fight.
//!
//! Pure geometry in canvas pixels, so it is unit-tested without a UI.

/// Default threshold, screen points.
pub const THRESHOLD: f32 = 6.0;
/// Safe area inset on each side, as a fraction of the canvas (title-safe 90%).
pub const SAFE_INSET: f64 = 0.05;

/// Axis-aligned bounds in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Bounds {
    pub fn of(r: &doc::Rect) -> Bounds {
        Bounds {
            x0: r.x,
            y0: r.y,
            x1: r.x + r.width,
            y1: r.y + r.height,
        }
    }

    fn span(&self, axis: Axis) -> (f64, f64) {
        match axis {
            Axis::X => (self.x0, self.x1),
            Axis::Y => (self.y0, self.y1),
        }
    }

    pub fn union(&self, o: &Bounds) -> Bounds {
        Bounds {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Horizontal position: the guide is a vertical line.
    X,
    Y,
}

impl Axis {
    fn other(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }
}

/// Target kinds, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    CanvasCentre,
    CanvasEdge,
    SafeArea,
    Element,
    Spacing,
    Grid,
}

/// What to draw for an active snap.
#[derive(Debug, Clone, PartialEq)]
pub enum Guide {
    /// A line at `pos` on `axis` (x = vertical line), spanning `from..to` on the
    /// other axis.
    Line {
        axis: Axis,
        pos: f64,
        from: f64,
        to: f64,
        kind: Kind,
    },
    /// Equal gaps of `gap` pixels along `axis`, drawn at `cross` on the other axis.
    Spacing {
        axis: Axis,
        gaps: Vec<(f64, f64)>,
        cross: f64,
        gap: f64,
    },
}

pub struct Targets {
    pub canvas: (f64, f64),
    /// Other elements (not the ones being moved).
    pub others: Vec<Bounds>,
    /// Grid spacing in canvas pixels, when the grid is on.
    pub grid: Option<f64>,
    /// Threshold in canvas pixels (screen threshold ÷ zoom).
    pub threshold: f64,
}

impl Targets {
    pub fn new(canvas: (f64, f64), others: Vec<Bounds>, grid: Option<f64>, scale: f32) -> Targets {
        Targets {
            canvas,
            others,
            grid,
            threshold: f64::from(THRESHOLD / scale.max(0.01)),
        }
    }

    fn lines(&self, axis: Axis) -> Vec<(f64, Kind)> {
        let len = match axis {
            Axis::X => self.canvas.0,
            Axis::Y => self.canvas.1,
        };
        let mut v = vec![
            (len / 2.0, Kind::CanvasCentre),
            (0.0, Kind::CanvasEdge),
            (len, Kind::CanvasEdge),
            (len * SAFE_INSET, Kind::SafeArea),
            (len * (1.0 - SAFE_INSET), Kind::SafeArea),
        ];
        for o in &self.others {
            let (a, b) = o.span(axis);
            v.extend([
                (a, Kind::Element),
                ((a + b) / 2.0, Kind::Element),
                (b, Kind::Element),
            ]);
        }
        v
    }
}

/// One candidate: move by `delta` on the axis.
struct Candidate {
    delta: f64,
    kind: Kind,
    guide: Guide,
}

fn better(a: &Candidate, b: &Candidate) -> bool {
    let (da, db) = (a.delta.abs(), b.delta.abs());
    da < db - 1e-9 || ((da - db).abs() <= 1e-9 && a.kind < b.kind)
}

/// Which points of the moving bounds may snap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Points {
    /// Both edges and the centre (moving).
    All,
    /// Only the low edge (resizing by the left / top).
    Low,
    /// Only the high edge (resizing by the right / bottom).
    High,
}

fn best_on_axis(b: &Bounds, axis: Axis, points: Points, t: &Targets) -> Option<Candidate> {
    let (a0, a1) = b.span(axis);
    let (c0, c1) = b.span(axis.other());
    let pts: Vec<f64> = match points {
        Points::All => vec![a0, (a0 + a1) / 2.0, a1],
        Points::Low => vec![a0],
        Points::High => vec![a1],
    };
    let mut best: Option<Candidate> = None;
    let mut offer = |c: Candidate| {
        if c.delta.abs() <= t.threshold && best.as_ref().is_none_or(|b| better(&c, b)) {
            best = Some(c);
        }
    };
    let other_len = match axis {
        Axis::X => t.canvas.1,
        Axis::Y => t.canvas.0,
    };
    for (pos, kind) in t.lines(axis) {
        for &p in &pts {
            // Guides for canvas lines run across the canvas; for elements, across
            // the moving box and the element.
            let (from, to) = if kind == Kind::Element {
                t.others
                    .iter()
                    .find(|o| {
                        let (x, y) = o.span(axis);
                        x == pos || y == pos || (x + y) / 2.0 == pos
                    })
                    .map_or((c0, c1), |o| {
                        let (p0, p1) = o.span(axis.other());
                        (p0.min(c0), p1.max(c1))
                    })
            } else {
                (0.0, other_len)
            };
            offer(Candidate {
                delta: pos - p,
                kind,
                guide: Guide::Line {
                    axis,
                    pos,
                    from,
                    to,
                    kind,
                },
            });
        }
    }
    if points == Points::All {
        for c in spacing(b, axis, t) {
            offer(c);
        }
    }
    if let Some(g) = t.grid.filter(|g| *g > 0.0) {
        for &p in &pts {
            let pos = (p / g).round() * g;
            offer(Candidate {
                delta: pos - p,
                kind: Kind::Grid,
                guide: Guide::Line {
                    axis,
                    pos,
                    from: 0.0,
                    to: other_len,
                    kind: Kind::Grid,
                },
            });
        }
    }
    best
}

/// Equal-spacing candidates along `axis`: continuing a pair's gap on either side,
/// or centring between two neighbours.
fn spacing(b: &Bounds, axis: Axis, t: &Targets) -> Vec<Candidate> {
    let (m0, m1) = b.span(axis);
    let (c0, c1) = b.span(axis.other());
    let w = m1 - m0;
    // Elements level with the moving one (overlapping on the other axis).
    let mut row: Vec<(f64, f64)> = t
        .others
        .iter()
        .filter(|o| {
            let (p0, p1) = o.span(axis.other());
            p0 < c1 && p1 > c0
        })
        .map(|o| o.span(axis))
        .collect();
    row.sort_by(|a, b| a.0.total_cmp(&b.0));
    let cross = (c0 + c1) / 2.0;
    let mut out = Vec::new();
    for (i, p) in row.iter().enumerate() {
        for q in &row[i + 1..] {
            let gap = q.0 - p.1;
            if gap <= 0.0 {
                continue;
            }
            // After q: [p] gap [q] gap [M].
            let at = q.1 + gap;
            out.push(Candidate {
                delta: at - m0,
                kind: Kind::Spacing,
                guide: Guide::Spacing {
                    axis,
                    gaps: vec![(p.1, q.0), (q.1, at)],
                    cross,
                    gap,
                },
            });
            // Before p: [M] gap [p] gap [q].
            let at = p.0 - gap;
            out.push(Candidate {
                delta: at - m1,
                kind: Kind::Spacing,
                guide: Guide::Spacing {
                    axis,
                    gaps: vec![(at, p.0), (p.1, q.0)],
                    cross,
                    gap,
                },
            });
            // Between p and q, with equal gaps.
            let free = q.0 - p.1 - w;
            if free > 0.0 {
                let g = free / 2.0;
                let at = p.1 + g;
                out.push(Candidate {
                    delta: at - m0,
                    kind: Kind::Spacing,
                    guide: Guide::Spacing {
                        axis,
                        gaps: vec![(p.1, at), (at + w, q.0)],
                        cross,
                        gap: g,
                    },
                });
            }
        }
    }
    out
}

/// Snap a box being moved: `(dx, dy)` to add, and the guides to show.
pub fn snap_move(b: &Bounds, t: &Targets) -> (f64, f64, Vec<Guide>) {
    let x = best_on_axis(b, Axis::X, Points::All, t);
    let y = best_on_axis(b, Axis::Y, Points::All, t);
    finish(x, y)
}

/// Snap the edges being resized (`None` = that axis isn't being resized).
pub fn snap_resize(
    b: &Bounds,
    x: Option<Points>,
    y: Option<Points>,
    t: &Targets,
) -> (f64, f64, Vec<Guide>) {
    let cx = x.and_then(|p| best_on_axis(b, Axis::X, p, t));
    let cy = y.and_then(|p| best_on_axis(b, Axis::Y, p, t));
    finish(cx, cy)
}

fn finish(x: Option<Candidate>, y: Option<Candidate>) -> (f64, f64, Vec<Guide>) {
    let mut guides = Vec::new();
    let dx = x.map_or(0.0, |c| {
        guides.push(c.guide);
        c.delta
    });
    let dy = y.map_or(0.0, |c| {
        guides.push(c.guide);
        c.delta
    });
    (dx, dy, guides)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(x: f64, y: f64, w: f64, h: f64) -> Bounds {
        Bounds {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
        }
    }

    fn targets(others: Vec<Bounds>, scale: f32) -> Targets {
        Targets::new((1000.0, 500.0), others, None, scale)
    }

    fn kind(g: &[Guide], axis: Axis) -> Option<Kind> {
        g.iter().find_map(|g| match g {
            Guide::Line { axis: a, kind, .. } if *a == axis => Some(*kind),
            Guide::Spacing { axis: a, .. } if *a == axis => Some(Kind::Spacing),
            _ => None,
        })
    }

    #[test]
    fn centres_on_the_canvas() {
        // 100 wide, centre at 497: 3 px from the canvas centre.
        let (dx, dy, g) = snap_move(&b(447.0, 120.0, 100.0, 40.0), &targets(vec![], 1.0));
        assert_eq!(dx, 3.0);
        assert_eq!(dy, 0.0, "nothing near on y");
        assert_eq!(kind(&g, Axis::X), Some(Kind::CanvasCentre));
        assert_eq!(g.len(), 1, "one guide");
    }

    #[test]
    fn priority_breaks_ties_and_nearest_wins_otherwise() {
        // An element edge exactly on the canvas centre line: the same distance,
        // so the canvas centre (higher priority) is the guide.
        let other = b(500.0, 300.0, 50.0, 50.0);
        // Moving box centre at 498: 2 px from both the canvas centre and the
        // other's left edge (both at 500).
        let (dx, _, g) = snap_move(&b(450.0, 100.0, 96.0, 40.0), &targets(vec![other], 1.0));
        assert_eq!(dx, 2.0);
        assert_eq!(kind(&g, Axis::X), Some(Kind::CanvasCentre));
    }

    #[test]
    fn element_edges_snap_when_nearer_than_the_canvas() {
        let other = b(300.0, 300.0, 100.0, 50.0);
        // Left edge 302: 2 px from the other's left edge (300); far from canvas lines.
        let (dx, _, g) = snap_move(&b(302.0, 100.0, 60.0, 40.0), &targets(vec![other], 1.0));
        assert_eq!(dx, -2.0);
        assert_eq!(kind(&g, Axis::X), Some(Kind::Element));
    }

    #[test]
    fn one_snap_per_axis() {
        let others = vec![b(300.0, 300.0, 100.0, 50.0), b(301.0, 10.0, 20.0, 20.0)];
        let (dx, _, g) = snap_move(&b(302.0, 100.0, 60.0, 40.0), &targets(others, 1.0));
        assert_eq!(dx, -1.0, "the nearest of two element edges");
        assert_eq!(
            g.iter()
                .filter(|g| matches!(g, Guide::Line { axis: Axis::X, .. }))
                .count(),
            1
        );
    }

    #[test]
    fn threshold_is_in_screen_points_at_any_zoom() {
        // 4 canvas px from the canvas centre.
        let m = b(446.0, 120.0, 100.0, 40.0);
        let at = |scale| snap_move(&m, &targets(vec![], scale)).0;
        assert_eq!(at(1.0), 4.0, "100%: 4 px is within 6 points");
        assert_eq!(at(2.0), 0.0, "200%: 4 px is 8 points, too far");
        assert_eq!(at(0.5), 4.0, "50%: 4 px is 2 points");
        // 10 canvas px: only snaps when zoomed out.
        let m = b(440.0, 120.0, 100.0, 40.0);
        assert_eq!(snap_move(&m, &targets(vec![], 0.5)).0, 10.0);
        assert_eq!(snap_move(&m, &targets(vec![], 1.0)).0, 0.0);
    }

    #[test]
    fn safe_area_edges_are_targets() {
        // Safe area left at 50 (5% of 1000).
        let (dx, _, g) = snap_move(&b(53.0, 120.0, 100.0, 40.0), &targets(vec![], 1.0));
        assert_eq!(dx, -3.0);
        assert_eq!(kind(&g, Axis::X), Some(Kind::SafeArea));
    }

    #[test]
    fn equal_spacing_continues_a_row() {
        // [100..200] gap 50 [250..350] gap ? [M]: M's left edge 403 → 400.
        let others = vec![b(100.0, 200.0, 100.0, 50.0), b(250.0, 200.0, 100.0, 50.0)];
        let (dx, _, g) = snap_move(&b(403.0, 205.0, 60.0, 40.0), &targets(others, 1.0));
        assert_eq!(dx, -3.0);
        let sp = g.iter().find_map(|g| match g {
            Guide::Spacing { gap, gaps, .. } => Some((*gap, gaps.clone())),
            _ => None,
        });
        assert_eq!(sp, Some((50.0, vec![(200.0, 250.0), (350.0, 400.0)])));
    }

    #[test]
    fn equal_spacing_centres_between_two() {
        // [0..100] and [300..400] on a row; M 60 wide centred → left 170.
        let others = vec![b(0.0, 200.0, 100.0, 50.0), b(300.0, 200.0, 100.0, 50.0)];
        let (dx, _, _) = snap_move(&b(172.0, 205.0, 60.0, 40.0), &targets(others, 1.0));
        assert_eq!(dx, -2.0);
    }

    #[test]
    fn grid_only_when_on() {
        let m = b(118.0, 117.0, 30.0, 30.0);
        assert_eq!(snap_move(&m, &targets(vec![], 1.0)).0, 0.0);
        let t = Targets::new((1000.0, 500.0), vec![], Some(40.0), 1.0);
        let (dx, dy, g) = snap_move(&m, &t);
        assert_eq!((dx, dy), (2.0, 3.0), "118 → 120, 117 → 120");
        assert_eq!(kind(&g, Axis::X), Some(Kind::Grid));
    }

    #[test]
    fn resizing_snaps_only_the_dragged_edge() {
        // Right edge at 497 → canvas centre 500; the left edge (447) isn't considered.
        let m = b(397.0, 120.0, 100.0, 40.0);
        let (dx, dy, _) = snap_resize(&m, Some(Points::High), None, &targets(vec![], 1.0));
        assert_eq!((dx, dy), (3.0, 0.0));
        let (dx, _, _) = snap_resize(&m, Some(Points::Low), None, &targets(vec![], 1.0));
        assert_eq!(dx, 0.0, "left edge 397: nothing within 6 px");
    }
}
