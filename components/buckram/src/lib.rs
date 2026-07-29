//! Genet's standards-owned CSS layout model.
//!
//! Buckram owns CSS box identity, provenance, and layout fragments. Style
//! engines and layout algorithms integrate through these types without
//! defining their shape.

#![forbid(unsafe_code)]

mod block;
mod box_tree;
mod flow;
mod fragment_tree;
mod intrinsic;
mod table;
mod taffy_adapter;

pub use block::{
    BlockBoxSizing, BlockContainingBlock, BlockDeferral, BlockDimensions, BlockFormattingContext,
    BlockMarginCollapse, BlockMarginState, BlockPlacement, BlockPosition, BlockSizeValue,
    BlockStyle, ClearSide, CollapsedMargin, FloatAvailableSpace, FloatAvoidingPlacement,
    FloatLineConstraints, FloatSide, FlowLength, FlowLengthAuto, UsedInlineSize,
    solve_float_inline_size, solve_in_flow_inline_size, solve_in_flow_inline_size_for_available,
    solve_shrink_to_fit_inline_size,
};
pub use box_tree::{
    AnonymousBoxKind, BoxGeneration, BoxId, BoxOrigin, BoxTreeInput, ContainingBlock,
    ContainingBlockRule, CssBox, CssBoxTree, DisplayInside, DisplayOutside, DisplayRole,
    FloatContextProvenance, FormattingContextKind, InternalTableRole, PositioningScheme,
    PseudoElement, generate_box_tree,
};
pub use flow::{
    Direction, FlowAxes, LogicalAxis, LogicalRect, LogicalSides, LogicalSize, PhysicalRect,
    PhysicalSide, PhysicalSides, PhysicalSize, WritingMode,
};
pub use fragment_tree::{
    Baselines, BreakToken, Fragment, FragmentId, FragmentTree, FragmentationContextId, LayoutResult,
};
pub use intrinsic::{
    IntrinsicQueryError, IntrinsicQueryState, IntrinsicSizeCache, IntrinsicSizeKind,
    IntrinsicSizeQuery, IntrinsicSizes, block_intrinsic_sizes_for_definite_inline,
};
pub use table::{
    AffineLengthPercentage, CaptionMinContribution, CellInlineOffsets, InlineSizeConstraint,
    TableAutomaticColumnGroupInput, TableAutomaticColumnInput, TableAutomaticColumnMeasureInput,
    TableAutomaticColumnMeasures, TableAutomaticInlineSizingIndefinite,
    TableAutomaticInlineSizingInput, TableAutomaticInlineSizingOutcome, TableBoxSizing, TableCell,
    TableCellInlineMeasure, TableCellInlineStyle, TableCellInput, TableColumnMeasure,
    TableDeferral, TableFixedColumnGroupInput, TableFixedColumnInput, TableFixedInlineSizingInput,
    TableFixedInlineSizingOutcome, TableFixedLayoutFallback, TableGrid, TableGridError,
    TableGridInputs, TableInlineBorderMetrics, TableInlineConstraints, TableInlineProperty,
    TableInlineSizingError, TableInlineSizingInput, TableInlineSizingResult,
    TableIntrinsicMeasureProvider, TableRowSpan, TableSeparatedBorderMetrics, TableSlot,
    TableSpanMeasureDistribution, TableTrack, TableTrackGroup, TableTrackGroupKind,
    TableTrackInput, TableTrackVisibility, TableTrackVisibilityState,
    cache_automatic_table_grid_intrinsic_sizes, collect_table_cell_inline_measures,
    measure_automatic_columns, query_table_cell_inline_sizes, size_automatic_table_inline,
    size_fixed_table_inline,
};
pub use taffy_adapter::{
    AlgorithmAvailableSpace, AlgorithmKind, AlgorithmLayout, AlgorithmNodeId, AlgorithmSize,
    AlgorithmStyle, AlgorithmTree, BlockAlgorithm,
};
