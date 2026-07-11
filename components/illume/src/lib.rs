//! illume: a portable text lexer and syntax highlighter for the Mere browser.
//!
//! The editor pipe, host-agnostic and toolkit-free. It reads `.knot` source text
//! and produces the `(range, kind)` highlight channel the edit surface paints,
//! plus the inner-language injection registry. This is the *editor* pipe in the
//! plan's two-parser split: it colors and (later) navigates the source. It is
//! separate from the *meaning* pipe (nematic's `DjotKnotEngine`: text →
//! `Block`), and both read the same bytes. The source text is always the
//! single source of truth; nothing here mutates it.
//!
//! Pure Rust and wasm-clean by construction (jotdown plus, later, `logos` inner
//! lexers), so native and the browser build the same way. See
//! `design_docs/mere_docs/implementation_strategy/2026-06-24_djot_editor_knot_nodes_plan.md`.

pub mod entity;
pub mod highlight;
pub mod injection;
pub mod pack;
pub mod tree;

pub use entity::entities;
pub use highlight::{Span, SyntaxKind, highlight, highlight_djot};
pub use injection::{InjectionLexer, InjectionRegistry};
pub use pack::default_pack;
pub use tree::{
    Fold, NodeKind, OutlineItem, TreeNode, container_tree, expand_selection, folds, outline,
};
