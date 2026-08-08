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
    AffineLengthPercentage, CaptionMinContribution, CellBlockOffsets, CellCollapsedBorderMetrics,
    CellInlineOffsets, CollapsedBorderInteropDeferral, CollapsedBorderMetricError,
    CollapsedBorderMetrics, CollapsedBorderProjection, CollapsedBorderSegmentMetric,
    CollapsedBorderSideMetrics, FragmentDraft, FragmentDraftTree, GridEdgeOrientation,
    InlineSizeConstraint, ResolvedTableBorder, ResolvedTableBorderGrid, TableAlignment,
    TableAutomaticColumnGroupInput, TableAutomaticColumnInput, TableAutomaticColumnMeasureInput,
    TableAutomaticColumnMeasures, TableAutomaticInlineSizingIndefinite,
    TableAutomaticInlineSizingInput, TableAutomaticInlineSizingOutcome, TableBlockBorderMetrics,
    TableBlockConstraint, TableBlockDeferral, TableBlockLayout, TableBlockSizingInput,
    TableBorderCandidate, TableBorderCandidates, TableBorderDisposition, TableBorderError,
    TableBorderLedgerEntry, TableBorderOrderKey, TableBorderOrigin, TableBorderPrecedence,
    TableBorderResolutionError, TableBorderSide, TableBorderSides, TableBorderSource,
    TableBorderSources, TableBorderStyle, TableBoxSizing, TableCell, TableCellAlignment,
    TableCellBlockStyle, TableCellFormatter, TableCellInlineMeasure, TableCellInlineStyle,
    TableCellInput, TableCellLayoutInput, TableCellLayoutOutput, TableCellLayoutPass,
    TableCellPlacement, TableColumnMeasure, TableDeferral, TableFixedColumnGroupInput,
    TableFixedColumnInput, TableFixedInlineSizingInput, TableFixedInlineSizingOutcome,
    TableFixedLayoutFallback, TableFragment, TableFragmentRole, TableFragments, TableGrid,
    TableGridEdge, TableGridError, TableGridInputs, TableInlineBorderMetrics,
    TableInlineConstraints, TableInlineProperty, TableInlineSizingError, TableInlineSizingInput,
    TableInlineSizingResult, TableIntrinsicMeasureProvider, TablePercentagePass, TableRowBaseline,
    TableRowLayoutError, TableRowMeasure, TableRowSizing, TableRowSpan, TableSeparatedBlockMetrics,
    TableSeparatedBorderMetrics, TableSlot, TableSpanMeasureDistribution, TableTrack,
    TableTrackGroup, TableTrackGroupKind, TableTrackInput, TableTrackVisibility,
    TableTrackVisibilityState, align_table_cells, apply_baseline_row_minima,
    cache_automatic_table_grid_intrinsic_sizes, collect_table_border_candidates,
    collect_table_cell_inline_measures, compare_table_border_candidates, emit_table_fragments,
    format_table_cells, layout_table_block, measure_automatic_columns, measure_single_span_rows,
    project_collapsed_border_metrics, query_table_cell_inline_sizes,
    resolve_percentage_block_sizes, resolve_table_border_candidates, size_automatic_table_inline,
    size_fixed_table_inline, size_table_rows, spanned_cell_content_inline_size,
};
pub use taffy_adapter::{
    AlgorithmAvailableSpace, AlgorithmKind, AlgorithmLayout, AlgorithmNodeId, AlgorithmSize,
    AlgorithmStyle, AlgorithmTree, BlockAlgorithm,
};
