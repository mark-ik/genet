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
    FormattingContextKind, InternalTableRole, PositioningScheme, PseudoElement, generate_box_tree,
};
pub use flow::{
    Direction, FlowAxes, LogicalAxis, LogicalRect, LogicalSides, LogicalSize, PhysicalRect,
    PhysicalSide, PhysicalSides, PhysicalSize, WritingMode,
};
pub use fragment_tree::{
    Baselines, BreakToken, Fragment, FragmentId, FragmentTree, FragmentationContextId, LayoutResult,
};
pub use intrinsic::{IntrinsicSizeCache, IntrinsicSizeKind, IntrinsicSizeQuery, IntrinsicSizes};
pub use taffy_adapter::{
    AlgorithmAvailableSpace, AlgorithmKind, AlgorithmLayout, AlgorithmNodeId, AlgorithmSize,
    AlgorithmStyle, AlgorithmTree, BlockAlgorithm,
};
