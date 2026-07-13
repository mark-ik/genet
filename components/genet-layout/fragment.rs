/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Fragment plane — laid-out rects keyed by DOM `NodeId`.
//!
//! After Taffy runs, this plane stores the per-node layout result so consumers
//! (paint emission, hit-testing, the apparatus inspector,
//! `getBoundingClientRect`-shaped queries) can read positions back without
//! re-running layout. The plane is a `NodeId → taffy::Layout` map; richer
//! fragment data (line boxes, pseudo-element fragments, scroll-container
//! metadata) is a future extension per the planes doc.
//!
//! Per the Hekate doc's "publishing observables" rule the plane is `pub(crate)`,
//! and the public query surface is the `engine_observables_api` `FragmentQuery`
//! trait, implemented by `GenetLaneView` (`genet_lane.rs`).

use std::hash::Hash;

use rustc_hash::FxHashMap;
use taffy::Layout;

#[derive(Clone)]
pub struct FragmentPlane<NodeId: Copy + Eq + Hash> {
    pub(crate) rects: FxHashMap<NodeId, Layout>,
    /// Absolute (layout-space) origins of boxes the box tree **hoisted** to a
    /// containing block that is not their DOM parent (position-containing-block
    /// plan: `fixed` to the ICB today, `absolute` to its positioned ancestor
    /// under F2). Their `Layout.location` is relative to the *hoist* parent, so
    /// DOM-driven origin accumulation (hit-testing, `absolute_origin`, a11y
    /// bounds) would add the DOM ancestors' offsets a second time; walkers that
    /// find a node here use this origin standalone instead. Filled from the box
    /// tree at fragment-readback time — the one source of truth, so walkers and
    /// paint agree by data rather than by re-derived predicates.
    pub(crate) hoisted_origins: FxHashMap<NodeId, (f32, f32)>,
    /// The reverse view: hoist target -> the boxes hoisted **to** it (the
    /// root's DOM id for `fixed`, the positioned ancestor's for `absolute`).
    /// The hit walk defers a hoisted box from its *target's* frame — the frame
    /// whose accumulated point mapping (scrolls above the containing block,
    /// clips on the containing-block chain) is the one that legitimately
    /// applies to it — rather than from its DOM parent's, where intermediate
    /// static clippers/scrollers would wrongly apply.
    pub(crate) hoisted_by_target: FxHashMap<NodeId, Vec<NodeId>>,
}

impl<NodeId: Copy + Eq + Hash> Default for FragmentPlane<NodeId> {
    fn default() -> Self {
        Self {
            rects: FxHashMap::default(),
            hoisted_origins: FxHashMap::default(),
            hoisted_by_target: FxHashMap::default(),
        }
    }
}

impl<NodeId: Copy + Eq + Hash> FragmentPlane<NodeId> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: NodeId, layout: Layout) {
        self.rects.insert(id, layout);
    }

    /// The absolute origin of a hoisted out-of-flow box (see
    /// [`Self::hoisted_origins`]), or `None` for every in-flow box.
    pub fn hoisted_origin(&self, id: NodeId) -> Option<(f32, f32)> {
        self.hoisted_origins.get(&id).copied()
    }

    /// The boxes hoisted **to** `id` (see [`Self::hoisted_by_target`]); empty
    /// for every node that is not a hoist target.
    pub fn hoisted_children(&self, id: NodeId) -> &[NodeId] {
        self.hoisted_by_target.get(&id).map_or(&[], Vec::as_slice)
    }

    /// Read the laid-out rect for a node, if it was reached by layout.
    /// Non-element nodes (text, comment, document) won't have entries
    /// in the probe — see `construct.rs`.
    pub fn rect_of(&self, id: NodeId) -> Option<&Layout> {
        self.rects.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &Layout)> {
        self.rects.iter()
    }

    pub fn len(&self) -> usize {
        self.rects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }
}
