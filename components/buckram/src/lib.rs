//! Genet's standards-owned CSS layout model.
//!
//! Buckram owns CSS box identity, provenance, and layout fragments. Style
//! engines and layout algorithms integrate through these types without
//! defining their shape.

#![forbid(unsafe_code)]

mod box_tree;
mod fragment_tree;

pub use box_tree::{
    AnonymousBoxKind, BoxGeneration, BoxId, BoxOrigin, ContainingBlock, ContainingBlockRule,
    CssBox, CssBoxTree, DisplayInside, DisplayOutside, DisplayRole, FormattingContextKind,
    InternalTableRole, PositioningScheme, PseudoElement,
};
pub use fragment_tree::{
    Baselines, BreakToken, Fragment, FragmentId, FragmentTree, FragmentationContextId,
    LayoutResult, LogicalRect, PhysicalRect,
};
