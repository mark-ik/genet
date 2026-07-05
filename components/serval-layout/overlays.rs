/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Overlay slots — a top-layer subtree anchored to a page element
//! (css-anchor-positioning + Popover/top-layer + a UA shadow root, subset).
//!
//! This is the "overlay slot" of the overlay-roots plan
//! (`mere:design_docs/.../2026-07-05_overlay_roots_and_ua_widgets_plan.md`):
//! link previews, autofill chips, annotation pins, reader mode. The design
//! reuses the highlight slot's discipline — a registry on the layout, painted
//! after content emission, with geometry re-derived at emit time — but the
//! payload is a laid-out subtree instead of a fill.
//!
//! **The architecture proved by this probe (P0 (a)-(d)):**
//! - **No reflow leak (a):** a slot's content is a *separately* emitted paint
//!   list stored beside the page; the page's box tree and fragment plane never
//!   see it, so the page lays out byte-identically with or without the slot.
//! - **Anchor tracking (b):** the slot paints at the anchor node's *current*
//!   fragment rect, resolved at emit time, so scroll and anchor-moving
//!   mutations reposition it for free — the same re-derive-at-emit property the
//!   highlight slot relies on.
//! - **Style isolation (c):** the content paint list is produced by the
//!   satellite's *own* [`crate::lay_out_content`] call with its own stylesheet
//!   (a UA shadow root, subset). The page cascade is never handed to it, so a
//!   page rule matching the satellite's element cannot restyle it.
//! - **Top layer (d):** the slot's commands append *after* the page's emit walk
//!   (which balances its transform stack to identity), wrapped in one
//!   `PushTransform` to the anchor — so it paints above every page stacking
//!   context, like the CSS top layer.
//!
//! **Subset, v0 (deliberate cuts, not oversights):**
//! - Anchor positioning is "the anchor's top-left" only; the spec's
//!   `position-area` / `anchor()` insets and flipping slot in later without
//!   changing this shape (a slot already carries its content's intrinsic size,
//!   which is what flipping/overflow-avoidance needs).
//! - Content is **fill-only** in the probe (backgrounds, borders — no text or
//!   images), so the composed sublist needs no font/image side-table merge.
//!   Text-bearing overlays (autofill chips) add that merge when P1's remote
//!   runner lands real views; the seam is [`ServalPaintList::push_sublist`].
//! - Popover dismissal/nesting and true top-layer promotion semantics are the
//!   host's concern in the probe; here "top layer" means paint-order-last.

use std::collections::BTreeMap;

use crate::ServalPaintList;

/// One overlay slot: a laid-out satellite (its pre-emitted, page-independent
/// paint list) anchored to a page node. Registered by name on the layout;
/// painted band-shifted at the anchor's current fragment position.
pub struct OverlaySlot<Id> {
    /// The page node the slot anchors to. Its live fragment rect (resolved at
    /// emit time) is the slot's origin, so the overlay tracks the anchor.
    pub anchor: Id,
    /// The satellite's own emitted paint list, in satellite-local coordinates
    /// (origin at its top-left). Produced from the satellite's isolated
    /// cascade, so the page's sheets never reach it.
    pub content: ServalPaintList,
}

/// The per-document overlay registry: name → slot. Owned by the layout,
/// painted after content + highlights (top-layer order).
pub(crate) type OverlayRegistry<Id> = BTreeMap<String, OverlaySlot<Id>>;
