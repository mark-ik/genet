//! CSS block formatting inputs and normal-flow placement.
//!
//! This module owns the CSS-facing model. Backend styles are lowered into
//! these values at an adapter edge; they do not choose or define the block
//! algorithm.

use crate::{FlowAxes, IntrinsicSizes, LogicalRect, PhysicalRect, PhysicalSides, PhysicalSize};

/// A linear used-value expression: an absolute component plus a percentage
/// of the containing block's inline size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FlowLength {
    pub px: f32,
    pub percentage: f32,
}

impl FlowLength {
    pub const ZERO: Self = Self {
        px: 0.0,
        percentage: 0.0,
    };

    pub const fn px(px: f32) -> Self {
        Self {
            px,
            percentage: 0.0,
        }
    }

    pub const fn percent(percentage: f32) -> Self {
        Self {
            px: 0.0,
            percentage,
        }
    }

    pub fn resolve(self, containing_inline_size: f32) -> f32 {
        self.px + self.percentage * containing_inline_size
    }
}

/// A margin or inset value which may remain automatic until layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FlowLengthAuto {
    #[default]
    Auto,
    Value(FlowLength),
}

impl FlowLengthAuto {
    pub const ZERO: Self = Self::Value(FlowLength::ZERO);

    pub fn resolve(self, containing_inline_size: f32) -> Option<f32> {
        match self {
            Self::Auto => None,
            Self::Value(value) => Some(value.resolve(containing_inline_size)),
        }
    }
}

/// A CSS preferred, minimum, or maximum size before intrinsic querying.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum BlockSizeValue {
    #[default]
    Auto,
    None,
    Length(FlowLength),
    MinContent,
    MaxContent,
    FitContent(FlowLength),
}

impl BlockSizeValue {
    pub fn resolve_definite(self, containing_size: Option<f32>) -> Option<f32> {
        match (self, containing_size) {
            (Self::Length(value), Some(containing_size)) => Some(value.resolve(containing_size)),
            (Self::Length(value), None) if value.percentage == 0.0 => Some(value.px),
            _ => None,
        }
    }

    fn requires_intrinsic_query(self) -> bool {
        matches!(
            self,
            Self::MinContent | Self::MaxContent | Self::FitContent(_)
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockDimensions<T> {
    pub width: T,
    pub height: T,
}

impl<T> BlockDimensions<T> {
    pub const fn new(width: T, height: T) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlockBoxSizing {
    #[default]
    ContentBox,
    BorderBox,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlockPosition {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FloatSide {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ClearSide {
    #[default]
    None,
    Left,
    Right,
    Both,
}

/// Standards-owned outer-box inputs used by block flow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockStyle {
    /// Axes used by this box to lay out its own contents.
    pub flow: FlowAxes,
    /// Axes of the containing block in which this box participates.
    pub containing_flow: FlowAxes,
    pub size: BlockDimensions<BlockSizeValue>,
    pub min_size: BlockDimensions<BlockSizeValue>,
    pub max_size: BlockDimensions<BlockSizeValue>,
    pub margin: PhysicalSides<FlowLengthAuto>,
    pub padding: PhysicalSides<FlowLength>,
    pub border: PhysicalSides<f32>,
    pub box_sizing: BlockBoxSizing,
    pub position: BlockPosition,
    pub float: FloatSide,
    pub clear: ClearSide,
    pub establishes_bfc: bool,
    /// The used inline size must be obtained from CSS shrink-to-fit sizing.
    pub shrink_to_fit: bool,
    pub replaced: bool,
    pub aspect_ratio: Option<f32>,
    pub size_containment: BlockDimensions<bool>,
    pub has_nonlinear_lengths: bool,
    /// The principal box of the document element. Its margins never collapse.
    pub is_root_element: bool,
}

impl Default for BlockStyle {
    fn default() -> Self {
        Self {
            flow: FlowAxes::HORIZONTAL_LTR,
            containing_flow: FlowAxes::HORIZONTAL_LTR,
            size: BlockDimensions::new(BlockSizeValue::Auto, BlockSizeValue::Auto),
            min_size: BlockDimensions::new(BlockSizeValue::Auto, BlockSizeValue::Auto),
            max_size: BlockDimensions::new(BlockSizeValue::None, BlockSizeValue::None),
            margin: PhysicalSides::splat(FlowLengthAuto::ZERO),
            padding: PhysicalSides::splat(FlowLength::ZERO),
            border: PhysicalSides::splat(0.0),
            box_sizing: BlockBoxSizing::ContentBox,
            position: BlockPosition::Static,
            float: FloatSide::None,
            clear: ClearSide::None,
            establishes_bfc: false,
            shrink_to_fit: false,
            replaced: false,
            aspect_ratio: None,
            size_containment: BlockDimensions::new(false, false),
            has_nonlinear_lengths: false,
            is_root_element: false,
        }
    }
}

impl BlockStyle {
    pub fn anonymous(flow: FlowAxes, containing_flow: FlowAxes) -> Self {
        Self {
            flow,
            containing_flow,
            ..Self::default()
        }
    }

    /// Name the first feature that requires a later owned algorithm.
    pub fn deferral(self) -> Option<BlockDeferral> {
        if self.position != BlockPosition::Static {
            return Some(BlockDeferral::Positioning);
        }
        if self.shrink_to_fit {
            return Some(BlockDeferral::ShrinkToFit);
        }
        if self.float != FloatSide::None
            && !matches!(
                logical_dimension(self.containing_flow, self.size).inline,
                BlockSizeValue::Length(_)
            )
        {
            return Some(BlockDeferral::FloatShrinkToFit);
        }
        if self.replaced {
            return Some(BlockDeferral::Replaced);
        }
        if self.aspect_ratio.is_some() {
            return Some(BlockDeferral::AspectRatio);
        }
        if self.size_containment.width || self.size_containment.height {
            return Some(BlockDeferral::SizeContainment);
        }
        if self.has_nonlinear_lengths {
            return Some(BlockDeferral::NonlinearLength);
        }
        if [
            self.size.width,
            self.size.height,
            self.min_size.width,
            self.min_size.height,
            self.max_size.width,
            self.max_size.height,
        ]
        .into_iter()
        .any(BlockSizeValue::requires_intrinsic_query)
        {
            return Some(BlockDeferral::IntrinsicSize);
        }
        None
    }

    pub fn resolved_padding(self, containing_inline_size: f32) -> PhysicalSides<f32> {
        self.padding
            .map(|value| value.resolve(containing_inline_size))
    }

    pub fn logical_margin(self, containing_inline_size: f32) -> crate::LogicalSides<Option<f32>> {
        self.containing_flow.logical_sides(
            self.margin
                .map(|value| value.resolve(containing_inline_size)),
        )
    }

    pub fn logical_padding_border(self, containing_inline_size: f32) -> crate::LogicalSides<f32> {
        self.containing_flow.logical_sides(
            self.resolved_padding(containing_inline_size)
                .zip_map(self.border, |padding, border| padding + border),
        )
    }

    pub fn content_logical_padding_border(
        self,
        containing_inline_size: f32,
    ) -> crate::LogicalSides<f32> {
        self.flow.logical_sides(
            self.resolved_padding(containing_inline_size)
                .zip_map(self.border, |padding, border| padding + border),
        )
    }

    /// Which of this box's block edges may adjoin an in-flow child's margin.
    pub fn child_margin_collapse(
        self,
        containing_inline_size: f32,
        containing_block_size: Option<f32>,
        is_layout_root: bool,
        first_and_last_child_margins_adjoin: bool,
    ) -> BlockMarginCollapse {
        if is_layout_root || self.is_root_element || self.establishes_bfc {
            return BlockMarginCollapse::NONE;
        }

        let padding_border = self.content_logical_padding_border(containing_inline_size);
        let block_size = logical_dimension(self.flow, self.size).block;
        let min_block_size = logical_dimension(self.flow, self.min_size).block;
        BlockMarginCollapse {
            block_start: padding_border.block_start == 0.0,
            block_end: padding_border.block_end == 0.0
                && matches!(block_size, BlockSizeValue::Auto)
                && (!first_and_last_child_margins_adjoin
                    || min_block_size
                        .resolve_definite(containing_block_size)
                        .is_none_or(|minimum| minimum <= 0.0)),
        }
    }

    /// Whether this box has no block-axis separator, so adjoining margins may
    /// collapse through it.
    pub fn can_collapse_through(
        self,
        containing_inline_size: f32,
        containing_block_size: Option<f32>,
        is_layout_root: bool,
        has_line_boxes: bool,
        all_children_collapse_through: bool,
    ) -> bool {
        if is_layout_root
            || self.is_root_element
            || self.establishes_bfc
            || has_line_boxes
            || !all_children_collapse_through
        {
            return false;
        }

        let padding_border = self.content_logical_padding_border(containing_inline_size);
        let block_size = logical_dimension(self.flow, self.size).block;
        let min_block_size = logical_dimension(self.flow, self.min_size).block;
        let block_size_is_zero_or_auto = match block_size {
            BlockSizeValue::Auto => true,
            value => value
                .resolve_definite(containing_block_size)
                .is_some_and(|size| size == 0.0),
        };
        let min_block_size_is_zero = min_block_size
            .resolve_definite(containing_block_size)
            .is_none_or(|minimum| minimum <= 0.0);

        padding_border.block_start == 0.0
            && padding_border.block_end == 0.0
            && block_size_is_zero_or_auto
            && min_block_size_is_zero
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockDeferral {
    Positioning,
    ShrinkToFit,
    FloatShrinkToFit,
    FloatLineExclusion,
    FloatFormattingContextAvoidance,
    NestedFloatState,
    IntrinsicSize,
    IndependentFormattingContext,
    Replaced,
    AspectRatio,
    SizeContainment,
    NonlinearLength,
    ParentMarginCollapse,
    IndefiniteInlineSize,
    BackendSizingMode,
}

/// Positive and negative adjoining margins collapse independently.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CollapsedMargin {
    positive: f32,
    negative: f32,
}

impl CollapsedMargin {
    pub const ZERO: Self = Self {
        positive: 0.0,
        negative: 0.0,
    };

    pub fn from_margin(margin: f32) -> Self {
        if margin >= 0.0 {
            Self {
                positive: margin,
                negative: 0.0,
            }
        } else {
            Self {
                positive: 0.0,
                negative: margin,
            }
        }
    }

    pub fn collapse(mut self, margin: f32) -> Self {
        if margin >= 0.0 {
            self.positive = self.positive.max(margin);
        } else {
            self.negative = self.negative.min(margin);
        }
        self
    }

    pub fn collapse_with(self, other: Self) -> Self {
        Self {
            positive: self.positive.max(other.positive),
            negative: self.negative.min(other.negative),
        }
    }

    pub fn resolve(self) -> f32 {
        self.positive + self.negative
    }
}

/// The two adjoining margin sets produced by a block box.
///
/// These are algorithm outputs, not pre-resolved spacing. A parent may need to
/// combine either set with an ancestor or sibling before its used width is
/// known.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlockMarginState {
    pub block_start: CollapsedMargin,
    pub block_end: CollapsedMargin,
    pub collapses_through: bool,
}

impl BlockMarginState {
    pub fn from_box(
        style: BlockStyle,
        containing_inline_size: f32,
        child_block_start: CollapsedMargin,
        child_block_end: CollapsedMargin,
        collapse: BlockMarginCollapse,
        collapses_through: bool,
    ) -> Self {
        let margin = style.logical_margin(containing_inline_size);
        let own_start = CollapsedMargin::from_margin(margin.block_start.unwrap_or(0.0));
        let own_end = CollapsedMargin::from_margin(margin.block_end.unwrap_or(0.0));
        Self {
            block_start: if collapse.block_start || collapses_through {
                own_start.collapse_with(child_block_start)
            } else {
                own_start
            },
            block_end: if collapse.block_end || collapses_through {
                own_end.collapse_with(child_block_end)
            } else {
                own_end
            },
            collapses_through,
        }
    }
}

/// Whether this box's start and end margins adjoin its first and last in-flow
/// children's margins.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockMarginCollapse {
    pub block_start: bool,
    pub block_end: bool,
}

impl BlockMarginCollapse {
    pub const NONE: Self = Self {
        block_start: false,
        block_end: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsedInlineSize {
    pub margin_start: f32,
    pub border_box: f32,
    pub margin_end: f32,
}

/// Solve CSS 2.2 section 10.3.3's block-width equation in logical terms.
pub fn solve_in_flow_inline_size(style: BlockStyle, containing_inline_size: f32) -> UsedInlineSize {
    solve_in_flow_inline_size_for_available(style, containing_inline_size, containing_inline_size)
}

/// Solve the block-width equation inside a float-constrained interval.
///
/// Percentages still resolve against the actual containing block. Only the
/// space distributed by `width: auto` and automatic margins is narrowed.
pub fn solve_in_flow_inline_size_for_available(
    style: BlockStyle,
    containing_inline_size: f32,
    available_inline_size: f32,
) -> UsedInlineSize {
    let margins = style.logical_margin(containing_inline_size);
    let padding_border = style.logical_padding_border(containing_inline_size);
    let padding_border_sum = padding_border.inline_start + padding_border.inline_end;
    let preferred = logical_dimension(style.containing_flow, style.size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map(|size| border_box_size(style.box_sizing, size, padding_border_sum));
    let minimum = logical_dimension(style.containing_flow, style.min_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map_or(padding_border_sum, |size| {
            border_box_size(style.box_sizing, size, padding_border_sum)
        });
    let maximum = logical_dimension(style.containing_flow, style.max_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map(|size| border_box_size(style.box_sizing, size, padding_border_sum));

    let start = margins.inline_start.unwrap_or(0.0);
    let end = margins.inline_end.unwrap_or(0.0);
    let mut border_box = preferred
        .unwrap_or_else(|| (available_inline_size - start - end).max(padding_border_sum))
        .max(minimum);
    if let Some(maximum) = maximum {
        border_box = border_box.min(maximum.max(padding_border_sum));
    }

    let remaining = available_inline_size - border_box - start - end;
    let (margin_start, margin_end) =
        match (margins.inline_start.is_none(), margins.inline_end.is_none()) {
            (true, true) if remaining >= 0.0 => (remaining / 2.0, remaining / 2.0),
            (true, true) => (0.0, remaining),
            (true, false) if remaining >= 0.0 => (remaining, end),
            // CSS 2.2 section 10.3.3 first resolves an auto margin to zero
            // when the equation is over-constrained, then ignores inline-end
            // in the containing block's direction.
            (true, false) => (0.0, end + remaining),
            (false, true) => (start, remaining),
            // In an over-constrained equation, direction determines the ignored
            // physical side. In logical coordinates that is always inline-end.
            (false, false) => (start, end + remaining),
        };

    UsedInlineSize {
        margin_start,
        border_box,
        margin_end,
    }
}

/// Resolve the outer inline inputs of a definite-width float.
///
/// CSS 2.2 section 10.3.5 resolves automatic float margins to zero and does
/// not distribute the remaining width as an ordinary in-flow block does.
pub fn solve_float_inline_size(style: BlockStyle, containing_inline_size: f32) -> UsedInlineSize {
    let margins = style.logical_margin(containing_inline_size);
    let padding_border = style.logical_padding_border(containing_inline_size);
    let padding_border_sum = padding_border.inline_start + padding_border.inline_end;
    let preferred = logical_dimension(style.containing_flow, style.size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map(|size| border_box_size(style.box_sizing, size, padding_border_sum))
        .unwrap_or(padding_border_sum);
    let minimum = logical_dimension(style.containing_flow, style.min_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map_or(padding_border_sum, |size| {
            border_box_size(style.box_sizing, size, padding_border_sum)
        });
    let maximum = logical_dimension(style.containing_flow, style.max_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map(|size| border_box_size(style.box_sizing, size, padding_border_sum));
    let border_box = maximum.map_or(preferred.max(minimum), |maximum| {
        preferred.max(minimum).min(maximum.max(padding_border_sum))
    });

    UsedInlineSize {
        margin_start: margins.inline_start.unwrap_or(0.0),
        border_box,
        margin_end: margins.inline_end.unwrap_or(0.0),
    }
}

/// Resolve an auto-width shrink-to-fit box using CSS2's shrink-to-fit equation.
///
/// The intrinsic pair describes the box's content box. Padding and border
/// are added after `min(max(min-content, available), max-content)`, then the
/// box's definite minimum and maximum constraints are applied.
pub fn solve_shrink_to_fit_inline_size(
    style: BlockStyle,
    containing_inline_size: f32,
    intrinsic: IntrinsicSizes,
) -> UsedInlineSize {
    debug_assert!(style.shrink_to_fit);
    let margins = style.logical_margin(containing_inline_size);
    let padding_border = style.logical_padding_border(containing_inline_size);
    let padding_border_sum = padding_border.inline_start + padding_border.inline_end;
    let margin_start = margins.inline_start.unwrap_or(0.0);
    let margin_end = margins.inline_end.unwrap_or(0.0);
    let available_content =
        (containing_inline_size - margin_start - margin_end - padding_border_sum).max(0.0);
    let shrink_to_fit = intrinsic
        .min_content
        .max(available_content)
        .min(intrinsic.max_content);
    let minimum = logical_dimension(style.containing_flow, style.min_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map_or(padding_border_sum, |size| {
            border_box_size(style.box_sizing, size, padding_border_sum)
        });
    let maximum = logical_dimension(style.containing_flow, style.max_size)
        .inline
        .resolve_definite(Some(containing_inline_size))
        .map(|size| border_box_size(style.box_sizing, size, padding_border_sum));
    let tentative = shrink_to_fit + padding_border_sum;
    let maximum_constrained = maximum.map_or(tentative, |maximum| {
        tentative.min(maximum.max(padding_border_sum))
    });
    let border_box = maximum_constrained.max(minimum);

    UsedInlineSize {
        margin_start,
        border_box,
        margin_end,
    }
}

fn border_box_size(sizing: BlockBoxSizing, specified: f32, padding_border: f32) -> f32 {
    match sizing {
        BlockBoxSizing::ContentBox => specified.max(0.0) + padding_border,
        BlockBoxSizing::BorderBox => specified.max(padding_border),
    }
}

fn logical_dimension<T: Copy>(
    axes: FlowAxes,
    physical: BlockDimensions<T>,
) -> LogicalDimensions<T> {
    if axes.is_horizontal() {
        LogicalDimensions {
            inline: physical.width,
            block: physical.height,
        }
    } else {
        LogicalDimensions {
            inline: physical.height,
            block: physical.width,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LogicalDimensions<T> {
    inline: T,
    block: T,
}

/// Dynamic containing-block geometry for one BFC run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockContainingBlock {
    pub flow: FlowAxes,
    pub content_box: PhysicalSize,
}

/// One in-flow child placement relative to the containing content box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockPlacement {
    /// Parent-flow geometry retained until the caller has finalized the
    /// containing block's own block size.
    pub logical_rect: LogicalRect,
    pub rect: PhysicalRect,
    pub margin_inline_start: f32,
    pub margin_inline_end: f32,
}

/// Candidate placement for an in-flow BFC that must avoid active floats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatAvoidingPlacement {
    /// Border-box block start in the containing BFC's logical coordinates.
    pub block_start: f32,
    /// Border-box inline start in the containing BFC's logical coordinates.
    pub inline_start: f32,
    /// Used inline dimensions solved inside the selected float band.
    pub inline_size: UsedInlineSize,
}

/// Inline interval left by floats for a line or atomic formatting context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatAvailableSpace {
    pub inline_start: f32,
    pub inline_size: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FloatExclusion {
    side: FloatSide,
    at_inline_start: bool,
    margin_box: FloatMarginBox,
}

/// The block axis of a float margin box retains its signed used size. A
/// negative used size still locates the float's border box, but cannot exclude
/// line boxes or participate in clearance.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SignedBlockExtent {
    start: f32,
    used_size: f32,
}

impl SignedBlockExtent {
    fn end(self) -> f32 {
        self.start + self.used_size
    }

    fn is_valid_exclusion(self) -> bool {
        self.used_size >= 0.0
    }
}

/// A float's margin box is deliberately not a [`LogicalRect`]: CSS margins
/// can make its used block size negative.
#[derive(Clone, Copy, Debug, PartialEq)]
struct FloatMarginBox {
    inline_start: f32,
    inline_size: f32,
    block: SignedBlockExtent,
}

impl FloatExclusion {
    fn block_start(self) -> f32 {
        self.margin_box.block.start
    }

    fn block_end(self) -> f32 {
        self.margin_box.block.end()
    }

    fn is_valid_exclusion(self) -> bool {
        self.margin_box.block.is_valid_exclusion()
    }

    fn overlaps_block(self, block_start: f32, block_size: f32) -> bool {
        self.is_valid_exclusion()
            && spans_overlap(
                self.block_start(),
                self.margin_box.block.used_size,
                block_start,
                block_size,
            )
    }
}

/// Float exclusions translated into one ordinary descendant block's content
/// coordinate space.
///
/// This state is crate-private because it is an algorithm continuation, not a
/// fragment-tree result. Explicit BFC boundaries never receive or export it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FloatContextState {
    exclusions: Vec<FloatExclusion>,
}

impl FloatContextState {
    pub(crate) fn has_side(&self, side: FloatSide) -> bool {
        self.exclusions.iter().any(|exclusion| {
            exclusion.side == side && exclusion.is_valid_exclusion() && exclusion.block_end() > 0.0
        })
    }
}

/// Immutable float geometry supplied to one inline formatting context.
///
/// Line positions are local to the inline context. Buckram retains the
/// context's block-axis offset into the owning BFC so callers do not have to
/// translate float rectangles or infer their coordinate space.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatLineConstraints {
    flow: FlowAxes,
    containing_inline_size: f32,
    block_offset: f32,
    exclusions: Vec<FloatExclusion>,
}

impl FloatLineConstraints {
    /// Return the inline interval available across this line's block-axis
    /// span. Using the whole span, rather than only its top edge, keeps a tall
    /// line from intersecting a float boundary partway through the line box.
    pub fn available_space(
        &self,
        line_block_start: f32,
        line_block_size: f32,
    ) -> FloatAvailableSpace {
        available_inline_space_for(
            self.containing_inline_size,
            &self.exclusions,
            self.block_offset + line_block_start,
            line_block_size,
        )
    }

    /// Convert the flow-relative interval to the horizontal physical
    /// coordinate consumed by a text shaper. Vertical flow remains outside
    /// the admitted Buckram block lane.
    pub fn horizontal_physical_space(
        &self,
        line_block_start: f32,
        line_block_size: f32,
    ) -> FloatAvailableSpace {
        let logical = self.available_space(line_block_start, line_block_size);
        let inline_start = match self.flow.inline_start() {
            crate::PhysicalSide::Left => logical.inline_start,
            crate::PhysicalSide::Right => {
                self.containing_inline_size - logical.inline_start - logical.inline_size
            },
            crate::PhysicalSide::Top | crate::PhysicalSide::Bottom => logical.inline_start,
        };
        FloatAvailableSpace {
            inline_start: inline_start.max(0.0),
            inline_size: logical.inline_size,
        }
    }

    /// Find the next lower float boundary that gives this line more inline
    /// room. CSS moves a line down when the interval beside a float cannot
    /// contain any of its content.
    pub fn next_wider_block_start(
        &self,
        line_block_start: f32,
        line_block_size: f32,
        current_inline_size: f32,
    ) -> Option<f32> {
        let absolute_start = self.block_offset + line_block_start;
        let mut candidates = self
            .exclusions
            .iter()
            .filter(|exclusion| exclusion.is_valid_exclusion())
            .map(|exclusion| exclusion.block_end())
            .filter(|block_end| *block_end > absolute_start)
            .collect::<Vec<_>>();
        candidates.sort_by(f32::total_cmp);
        candidates.dedup_by(|left, right| (*left - *right).abs() <= f32::EPSILON);
        candidates.into_iter().find_map(|candidate| {
            let available = available_inline_space_for(
                self.containing_inline_size,
                &self.exclusions,
                candidate,
                line_block_size,
            );
            (available.inline_size > current_inline_size + 0.01)
                .then_some((candidate - self.block_offset).max(line_block_start))
        })
    }
}

/// Normal-flow cursor for a block formatting context.
pub struct BlockFormattingContext {
    containing_block: BlockContainingBlock,
    block_cursor: f32,
    active_margin: CollapsedMargin,
    first_margin: CollapsedMargin,
    has_child: bool,
    adjoining_parent_start: bool,
    all_children_collapse_through: bool,
    collapse_parent_start: bool,
    float_exclusions: Vec<FloatExclusion>,
    inherited_float_count: usize,
    latest_float_block_start: f32,
    active_margin_has_clearance: bool,
}

impl BlockFormattingContext {
    pub fn new(containing_block: BlockContainingBlock) -> Self {
        Self::with_margin_collapse(containing_block, false)
    }

    pub fn with_margin_collapse(
        containing_block: BlockContainingBlock,
        collapse_parent_start: bool,
    ) -> Self {
        Self::with_float_state(
            containing_block,
            collapse_parent_start,
            FloatContextState::default(),
        )
    }

    pub(crate) fn with_float_state(
        containing_block: BlockContainingBlock,
        collapse_parent_start: bool,
        float_state: FloatContextState,
    ) -> Self {
        let inherited_float_count = float_state.exclusions.len();
        let latest_float_block_start = float_state
            .exclusions
            .iter()
            .filter(|exclusion| exclusion.is_valid_exclusion())
            .map(|exclusion| exclusion.block_start())
            .fold(0.0, f32::max);
        Self {
            containing_block,
            block_cursor: 0.0,
            active_margin: CollapsedMargin::ZERO,
            first_margin: CollapsedMargin::ZERO,
            has_child: false,
            adjoining_parent_start: true,
            all_children_collapse_through: true,
            collapse_parent_start,
            float_exclusions: float_state.exclusions,
            inherited_float_count,
            latest_float_block_start,
            active_margin_has_clearance: false,
        }
    }

    /// Translate all active exclusions into a descendant content box.
    pub(crate) fn float_state_for_descendant(
        &self,
        inline_offset: f32,
        block_offset: f32,
    ) -> FloatContextState {
        FloatContextState {
            exclusions: self
                .float_exclusions
                .iter()
                .cloned()
                .map(|mut exclusion| {
                    exclusion.margin_box.inline_start -= inline_offset;
                    exclusion.margin_box.block.start -= block_offset;
                    exclusion
                })
                .collect(),
        }
    }

    /// Return only floats created by this ordinary block, excluding inherited
    /// ancestor exclusions.
    pub(crate) fn exported_float_state(&self) -> FloatContextState {
        FloatContextState {
            exclusions: self.float_exclusions[self.inherited_float_count..].to_vec(),
        }
    }

    /// Merge a descendant's newly created floats back into this BFC.
    pub(crate) fn import_descendant_float_state(
        &mut self,
        state: FloatContextState,
        inline_offset: f32,
        block_offset: f32,
    ) {
        for mut exclusion in state.exclusions {
            exclusion.margin_box.inline_start += inline_offset;
            exclusion.margin_box.block.start += block_offset;
            if exclusion.is_valid_exclusion() {
                self.latest_float_block_start =
                    self.latest_float_block_start.max(exclusion.block_start());
            }
            self.float_exclusions.push(exclusion);
        }
    }

    /// Place a definite-size float using CSS's high-as-possible, then
    /// side-most placement rules. The stored exclusion is the float margin
    /// box; the returned rectangle is its border box.
    pub fn place_float(
        &mut self,
        style: BlockStyle,
        border_box_size: PhysicalSize,
    ) -> BlockPlacement {
        debug_assert_ne!(style.float, FloatSide::None);
        let containing_size = self
            .containing_block
            .flow
            .logical_size(self.containing_block.content_box);
        let margin = style.logical_margin(containing_size.inline);
        let margin_inline_start = margin.inline_start.unwrap_or(0.0);
        let margin_inline_end = margin.inline_end.unwrap_or(0.0);
        let margin_block_start = margin.block_start.unwrap_or(0.0);
        let margin_block_end = margin.block_end.unwrap_or(0.0);
        let border_size = self.containing_block.flow.logical_size(border_box_size);
        let outer_inline_size = margin_inline_start + border_size.inline + margin_inline_end;
        let outer_block_size = margin_block_start + border_size.block + margin_block_end;
        let hypothetical_block_start =
            self.block_cursor + self.active_margin.resolve() + margin_block_start;
        let clear_block_start = self.clearance_block_end(style.clear) + margin_block_start;
        let mut margin_box_block_start = (hypothetical_block_start - margin_block_start)
            .max(clear_block_start - margin_block_start)
            .max(self.latest_float_block_start);
        let at_inline_start = self.float_is_at_inline_start(style.float);

        let margin_box_inline_start = loop {
            let available = self.available_inline_space(margin_box_block_start, outer_block_size);
            if outer_inline_size <= available.inline_size
                || !self.float_exclusions_overlap(margin_box_block_start, outer_block_size)
            {
                break if at_inline_start {
                    available.inline_start
                } else {
                    available.inline_start + available.inline_size - outer_inline_size
                };
            }
            let Some(next_block_start) =
                self.next_float_block_end(margin_box_block_start, outer_block_size)
            else {
                break if at_inline_start {
                    available.inline_start
                } else {
                    available.inline_start + available.inline_size - outer_inline_size
                };
            };
            margin_box_block_start = next_block_start;
        };
        let margin_box = FloatMarginBox {
            inline_start: margin_box_inline_start,
            inline_size: outer_inline_size,
            block: SignedBlockExtent {
                start: margin_box_block_start,
                used_size: outer_block_size,
            },
        };
        self.float_exclusions.push(FloatExclusion {
            side: style.float,
            at_inline_start,
            margin_box,
        });
        if margin_box.block.is_valid_exclusion() {
            self.latest_float_block_start =
                self.latest_float_block_start.max(margin_box.block.start);
        }
        let border_box = LogicalRect {
            inline_start: margin_box.inline_start + margin_inline_start,
            block_start: margin_box.block.start + margin_block_start,
            inline_size: border_size.inline,
            block_size: border_size.block,
        };

        BlockPlacement {
            logical_rect: border_box,
            rect: self
                .containing_block
                .flow
                .physical_rect(border_box, self.containing_block.content_box),
            margin_inline_start,
            margin_inline_end,
        }
    }

    /// Return the flow-relative interval not intersected by a float margin
    /// box across the supplied block-axis span.
    pub fn available_inline_space(&self, block_start: f32, block_size: f32) -> FloatAvailableSpace {
        let containing_inline = self
            .containing_block
            .flow
            .logical_size(self.containing_block.content_box)
            .inline;
        available_inline_space_for(
            containing_inline,
            &self.float_exclusions,
            block_start,
            block_size,
        )
    }

    /// Choose the highest float band in which an independent BFC's border
    /// box fits without intersecting a float margin box.
    ///
    /// `width: auto` is resolved against the selected band. A definite or
    /// minimum width that cannot fit moves to the next overlapping float
    /// boundary. The caller may repeat this query with the measured block
    /// size when layout at the candidate width changes the box's height.
    pub fn float_avoiding_placement(
        &self,
        style: BlockStyle,
        margin_state: BlockMarginState,
        border_box_block_size: f32,
    ) -> FloatAvoidingPlacement {
        debug_assert!(style.establishes_bfc);
        debug_assert_eq!(style.float, FloatSide::None);
        let containing_inline = self
            .containing_block
            .flow
            .logical_size(self.containing_block.content_box)
            .inline;
        let mut block_start = self.hypothetical_in_flow_block_start(style, margin_state);

        loop {
            if !self.float_exclusions_overlap(block_start, border_box_block_size) {
                let inline_size = solve_in_flow_inline_size(style, containing_inline);
                return FloatAvoidingPlacement {
                    block_start,
                    inline_start: inline_size.margin_start,
                    inline_size,
                };
            }

            let available = self.available_inline_space(block_start, border_box_block_size);
            let inline_size = solve_in_flow_inline_size_for_available(
                style,
                containing_inline,
                available.inline_size,
            );
            let inline_start = available.inline_start + inline_size.margin_start;
            let available_end = available.inline_start + available.inline_size;
            let border_end = inline_start + inline_size.border_box;
            if inline_start + 0.01 >= available.inline_start && border_end <= available_end + 0.01 {
                return FloatAvoidingPlacement {
                    block_start,
                    inline_start,
                    inline_size,
                };
            }

            let Some(next_block_start) =
                self.next_float_block_end(block_start, border_box_block_size)
            else {
                let inline_size = solve_in_flow_inline_size(style, containing_inline);
                return FloatAvoidingPlacement {
                    block_start,
                    inline_start: inline_size.margin_start,
                    inline_size,
                };
            };
            block_start = next_block_start;
        }
    }

    pub fn float_exclusion_count(&self) -> usize {
        self.float_exclusions.len()
    }

    /// Snapshot active floats for line breaking whose local block-axis origin
    /// starts at `block_offset` in this BFC.
    pub fn float_line_constraints(&self, block_offset: f32) -> Option<FloatLineConstraints> {
        self.float_exclusions
            .iter()
            .any(|exclusion| exclusion.is_valid_exclusion())
            .then(|| FloatLineConstraints {
                flow: self.containing_block.flow,
                containing_inline_size: self
                    .containing_block
                    .flow
                    .logical_size(self.containing_block.content_box)
                    .inline,
                block_offset,
                exclusions: self.float_exclusions.clone(),
            })
    }

    pub(crate) fn hypothetical_in_flow_block_start(
        &self,
        style: BlockStyle,
        margin_state: BlockMarginState,
    ) -> f32 {
        self.normal_in_flow_block_start(margin_state)
            .max(self.clearance_block_end(style.clear))
    }

    fn normal_in_flow_block_start(&self, margin_state: BlockMarginState) -> f32 {
        let adjoining = self.active_margin.collapse_with(margin_state.block_start);
        let participates_in_parent_start =
            self.adjoining_parent_start && self.collapse_parent_start;
        if participates_in_parent_start {
            self.block_cursor
        } else {
            self.block_cursor + adjoining.resolve()
        }
    }

    pub fn place_in_flow(
        &mut self,
        style: BlockStyle,
        border_box_size: PhysicalSize,
    ) -> BlockPlacement {
        let containing_inline = self
            .containing_block
            .flow
            .logical_size(self.containing_block.content_box)
            .inline;
        let margin = style.logical_margin(containing_inline);
        self.place_in_flow_with_margins(
            style,
            border_box_size,
            BlockMarginState {
                block_start: CollapsedMargin::from_margin(margin.block_start.unwrap_or(0.0)),
                block_end: CollapsedMargin::from_margin(margin.block_end.unwrap_or(0.0)),
                collapses_through: false,
            },
        )
    }

    pub fn place_in_flow_with_margins(
        &mut self,
        style: BlockStyle,
        border_box_size: PhysicalSize,
        margin_state: BlockMarginState,
    ) -> BlockPlacement {
        let containing_inline = self
            .containing_block
            .flow
            .logical_size(self.containing_block.content_box)
            .inline;
        let used_inline = solve_in_flow_inline_size(style, containing_inline);
        self.place_in_flow_at(
            style,
            border_box_size,
            margin_state,
            used_inline.margin_start,
            used_inline,
            None,
        )
    }

    /// Commit a previously measured float-avoiding BFC placement to normal
    /// flow. The candidate is produced by [`Self::float_avoiding_placement`].
    pub fn place_float_avoiding_in_flow(
        &mut self,
        style: BlockStyle,
        border_box_size: PhysicalSize,
        margin_state: BlockMarginState,
        placement: FloatAvoidingPlacement,
    ) -> BlockPlacement {
        self.place_in_flow_at(
            style,
            border_box_size,
            margin_state,
            placement.inline_start,
            placement.inline_size,
            Some(placement.block_start),
        )
    }

    fn place_in_flow_at(
        &mut self,
        style: BlockStyle,
        border_box_size: PhysicalSize,
        margin_state: BlockMarginState,
        inline_start: f32,
        used_inline: UsedInlineSize,
        minimum_block_start: Option<f32>,
    ) -> BlockPlacement {
        let adjoining = self.active_margin.collapse_with(margin_state.block_start);
        let normal_block_start = self.normal_in_flow_block_start(margin_state);
        let clear_block_end = self.clearance_block_end(style.clear);
        let block_start = normal_block_start
            .max(clear_block_end)
            .max(minimum_block_start.unwrap_or(normal_block_start));
        let has_clearance = block_start > normal_block_start;
        let child_size = self.containing_block.flow.logical_size(border_box_size);
        let logical = LogicalRect {
            inline_start,
            block_start,
            inline_size: child_size.inline,
            block_size: child_size.block,
        };

        if has_clearance {
            if self.adjoining_parent_start {
                self.first_margin = CollapsedMargin::ZERO;
            }
            self.adjoining_parent_start = false;
        } else if self.adjoining_parent_start {
            self.first_margin = adjoining;
            if margin_state.collapses_through {
                self.first_margin = self.first_margin.collapse_with(margin_state.block_end);
            } else {
                self.adjoining_parent_start = false;
            }
        }
        if margin_state.collapses_through {
            if has_clearance {
                // Clearance separates this empty box's adjoining margins
                // from the preceding chain. Its own start and end margins
                // may still collapse through into following siblings, but
                // that resulting chain may not collapse with the parent's
                // block-end margin.
                self.block_cursor = logical.block_start;
                self.active_margin = margin_state
                    .block_start
                    .collapse_with(margin_state.block_end);
                self.active_margin_has_clearance = true;
                self.all_children_collapse_through = false;
            } else {
                self.active_margin = adjoining.collapse_with(margin_state.block_end);
            }
        } else {
            self.block_cursor = logical.block_start + logical.block_size;
            self.active_margin = margin_state.block_end;
            self.active_margin_has_clearance = false;
            self.all_children_collapse_through = false;
        }
        self.has_child = true;

        BlockPlacement {
            logical_rect: logical,
            rect: self
                .containing_block
                .flow
                .physical_rect(logical, self.containing_block.content_box),
            margin_inline_start: used_inline.margin_start,
            margin_inline_end: used_inline.margin_end,
        }
    }

    pub fn all_children_collapse_through(&self) -> bool {
        self.all_children_collapse_through
    }

    pub fn first_child_margin(&self) -> CollapsedMargin {
        if self.has_child {
            self.first_margin
        } else {
            CollapsedMargin::ZERO
        }
    }

    pub fn last_child_margin(&self) -> CollapsedMargin {
        if self.has_child {
            self.active_margin
        } else {
            CollapsedMargin::ZERO
        }
    }

    /// Whether the active trailing margin chain may adjoin the parent's
    /// block-end margin.
    pub(crate) fn active_margin_may_collapse_with_parent_end(&self) -> bool {
        !self.active_margin_has_clearance
    }

    pub fn used_block_size_with_margin_collapse(&self, collapse_parent_end: bool) -> f32 {
        self.block_cursor
            + if self.has_child && !collapse_parent_end {
                self.active_margin.resolve()
            } else {
                0.0
            }
    }

    pub fn used_block_size(&self) -> f32 {
        self.used_block_size_with_margin_collapse(false)
    }

    /// Auto block size of a box that establishes a BFC includes the bottom
    /// margin edge of its in-flow floats.
    pub fn used_block_size_containing_floats(&self, collapse_parent_end: bool) -> f32 {
        self.used_block_size_with_margin_collapse(collapse_parent_end)
            .max(self.lowest_float_block_end())
    }

    fn float_is_at_inline_start(&self, side: FloatSide) -> bool {
        matches!(
            (side, self.containing_block.flow.inline_start()),
            (FloatSide::Left, crate::PhysicalSide::Left)
                | (FloatSide::Right, crate::PhysicalSide::Right)
        )
    }

    fn clearance_block_end(&self, clear: ClearSide) -> f32 {
        self.float_exclusions
            .iter()
            .filter(|exclusion| match clear {
                ClearSide::None => false,
                ClearSide::Left => exclusion.side == FloatSide::Left,
                ClearSide::Right => exclusion.side == FloatSide::Right,
                ClearSide::Both => true,
            })
            .filter(|exclusion| exclusion.is_valid_exclusion())
            .map(|exclusion| exclusion.block_end())
            .fold(0.0, f32::max)
    }

    fn lowest_float_block_end(&self) -> f32 {
        self.float_exclusions
            .iter()
            .filter(|exclusion| exclusion.is_valid_exclusion())
            .map(|exclusion| exclusion.block_end())
            .fold(0.0, f32::max)
    }

    fn float_exclusions_overlap(&self, block_start: f32, block_size: f32) -> bool {
        self.float_exclusions
            .iter()
            .any(|exclusion| exclusion.overlaps_block(block_start, block_size))
    }

    fn next_float_block_end(&self, block_start: f32, block_size: f32) -> Option<f32> {
        self.float_exclusions
            .iter()
            .filter(|exclusion| exclusion.overlaps_block(block_start, block_size))
            .map(|exclusion| exclusion.block_end())
            .filter(|block_end| *block_end > block_start)
            .min_by(f32::total_cmp)
    }
}

fn available_inline_space_for(
    containing_inline: f32,
    exclusions: &[FloatExclusion],
    block_start: f32,
    block_size: f32,
) -> FloatAvailableSpace {
    let mut inline_start: f32 = 0.0;
    let mut inline_end = containing_inline;
    for exclusion in exclusions
        .iter()
        .filter(|exclusion| exclusion.overlaps_block(block_start, block_size))
    {
        if exclusion.at_inline_start {
            inline_start = inline_start
                .max(exclusion.margin_box.inline_start + exclusion.margin_box.inline_size);
        } else {
            inline_end = inline_end.min(exclusion.margin_box.inline_start);
        }
    }
    FloatAvailableSpace {
        inline_start,
        inline_size: (inline_end - inline_start).max(0.0),
    }
}

fn spans_overlap(first_start: f32, first_size: f32, second_start: f32, second_size: f32) -> bool {
    if first_size < 0.0 || second_size < 0.0 {
        return false;
    }
    let first_end = first_start + first_size;
    let second_end = second_start + second_size;
    if first_size == 0.0 || second_size == 0.0 {
        first_start <= second_start && second_start < first_end
            || second_start <= first_start && first_start < second_end
    } else {
        first_start < second_end && second_start < first_end
    }
}

#[cfg(test)]
mod tests {
    use crate::{Direction, WritingMode};

    use super::*;

    fn fixed_width(width: f32) -> BlockStyle {
        BlockStyle {
            size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(width)),
                BlockSizeValue::Auto,
            ),
            ..BlockStyle::default()
        }
    }

    #[test]
    fn block_width_equation_centres_two_auto_margins() {
        let mut style = fixed_width(100.0);
        style.margin.left = FlowLengthAuto::Auto;
        style.margin.right = FlowLengthAuto::Auto;

        assert_eq!(
            solve_in_flow_inline_size(style, 300.0),
            UsedInlineSize {
                margin_start: 100.0,
                border_box: 100.0,
                margin_end: 100.0,
            }
        );
    }

    #[test]
    fn one_auto_inline_margin_uses_the_logical_start_in_both_directions() {
        for (flow, auto_on_left) in [
            (FlowAxes::HORIZONTAL_LTR, true),
            (
                FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl),
                false,
            ),
        ] {
            let mut style = fixed_width(100.0);
            style.flow = flow;
            style.containing_flow = flow;
            if auto_on_left {
                style.margin.left = FlowLengthAuto::Auto;
                style.margin.right = FlowLengthAuto::Value(FlowLength::px(20.0));
            } else {
                style.margin.left = FlowLengthAuto::Value(FlowLength::px(20.0));
                style.margin.right = FlowLengthAuto::Auto;
            }

            assert_eq!(
                solve_in_flow_inline_size(style, 300.0),
                UsedInlineSize {
                    margin_start: 180.0,
                    border_box: 100.0,
                    margin_end: 20.0,
                },
                "flow={flow:?}"
            );
        }
    }

    #[test]
    fn overconstrained_auto_inline_start_resolves_to_zero() {
        for (flow, auto_on_left) in [
            (FlowAxes::HORIZONTAL_LTR, true),
            (
                FlowAxes::new(WritingMode::HorizontalTb, Direction::Rtl),
                false,
            ),
        ] {
            let mut style = fixed_width(200.0);
            style.flow = flow;
            style.containing_flow = flow;
            if auto_on_left {
                style.margin.left = FlowLengthAuto::Auto;
                style.margin.right = FlowLengthAuto::Value(FlowLength::px(25.0));
            } else {
                style.margin.left = FlowLengthAuto::Value(FlowLength::px(25.0));
                style.margin.right = FlowLengthAuto::Auto;
            }

            assert_eq!(
                solve_in_flow_inline_size(style, 100.0),
                UsedInlineSize {
                    margin_start: 0.0,
                    border_box: 200.0,
                    margin_end: -100.0,
                },
                "flow={flow:?}"
            );
        }
    }

    #[test]
    fn adjoining_positive_and_negative_margins_collapse_separately() {
        assert_eq!(
            CollapsedMargin::from_margin(20.0)
                .collapse(12.0)
                .collapse(-7.0)
                .collapse(-4.0)
                .resolve(),
            13.0
        );
    }

    fn margin_state(start: f32, end: f32, collapses_through: bool) -> BlockMarginState {
        BlockMarginState {
            block_start: CollapsedMargin::from_margin(start),
            block_end: CollapsedMargin::from_margin(end),
            collapses_through,
        }
    }

    #[test]
    fn parent_start_and_end_edges_keep_child_margins_outside_used_height() {
        let containing = BlockContainingBlock {
            flow: FlowAxes::HORIZONTAL_LTR,
            content_box: PhysicalSize {
                width: 200.0,
                height: 0.0,
            },
        };
        let child = fixed_width(200.0);
        let mut context = BlockFormattingContext::with_margin_collapse(containing, true);
        let placement = context.place_in_flow_with_margins(
            child,
            PhysicalSize {
                width: 200.0,
                height: 20.0,
            },
            margin_state(30.0, 40.0, false),
        );

        assert_eq!(placement.rect.y, 0.0);
        assert_eq!(context.first_child_margin().resolve(), 30.0);
        assert_eq!(context.last_child_margin().resolve(), 40.0);
        assert_eq!(context.used_block_size_with_margin_collapse(true), 20.0);
    }

    #[test]
    fn empty_block_carries_one_margin_chain_between_siblings() {
        let containing = BlockContainingBlock {
            flow: FlowAxes::HORIZONTAL_LTR,
            content_box: PhysicalSize {
                width: 200.0,
                height: 0.0,
            },
        };
        let child = fixed_width(200.0);
        let mut context = BlockFormattingContext::new(containing);
        let first = context.place_in_flow_with_margins(
            child,
            PhysicalSize {
                width: 200.0,
                height: 10.0,
            },
            margin_state(0.0, 20.0, false),
        );
        let empty = context.place_in_flow_with_margins(
            child,
            PhysicalSize {
                width: 200.0,
                height: 0.0,
            },
            margin_state(-7.0, 12.0, true),
        );
        let last = context.place_in_flow_with_margins(
            child,
            PhysicalSize {
                width: 200.0,
                height: 10.0,
            },
            margin_state(-15.0, 0.0, false),
        );

        assert_eq!(first.rect.y, 0.0);
        assert_eq!(empty.rect.y, 23.0);
        assert_eq!(last.rect.y, 15.0);
        assert_eq!(context.used_block_size(), 25.0);
    }

    #[test]
    fn empty_auto_block_exposes_collapsing_start_and_end_sets() {
        let mut style = BlockStyle::default();
        style.margin.top = FlowLengthAuto::Value(FlowLength::px(20.0));
        style.margin.bottom = FlowLengthAuto::Value(FlowLength::px(30.0));
        let collapse = style.child_margin_collapse(200.0, None, false, true);
        let through = style.can_collapse_through(200.0, None, false, false, true);
        let margins = BlockMarginState::from_box(
            style,
            200.0,
            CollapsedMargin::from_margin(-8.0),
            CollapsedMargin::from_margin(12.0),
            collapse,
            through,
        );

        assert!(through);
        assert_eq!(margins.block_start.resolve(), 12.0);
        assert_eq!(margins.block_end.resolve(), 30.0);
        assert!(margins.collapses_through);
    }

    #[test]
    fn nonzero_minimum_only_blocks_end_collapse_when_the_chain_reaches_start() {
        let style = BlockStyle {
            min_size: BlockDimensions::new(
                BlockSizeValue::Auto,
                BlockSizeValue::Length(FlowLength::px(10.0)),
            ),
            ..BlockStyle::default()
        };

        assert!(
            style
                .child_margin_collapse(200.0, None, false, false)
                .block_end
        );
        assert!(
            !style
                .child_margin_collapse(200.0, None, false, true)
                .block_end
        );
    }

    #[test]
    fn vertical_rl_stacks_children_from_the_right_edge() {
        let flow = FlowAxes::new(WritingMode::VerticalRl, Direction::Ltr);
        let containing = BlockContainingBlock {
            flow,
            content_box: PhysicalSize {
                width: 300.0,
                height: 200.0,
            },
        };
        let mut context = BlockFormattingContext::new(containing);
        let mut child = fixed_width(40.0);
        child.containing_flow = flow;
        child.size = BlockDimensions::new(
            BlockSizeValue::Length(FlowLength::px(40.0)),
            BlockSizeValue::Length(FlowLength::px(200.0)),
        );

        let first = context.place_in_flow(
            child,
            PhysicalSize {
                width: 40.0,
                height: 200.0,
            },
        );
        let second = context.place_in_flow(
            child,
            PhysicalSize {
                width: 40.0,
                height: 200.0,
            },
        );

        assert_eq!(first.rect.x, 260.0);
        assert_eq!(second.rect.x, 220.0);
        assert_eq!(first.rect.y, 0.0);
        assert_eq!(context.used_block_size(), 80.0);
    }

    fn fixed_float(side: FloatSide, width: f32) -> BlockStyle {
        BlockStyle {
            size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(width)),
                BlockSizeValue::Auto,
            ),
            float: side,
            establishes_bfc: true,
            ..BlockStyle::default()
        }
    }

    fn horizontal_context(width: f32) -> BlockFormattingContext {
        BlockFormattingContext::new(BlockContainingBlock {
            flow: FlowAxes::HORIZONTAL_LTR,
            content_box: PhysicalSize { width, height: 0.0 },
        })
    }

    #[test]
    fn definite_float_margins_resolve_auto_to_zero() {
        let mut style = fixed_float(FloatSide::Left, 80.0);
        style.margin.left = FlowLengthAuto::Auto;
        style.margin.right = FlowLengthAuto::Auto;

        assert_eq!(
            solve_float_inline_size(style, 200.0),
            UsedInlineSize {
                margin_start: 0.0,
                border_box: 80.0,
                margin_end: 0.0,
            }
        );
    }

    #[test]
    fn auto_shrink_to_fit_width_clamps_available_space_between_intrinsic_sizes() {
        let style = BlockStyle {
            padding: PhysicalSides {
                top: FlowLength::ZERO,
                right: FlowLength::px(5.0),
                bottom: FlowLength::ZERO,
                left: FlowLength::px(5.0),
            },
            float: FloatSide::Left,
            establishes_bfc: true,
            shrink_to_fit: true,
            ..BlockStyle::default()
        };
        let intrinsic = IntrinsicSizes::new(40.0, 120.0).expect("valid intrinsic pair");

        assert_eq!(
            solve_shrink_to_fit_inline_size(style, 100.0, intrinsic).border_box,
            100.0
        );
        assert_eq!(
            solve_shrink_to_fit_inline_size(style, 200.0, intrinsic).border_box,
            130.0
        );
        assert_eq!(
            solve_shrink_to_fit_inline_size(style, 30.0, intrinsic).border_box,
            50.0
        );

        let conflicting_constraints = BlockStyle {
            min_size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(90.0)),
                BlockSizeValue::Auto,
            ),
            max_size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(60.0)),
                BlockSizeValue::None,
            ),
            ..style
        };
        assert_eq!(
            solve_shrink_to_fit_inline_size(conflicting_constraints, 200.0, intrinsic).border_box,
            100.0
        );
    }

    #[test]
    fn floats_use_the_highest_side_most_available_interval() {
        let mut context = horizontal_context(200.0);
        let left = context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        let right = context.place_float(
            fixed_float(FloatSide::Right, 70.0),
            PhysicalSize {
                width: 70.0,
                height: 30.0,
            },
        );
        let lowered = context.place_float(
            fixed_float(FloatSide::Left, 100.0),
            PhysicalSize {
                width: 100.0,
                height: 20.0,
            },
        );

        assert_eq!((left.rect.x, left.rect.y), (0.0, 0.0));
        assert_eq!((right.rect.x, right.rect.y), (130.0, 0.0));
        assert_eq!((lowered.rect.x, lowered.rect.y), (80.0, 30.0));
        assert_eq!(
            context.available_inline_space(10.0, 1.0),
            FloatAvailableSpace {
                inline_start: 80.0,
                inline_size: 50.0,
            }
        );
    }

    #[test]
    fn ordinary_descendants_translate_and_return_float_exclusions() {
        let mut parent = horizontal_context(200.0);
        parent.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );

        let inherited = parent.float_state_for_descendant(20.0, 10.0);
        let mut descendant = BlockFormattingContext::with_float_state(
            BlockContainingBlock {
                flow: FlowAxes::HORIZONTAL_LTR,
                content_box: PhysicalSize {
                    width: 160.0,
                    height: 0.0,
                },
            },
            false,
            inherited,
        );
        assert_eq!(
            descendant.available_inline_space(0.0, 10.0),
            FloatAvailableSpace {
                inline_start: 60.0,
                inline_size: 100.0,
            }
        );

        let nested = descendant.place_float(
            fixed_float(FloatSide::Right, 50.0),
            PhysicalSize {
                width: 50.0,
                height: 20.0,
            },
        );
        assert_eq!((nested.rect.x, nested.rect.y), (110.0, 0.0));

        parent.import_descendant_float_state(descendant.exported_float_state(), 20.0, 10.0);
        assert_eq!(parent.float_exclusion_count(), 2);
        assert_eq!(
            parent.available_inline_space(15.0, 1.0),
            FloatAvailableSpace {
                inline_start: 80.0,
                inline_size: 50.0,
            }
        );
    }

    #[test]
    fn translated_float_exclusions_remain_active_above_a_descendant_origin() {
        let mut parent = horizontal_context(200.0);
        parent.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );

        let mut descendant = BlockFormattingContext::with_float_state(
            BlockContainingBlock {
                flow: FlowAxes::HORIZONTAL_LTR,
                content_box: PhysicalSize {
                    width: 180.0,
                    height: 0.0,
                },
            },
            false,
            parent.float_state_for_descendant(20.0, 30.0),
        );

        assert_eq!(
            descendant.available_inline_space(0.0, 5.0),
            FloatAvailableSpace {
                inline_start: 60.0,
                inline_size: 120.0,
            }
        );
        let clear = BlockStyle {
            clear: ClearSide::Left,
            ..fixed_width(180.0)
        };
        let placed = descendant.place_in_flow(
            clear,
            PhysicalSize {
                width: 180.0,
                height: 10.0,
            },
        );
        assert_eq!(placed.rect.y, 10.0);
    }

    #[test]
    fn negative_float_margin_box_sizes_do_not_create_exclusions_on_either_side() {
        for (side, clear) in [
            (FloatSide::Left, ClearSide::Left),
            (FloatSide::Right, ClearSide::Right),
        ] {
            let mut context = horizontal_context(200.0);
            let mut float = fixed_float(side, 80.0);
            float.margin.top = FlowLengthAuto::Value(FlowLength::px(-30.0));
            float.margin.bottom = FlowLengthAuto::Value(FlowLength::px(-20.0));
            let placement = context.place_float(
                float,
                PhysicalSize {
                    width: 80.0,
                    height: 40.0,
                },
            );

            assert_eq!(placement.rect.y, -30.0, "side={side:?}");
            assert_eq!(
                context.available_inline_space(0.0, 1.0),
                FloatAvailableSpace {
                    inline_start: 0.0,
                    inline_size: 200.0,
                },
                "side={side:?}"
            );
            assert!(
                context.float_line_constraints(0.0).is_none(),
                "side={side:?}"
            );
            let in_flow = context.place_in_flow(
                BlockStyle {
                    clear,
                    ..fixed_width(200.0)
                },
                PhysicalSize {
                    width: 200.0,
                    height: 10.0,
                },
            );
            assert_eq!(in_flow.rect.y, 0.0, "side={side:?}");
            assert_eq!(context.used_block_size_containing_floats(false), 10.0);
        }
    }

    #[test]
    fn floats_do_not_advance_normal_flow_but_bfc_height_contains_them() {
        let mut context = horizontal_context(200.0);
        context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        let in_flow = context.place_in_flow(
            fixed_width(200.0),
            PhysicalSize {
                width: 200.0,
                height: 10.0,
            },
        );

        assert_eq!(in_flow.rect.y, 0.0);
        assert_eq!(context.used_block_size(), 10.0);
        assert_eq!(context.used_block_size_containing_floats(false), 40.0);
    }

    #[test]
    fn independent_bfc_narrows_beside_a_float_or_moves_below_when_it_cannot_fit() {
        let margin_state = BlockMarginState {
            block_start: CollapsedMargin::ZERO,
            block_end: CollapsedMargin::ZERO,
            collapses_through: false,
        };

        let mut adjacent_context = horizontal_context(200.0);
        adjacent_context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        let auto_bfc = BlockStyle {
            establishes_bfc: true,
            ..BlockStyle::default()
        };
        let adjacent = adjacent_context.float_avoiding_placement(auto_bfc, margin_state, 20.0);
        assert_eq!(
            adjacent,
            FloatAvoidingPlacement {
                block_start: 0.0,
                inline_start: 80.0,
                inline_size: UsedInlineSize {
                    margin_start: 0.0,
                    border_box: 120.0,
                    margin_end: 0.0,
                },
            }
        );
        let committed = adjacent_context.place_float_avoiding_in_flow(
            auto_bfc,
            PhysicalSize {
                width: 120.0,
                height: 20.0,
            },
            margin_state,
            adjacent,
        );
        assert_eq!((committed.rect.x, committed.rect.y), (80.0, 0.0));

        let mut lowered_context = horizontal_context(200.0);
        lowered_context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        let definite_bfc = BlockStyle {
            size: BlockDimensions::new(
                BlockSizeValue::Length(FlowLength::px(150.0)),
                BlockSizeValue::Auto,
            ),
            establishes_bfc: true,
            ..BlockStyle::default()
        };
        let lowered = lowered_context.float_avoiding_placement(definite_bfc, margin_state, 20.0);
        assert_eq!(lowered.block_start, 40.0);
        assert_eq!(lowered.inline_start, 0.0);
        assert_eq!(lowered.inline_size.border_box, 150.0);
    }

    #[test]
    fn bfc_margin_can_force_its_border_box_below_a_float() {
        let margin_state = BlockMarginState {
            block_start: CollapsedMargin::ZERO,
            block_end: CollapsedMargin::ZERO,
            collapses_through: false,
        };
        let mut context = horizontal_context(100.0);
        context.place_float(
            fixed_float(FloatSide::Right, 50.0),
            PhysicalSize {
                width: 50.0,
                height: 40.0,
            },
        );
        let mut bfc = BlockStyle {
            establishes_bfc: true,
            ..BlockStyle::default()
        };
        bfc.margin.left = FlowLengthAuto::Value(FlowLength::px(51.0));

        let placement = context.float_avoiding_placement(bfc, margin_state, 60.0);

        assert_eq!(
            placement,
            FloatAvoidingPlacement {
                block_start: 40.0,
                inline_start: 51.0,
                inline_size: UsedInlineSize {
                    margin_start: 51.0,
                    border_box: 49.0,
                    margin_end: 0.0,
                },
            }
        );
    }

    #[test]
    fn float_line_constraints_follow_each_line_span_and_reclaim_the_column() {
        let mut context = horizontal_context(200.0);
        context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        context.place_float(
            fixed_float(FloatSide::Right, 60.0),
            PhysicalSize {
                width: 60.0,
                height: 20.0,
            },
        );
        let constraints = context.float_line_constraints(0.0).expect("floats");

        assert_eq!(
            constraints.available_space(0.0, 18.0),
            FloatAvailableSpace {
                inline_start: 80.0,
                inline_size: 60.0,
            }
        );
        assert_eq!(
            constraints.available_space(24.0, 12.0),
            FloatAvailableSpace {
                inline_start: 80.0,
                inline_size: 120.0,
            }
        );
        assert_eq!(
            constraints.available_space(40.0, 18.0),
            FloatAvailableSpace {
                inline_start: 0.0,
                inline_size: 200.0,
            }
        );
        assert_eq!(
            constraints.next_wider_block_start(0.0, 18.0, 60.0),
            Some(20.0)
        );
    }

    #[test]
    fn clearance_uses_only_the_requested_physical_float_sides() {
        let build = || {
            let mut context = horizontal_context(200.0);
            context.place_float(
                fixed_float(FloatSide::Left, 80.0),
                PhysicalSize {
                    width: 80.0,
                    height: 40.0,
                },
            );
            context.place_float(
                fixed_float(FloatSide::Right, 70.0),
                PhysicalSize {
                    width: 70.0,
                    height: 70.0,
                },
            );
            context
        };
        let clear = |side| BlockStyle {
            clear: side,
            ..fixed_width(200.0)
        };
        let size = PhysicalSize {
            width: 200.0,
            height: 10.0,
        };

        assert_eq!(
            build().place_in_flow(clear(ClearSide::Left), size).rect.y,
            40.0
        );
        assert_eq!(
            build().place_in_flow(clear(ClearSide::Right), size).rect.y,
            70.0
        );
        assert_eq!(
            build().place_in_flow(clear(ClearSide::Both), size).rect.y,
            70.0
        );
    }

    #[test]
    fn clearance_through_an_empty_box_starts_a_new_following_margin_chain() {
        let mut context = horizontal_context(200.0);
        context.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        let clear = BlockStyle {
            clear: ClearSide::Left,
            ..fixed_width(200.0)
        };
        let empty = context.place_in_flow_with_margins(
            clear,
            PhysicalSize {
                width: 200.0,
                height: 0.0,
            },
            margin_state(10.0, 20.0, true),
        );
        let following = context.place_in_flow_with_margins(
            fixed_width(200.0),
            PhysicalSize {
                width: 200.0,
                height: 10.0,
            },
            margin_state(30.0, 0.0, false),
        );

        assert_eq!(empty.rect.y, 40.0);
        assert_eq!(following.rect.y, 70.0);
        assert_eq!(context.used_block_size(), 80.0);
        assert!(context.active_margin_may_collapse_with_parent_end());

        let mut trailing_empty = horizontal_context(200.0);
        trailing_empty.place_float(
            fixed_float(FloatSide::Left, 80.0),
            PhysicalSize {
                width: 80.0,
                height: 40.0,
            },
        );
        trailing_empty.place_in_flow_with_margins(
            clear,
            PhysicalSize {
                width: 200.0,
                height: 0.0,
            },
            margin_state(10.0, 20.0, true),
        );

        assert_eq!(trailing_empty.used_block_size(), 60.0);
        assert!(!trailing_empty.active_margin_may_collapse_with_parent_end());
    }
}
