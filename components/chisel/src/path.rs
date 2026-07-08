//! A small builder over [`PathData`] so leaves author geometry fluently, plus
//! the arc/circle math (`PathData` has no arc verb; arcs are lowered to cubic
//! Beziers here, in the leaf's local coordinates).

use paint_list_api::items::{PathCommand, PathData};
use paint_list_api::LayoutPoint;

/// Fluent [`PathData`] builder.
///
/// ```ignore
/// let p = Path::new().move_to(0.0, 8.0).line_to(4.0, 2.0).line_to(8.0, 6.0).build();
/// ```
#[derive(Clone, Debug, Default)]
pub struct Path {
    cmds: Vec<PathCommand>,
}

/// Max sweep per cubic segment when flattening an arc (quarter turn).
const MAX_SEG_SWEEP: f32 = std::f32::consts::FRAC_PI_2;

impl Path {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_to(mut self, x: f32, y: f32) -> Self {
        self.cmds.push(PathCommand::MoveTo(LayoutPoint::new(x, y)));
        self
    }

    pub fn line_to(mut self, x: f32, y: f32) -> Self {
        self.cmds.push(PathCommand::LineTo(LayoutPoint::new(x, y)));
        self
    }

    pub fn quad_to(mut self, cx: f32, cy: f32, x: f32, y: f32) -> Self {
        self.cmds.push(PathCommand::QuadTo {
            control: LayoutPoint::new(cx, cy),
            to: LayoutPoint::new(x, y),
        });
        self
    }

    pub fn cubic_to(mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> Self {
        self.cmds.push(PathCommand::CurveTo {
            control1: LayoutPoint::new(c1x, c1y),
            control2: LayoutPoint::new(c2x, c2y),
            to: LayoutPoint::new(x, y),
        });
        self
    }

    pub fn close(mut self) -> Self {
        self.cmds.push(PathCommand::Close);
        self
    }

    /// Append a circular arc around `(cx, cy)` with radius `r` from
    /// `start` to `end` (radians; screen convention, y-down, `0` = +x,
    /// increasing = clockwise on screen). Starts with a `MoveTo` onto the
    /// arc if the path is empty, else a `LineTo` (pen joins the arc).
    /// The sweep is split into cubic segments of at most a quarter turn.
    pub fn arc(mut self, cx: f32, cy: f32, r: f32, start: f32, end: f32) -> Self {
        let sweep = end - start;
        let (sx, sy) = (cx + r * start.cos(), cy + r * start.sin());
        if self.cmds.is_empty() {
            self = self.move_to(sx, sy);
        } else {
            self = self.line_to(sx, sy);
        }
        let segments = (sweep.abs() / MAX_SEG_SWEEP).ceil().max(1.0) as u32;
        let delta = sweep / segments as f32;
        // Control-point distance for a cubic approximating a `delta` sweep.
        let k = 4.0 / 3.0 * (delta / 4.0).tan();
        let mut a0 = start;
        for _ in 0..segments {
            let a1 = a0 + delta;
            let (cos0, sin0) = (a0.cos(), a0.sin());
            let (cos1, sin1) = (a1.cos(), a1.sin());
            self = self.cubic_to(
                cx + r * (cos0 - k * sin0),
                cy + r * (sin0 + k * cos0),
                cx + r * (cos1 + k * sin1),
                cy + r * (sin1 - k * cos1),
                cx + r * cos1,
                cy + r * sin1,
            );
            a0 = a1;
        }
        self
    }

    pub fn build(self) -> PathData {
        PathData { commands: self.cmds }
    }

    /// An open polyline through `points` (empty/1-point input builds an
    /// empty/move-only path, which paints nothing).
    pub fn polyline(points: &[(f32, f32)]) -> PathData {
        let mut p = Self::new();
        let mut iter = points.iter();
        if let Some(&(x, y)) = iter.next() {
            p = p.move_to(x, y);
        }
        for &(x, y) in iter {
            p = p.line_to(x, y);
        }
        p.build()
    }

    /// A full circle around `(cx, cy)` with radius `r`, closed.
    pub fn circle(cx: f32, cy: f32, r: f32) -> PathData {
        Self::new()
            .arc(cx, cy, r, 0.0, std::f32::consts::TAU)
            .close()
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_emits_verbs_in_order() {
        let pd = Path::new().move_to(0.0, 0.0).line_to(1.0, 2.0).close().build();
        assert!(matches!(pd.commands[0], PathCommand::MoveTo(_)));
        assert!(matches!(pd.commands[1], PathCommand::LineTo(_)));
        assert!(matches!(pd.commands[2], PathCommand::Close));
    }

    #[test]
    fn circle_is_move_plus_four_cubics_closed() {
        let pd = Path::circle(5.0, 5.0, 4.0);
        assert_eq!(pd.commands.len(), 6, "move + 4 cubics + close");
        assert!(matches!(pd.commands[0], PathCommand::MoveTo(_)));
        assert!(pd.commands[1..5]
            .iter()
            .all(|c| matches!(c, PathCommand::CurveTo { .. })));
        assert!(matches!(pd.commands[5], PathCommand::Close));
    }

    #[test]
    fn arc_endpoints_land_on_the_circle() {
        // Quarter arc from 0 to 90 deg (y-down: +x around to +y).
        let pd = Path::new().arc(0.0, 0.0, 10.0, 0.0, std::f32::consts::FRAC_PI_2).build();
        let PathCommand::MoveTo(start) = pd.commands[0] else {
            panic!("arc starts with MoveTo")
        };
        assert!((start.x - 10.0).abs() < 1e-4 && start.y.abs() < 1e-4);
        let PathCommand::CurveTo { to, .. } = pd.commands[pd.commands.len() - 1] else {
            panic!("arc ends with a cubic")
        };
        assert!(to.x.abs() < 1e-4 && (to.y - 10.0).abs() < 1e-4);
    }

    #[test]
    fn polyline_joins_points() {
        let pd = Path::polyline(&[(0.0, 0.0), (2.0, 1.0), (4.0, 0.5)]);
        assert_eq!(pd.commands.len(), 3);
        assert!(Path::polyline(&[]).commands.is_empty());
    }
}
