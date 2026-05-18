/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Laid-out geometry queries — hit-testing, box-model lookup,
//! anchor-to-fragments, selection rects.
//!
//! Cf. Hekate doc §"Layout/Fragment Plane". The trait is the
//! permanent ABI; internal plane storage (IndexVec / FxHashMap /
//! whatever) stays each lane's implementation detail.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use crate::types::{Point, Rect, SourceNodeId, SourceRange};

/// Common-minimum laid-out-geometry queries. Each lane that has a
/// layout phase implements this. Internal `FragmentId` type is per-
/// lane (opaque to consumers — only used as a map key).
pub trait FragmentQuery {
    /// Per-lane opaque fragment identity. Consumers compare for
    /// equality and use as map keys.
    type FragmentId: Copy + Eq + std::hash::Hash;

    /// Epoch — invalidated on any relayout. Consumers cache against
    /// this; the value rolls when the plane regenerates.
    fn generation_id(&self) -> u64;

    /// Hit-test at a viewport point. Returns the topmost fragment
    /// hit (paint-order semantics), or `None` if the point falls
    /// outside any fragment.
    fn hit_test(&self, point: Point) -> Option<FragmentHit<Self::FragmentId>>;

    /// CSS box-model for a source node. None if the node has no
    /// fragment (e.g., `display: none`, or before layout completes).
    fn box_model(&self, source_id: SourceNodeId) -> Option<BoxModel>;

    /// Fragments under a named anchor (e.g., `#section-2`).
    fn fragments_for_anchor<'a>(
        &'a self,
        anchor: &str,
    ) -> Box<dyn Iterator<Item = Self::FragmentId> + 'a>;

    /// Reverse mapping: fragment → source span. Used by selection,
    /// "what node was here," and Apparatus.
    fn text_range_for_fragment(&self, fragment: Self::FragmentId) -> Option<SourceRange>;

    /// Selection → screen rects. Multi-rect because a selection can
    /// span lines or wrap; consumers (selection-highlight painter)
    /// draw one rect per returned entry.
    fn rects_for_selection(&self, range: SourceRange) -> Vec<Rect>;
}

/// What `hit_test` returns. Carries the lane's fragment id, the
/// containing source node, and the local hit point inside the
/// fragment (so consumers can drive caret positioning without
/// re-walking).
#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct FragmentHit<FragmentId: Copy> {
    pub fragment: FragmentId,
    pub source_node: SourceNodeId,
    /// Hit point in the fragment's local space (origin at the
    /// fragment's top-left).
    pub local_point: Point,
}

/// CSS box-model for one source node's fragment. The four nested
/// rectangles — content / padding / border / margin — are each in
/// viewport coordinates. Consumers wanting `getBoundingClientRect`
/// shape read `border` (the border-edge rect).
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct BoxModel {
    /// Content-box rect (inside padding).
    pub content: Rect,
    /// Padding-box rect (between border and content).
    pub padding: Rect,
    /// Border-box rect (between margin and padding). Matches
    /// `getBoundingClientRect()` in DOM semantics.
    pub border: Rect,
    /// Margin-box rect (outermost).
    pub margin: Rect,
}
