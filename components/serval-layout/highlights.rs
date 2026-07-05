/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! CSS Custom Highlight API subset (css-highlight-api-1): a host-registered
//! registry of **named highlights**, each a set of static ranges, painted by
//! the engine from the retained layout — no DOM nodes, no boxes.
//!
//! This is the "highlight slot" of the overlay-roots plan
//! (`mere:design_docs/.../2026-07-05_overlay_roots_and_ua_widgets_plan.md`):
//! the cheapest satellite tier. Because a highlight is stored as a *range*
//! and its rectangles derive at emit time through the same primitives text
//! selection uses ([`crate::caret::selection_rects`]), highlights survive
//! relayout, reflow, and scrolling without re-registration — the property the
//! spec's `HighlightRegistry` (css-highlight-api-1 §3, "highlights are not
//! live ranges" notwithstanding: static ranges re-resolved against layout)
//! is designed around.
//!
//! **Subset, v0** (each deviation is a deliberate cut, not an oversight):
//! - A range addresses one text-bearing node's laid-out text by byte offsets
//!   (`HighlightRange { node, start, end }`); cross-node ranges decompose into
//!   one entry per node at registration (the host's find worker already
//!   produces per-node matches).
//! - Painting is a translucent fill over the content, matching the host's
//!   existing selection rendering. css-highlight-api-1 §5 paints highlight
//!   backgrounds *below the text's ink*; moving under-ink (and supporting
//!   `::highlight(name)` color/text styling from author sheets) is the
//!   follow-on that rides real cascade participation.
//! - Priority is registration-name order (BTreeMap); the spec's explicit
//!   `priority` field can slot in without changing the shape.

use std::collections::BTreeMap;

use paint_list_api::ColorF;

/// How one named highlight paints. v0: a translucent fill color (straight
/// alpha). The spec's model styles highlights through `::highlight(name)`
/// pseudo rules; this style struct is the engine-internal stand-in until
/// highlight pseudos join the cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HighlightStyle {
    pub color: ColorF,
}

/// One highlighted range: byte offsets into `node`'s laid-out text (the same
/// addressing [`crate::caret::selection_rects`] takes). Ranges are static;
/// geometry re-derives at every emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HighlightRange<Id> {
    pub node: Id,
    pub start: usize,
    pub end: usize,
}

/// The per-document highlight registry: name → (ranges, style). Owned by the
/// layout session; painted by `emit_paint_list` after content emission.
pub(crate) type HighlightRegistry<Id> = BTreeMap<String, (Vec<HighlightRange<Id>>, HighlightStyle)>;
