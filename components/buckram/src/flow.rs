//! CSS logical axes and their physical mappings.
//!
//! The side table follows CSS Writing Modes Level 4 section 6.4:
//! <https://drafts.csswg.org/css-writing-modes-4/#logical-to-physical>.

/// One of the two axes in a box's writing mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LogicalAxis {
    Inline,
    Block,
}

/// The inherited inline base direction.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

/// The inherited block-flow and typographic mode.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
    SidewaysRl,
    SidewaysLr,
}

/// A physical side used at CSS consumer and backend boundaries.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PhysicalSide {
    Top,
    Right,
    Bottom,
    Left,
}

/// The flow-relative axes for one CSS box.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FlowAxes {
    pub writing_mode: WritingMode,
    pub direction: Direction,
}

impl FlowAxes {
    pub const HORIZONTAL_LTR: Self = Self {
        writing_mode: WritingMode::HorizontalTb,
        direction: Direction::Ltr,
    };

    pub const fn new(writing_mode: WritingMode, direction: Direction) -> Self {
        Self {
            writing_mode,
            direction,
        }
    }

    pub const fn is_horizontal(self) -> bool {
        matches!(self.writing_mode, WritingMode::HorizontalTb)
    }

    pub const fn block_start(self) -> PhysicalSide {
        match self.writing_mode {
            WritingMode::HorizontalTb => PhysicalSide::Top,
            WritingMode::VerticalRl | WritingMode::SidewaysRl => PhysicalSide::Right,
            WritingMode::VerticalLr | WritingMode::SidewaysLr => PhysicalSide::Left,
        }
    }

    pub const fn block_end(self) -> PhysicalSide {
        opposite(self.block_start())
    }

    pub const fn inline_start(self) -> PhysicalSide {
        match (self.writing_mode, self.direction) {
            (WritingMode::HorizontalTb, Direction::Ltr) => PhysicalSide::Left,
            (WritingMode::HorizontalTb, Direction::Rtl) => PhysicalSide::Right,
            (
                WritingMode::VerticalRl | WritingMode::VerticalLr | WritingMode::SidewaysRl,
                Direction::Ltr,
            ) => PhysicalSide::Top,
            (
                WritingMode::VerticalRl | WritingMode::VerticalLr | WritingMode::SidewaysRl,
                Direction::Rtl,
            ) => PhysicalSide::Bottom,
            (WritingMode::SidewaysLr, Direction::Ltr) => PhysicalSide::Bottom,
            (WritingMode::SidewaysLr, Direction::Rtl) => PhysicalSide::Top,
        }
    }

    pub const fn inline_end(self) -> PhysicalSide {
        opposite(self.inline_start())
    }

    pub const fn logical_size(self, physical: PhysicalSize) -> LogicalSize {
        if self.is_horizontal() {
            LogicalSize {
                inline: physical.width,
                block: physical.height,
            }
        } else {
            LogicalSize {
                inline: physical.height,
                block: physical.width,
            }
        }
    }

    pub const fn physical_size(self, logical: LogicalSize) -> PhysicalSize {
        if self.is_horizontal() {
            PhysicalSize {
                width: logical.inline,
                height: logical.block,
            }
        } else {
            PhysicalSize {
                width: logical.block,
                height: logical.inline,
            }
        }
    }

    /// Convert a physical rectangle in one containing block into logical
    /// coordinates for this writing mode.
    pub fn logical_rect(
        self,
        physical: PhysicalRect,
        containing_block: PhysicalSize,
    ) -> LogicalRect {
        let size = self.logical_size(PhysicalSize {
            width: physical.width,
            height: physical.height,
        });
        let inline_start = match self.inline_start() {
            PhysicalSide::Top => physical.y,
            PhysicalSide::Right => containing_block.width - physical.x - physical.width,
            PhysicalSide::Bottom => containing_block.height - physical.y - physical.height,
            PhysicalSide::Left => physical.x,
        };
        let block_start = match self.block_start() {
            PhysicalSide::Top => physical.y,
            PhysicalSide::Right => containing_block.width - physical.x - physical.width,
            PhysicalSide::Bottom => containing_block.height - physical.y - physical.height,
            PhysicalSide::Left => physical.x,
        };
        LogicalRect {
            inline_start,
            block_start,
            inline_size: size.inline,
            block_size: size.block,
        }
    }

    /// Derive a physical rectangle at a consumer edge from logical geometry.
    pub fn physical_rect(
        self,
        logical: LogicalRect,
        containing_block: PhysicalSize,
    ) -> PhysicalRect {
        let size = self.physical_size(LogicalSize {
            inline: logical.inline_size,
            block: logical.block_size,
        });
        let inline_start = match self.inline_start() {
            PhysicalSide::Top => (None, Some(logical.inline_start)),
            PhysicalSide::Right => (
                Some(containing_block.width - logical.inline_start - size.width),
                None,
            ),
            PhysicalSide::Bottom => (
                None,
                Some(containing_block.height - logical.inline_start - size.height),
            ),
            PhysicalSide::Left => (Some(logical.inline_start), None),
        };
        let block_start = match self.block_start() {
            PhysicalSide::Top => (None, Some(logical.block_start)),
            PhysicalSide::Right => (
                Some(containing_block.width - logical.block_start - size.width),
                None,
            ),
            PhysicalSide::Bottom => (
                None,
                Some(containing_block.height - logical.block_start - size.height),
            ),
            PhysicalSide::Left => (Some(logical.block_start), None),
        };
        PhysicalRect {
            x: inline_start.0.or(block_start.0).unwrap_or_default(),
            y: inline_start.1.or(block_start.1).unwrap_or_default(),
            width: size.width,
            height: size.height,
        }
    }
}

const fn opposite(side: PhysicalSide) -> PhysicalSide {
    match side {
        PhysicalSide::Top => PhysicalSide::Bottom,
        PhysicalSide::Right => PhysicalSide::Left,
        PhysicalSide::Bottom => PhysicalSide::Top,
        PhysicalSide::Left => PhysicalSide::Right,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalSides<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> PhysicalSides<T> {
    pub const fn splat(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub fn map<U>(self, mut map: impl FnMut(T) -> U) -> PhysicalSides<U> {
        PhysicalSides {
            top: map(self.top),
            right: map(self.right),
            bottom: map(self.bottom),
            left: map(self.left),
        }
    }

    pub fn zip_map<U: Copy, V>(
        self,
        other: PhysicalSides<U>,
        mut map: impl FnMut(T, U) -> V,
    ) -> PhysicalSides<V> {
        PhysicalSides {
            top: map(self.top, other.top),
            right: map(self.right, other.right),
            bottom: map(self.bottom, other.bottom),
            left: map(self.left, other.left),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalSides<T> {
    pub inline_start: T,
    pub inline_end: T,
    pub block_start: T,
    pub block_end: T,
}

impl FlowAxes {
    pub fn logical_sides<T: Copy>(self, physical: PhysicalSides<T>) -> LogicalSides<T> {
        let side = |physical_side| match physical_side {
            PhysicalSide::Top => physical.top,
            PhysicalSide::Right => physical.right,
            PhysicalSide::Bottom => physical.bottom,
            PhysicalSide::Left => physical.left,
        };
        LogicalSides {
            inline_start: side(self.inline_start()),
            inline_end: side(self.inline_end()),
            block_start: side(self.block_start()),
            block_end: side(self.block_end()),
        }
    }

    pub fn physical_sides<T: Copy>(self, logical: LogicalSides<T>) -> PhysicalSides<T> {
        let side = |physical_side| {
            if physical_side == self.inline_start() {
                logical.inline_start
            } else if physical_side == self.inline_end() {
                logical.inline_end
            } else if physical_side == self.block_start() {
                logical.block_start
            } else {
                logical.block_end
            }
        };
        PhysicalSides {
            top: side(PhysicalSide::Top),
            right: side(PhysicalSide::Right),
            bottom: side(PhysicalSide::Bottom),
            left: side(PhysicalSide::Left),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalSize {
    pub inline: f32,
    pub block: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Geometry in a fragment's inline and block axes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRect {
    pub inline_start: f32,
    pub block_start: f32,
    pub inline_size: f32,
    pub block_size: f32,
}

impl LogicalRect {
    /// Compatibility with the current horizontal physical layout lane.
    pub fn from_horizontal_physical(rect: PhysicalRect) -> Self {
        Self {
            inline_start: rect.x,
            block_start: rect.y,
            inline_size: rect.width,
            block_size: rect.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTAINING: PhysicalSize = PhysicalSize {
        width: 300.0,
        height: 200.0,
    };
    const PHYSICAL: PhysicalRect = PhysicalRect {
        x: 30.0,
        y: 20.0,
        width: 70.0,
        height: 40.0,
    };

    #[test]
    fn abstract_sides_follow_writing_modes_level_four() {
        let cases = [
            (
                FlowAxes::new(WritingMode::HorizontalTb, Direction::Ltr),
                (PhysicalSide::Top, PhysicalSide::Left),
            ),
            (
                FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl),
                (PhysicalSide::Top, PhysicalSide::Right),
            ),
            (
                FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr),
                (PhysicalSide::Right, PhysicalSide::Top),
            ),
            (
                FlowAxes::new(WritingMode::VerticalRl, Direction::Rtl),
                (PhysicalSide::Right, PhysicalSide::Bottom),
            ),
            (
                FlowAxes::new(WritingMode::VerticalLr, Direction::Ltr),
                (PhysicalSide::Left, PhysicalSide::Top),
            ),
            (
                FlowAxes::new(WritingMode::SidewaysLr, Direction::Ltr),
                (PhysicalSide::Left, PhysicalSide::Bottom),
            ),
            (
                FlowAxes::new(WritingMode::SidewaysLr, Direction::Rtl),
                (PhysicalSide::Left, PhysicalSide::Top),
            ),
        ];

        for (axes, (block_start, inline_start)) in cases {
            assert_eq!(axes.block_start(), block_start);
            assert_eq!(axes.inline_start(), inline_start);
            assert_eq!(axes.block_end(), opposite(block_start));
            assert_eq!(axes.inline_end(), opposite(inline_start));
        }
    }

    #[test]
    fn logical_and_physical_rects_round_trip_in_every_supported_mode() {
        for writing_mode in [
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::VerticalLr,
            WritingMode::SidewaysRl,
            WritingMode::SidewaysLr,
        ] {
            for direction in [Direction::Ltr, Direction::Rtl] {
                let axes = FlowAxes::new(writing_mode, direction);
                let logical = axes.logical_rect(PHYSICAL, CONTAINING);

                assert_eq!(axes.physical_rect(logical, CONTAINING), PHYSICAL);
            }
        }
    }

    #[test]
    fn vertical_rl_uses_right_and_top_as_starts_for_ltr() {
        let logical = FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr)
            .logical_rect(PHYSICAL, CONTAINING);

        assert_eq!(
            logical,
            LogicalRect {
                inline_start: 20.0,
                block_start: 200.0,
                inline_size: 40.0,
                block_size: 70.0,
            }
        );
    }
}
