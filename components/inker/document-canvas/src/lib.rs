/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # document-canvas
//!
//! Document-view canvas for the [`mere`](https://crates.io/crates/mere)
//! browser. Owns within-document layout: parley-driven text shaping +
//! simple block stacking, link interaction regions, and render-packet
//! derivation. Sibling to [`graph-canvas`](https://crates.io/crates/graph-canvas)
//! in the canvas-swatches taxonomy.
//!
//! Entry point: [`layout_document`].
//!
//! ## What it owns
//!
//! - Layout of every [`inker::Block`] variant into a portable
//!   [`DocumentRenderPacket`].
//! - Text shaping via [`parley`] (CPU-only, wasm32-portable; renders happen
//!   downstream).
//! - Hit-testable interaction regions for inline links.
//!
//! ## What it does NOT own
//!
//! - Rendering — packets, not pixels. Downstream backends consume them.
//! - Scrolling / viewport management — the caller supplies the viewport.
//! - Editing / interaction state — selection / cursor / IME are the host's
//!   job; document-canvas hands the host the layout to hang interaction off.
//! - A11y tree projection — uxtree consumes the same `EngineDocument`
//!   separately. Document-canvas is the *visual* side; uxtree is the
//!   *semantic* side.

#![doc(html_root_url = "https://docs.rs/document-canvas/0.0.1")]

pub mod font;
pub mod font_table;
pub mod layout;
pub mod paint_list;
pub mod style;
pub mod style_sheet;
pub mod text;
pub mod types;

/// Netrender backend — lowers an [`paint_list::InkerPaintList`] to a
/// `netrender::Scene` via the shared `paint_list_render` translator.
/// Behind the `netrender` cargo feature; pulls in wgpu transitively, so
/// wasm-light consumers skip it. The portable producer
/// ([`paint_list::paint_list_from_packet`]) is always available.
#[cfg(feature = "netrender")]
pub mod netrender_backend;

pub use font::{FontResolver, NoFontResolver};
pub use font_table::FontTable;
pub use layout::{LaidOutDocument, layout_document};
pub use paint_list::{InkerPaintList, paint_list_from_packet};
pub use style::{ColorVocabulary, InlineStyle};
pub use style_sheet::{
    BlockRole, BlockStyle, ColorToken, DocumentStyleSheet, FontChoice, HeadingStyle, LinkAdornment,
    ResolvedBlockStyle, RoleStyles, SizeSpec, WrapPolicy,
};
pub use types::{
    DocumentRenderPacket, FontFaceId, GlyphRun, InteractionKind, InteractionRegion, Point,
    PositionedGlyph, Rect, RenderedBlock, RenderedBlockKind, Size, TextStyle, Viewport,
};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Lifecycle stage marker.
pub const STAGE: &str = "pre-alpha";
