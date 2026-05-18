/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Fragment plane skeleton — laid-out rects keyed by DOM `NodeId`.
//!
//! After Taffy runs, this plane stores the per-node layout result so
//! consumers (display-list emission, hit-testing, apparatus inspector,
//! eventual `getBoundingClientRect`) can read positions back without
//! re-running layout. Today the plane is just a `NodeId → taffy::Layout`
//! map; the eventual impl will carry richer fragment data (line boxes,
//! pseudo-element fragments, scroll-container metadata) per the planes
//! doc.
//!
//! Per the Hekate doc's "publishing observables" rule, the plane is
//! `pub(crate)` and the public ABI is a query trait (`FragmentQuery`)
//! living in a future `engine_observables_api` crate. The probe slice
//! exposes just enough for tests to assert rects came out non-zero.

use std::hash::Hash;

use rustc_hash::FxHashMap;
use taffy::Layout;

pub struct FragmentPlane<NodeId: Copy + Eq + Hash> {
    pub(crate) rects: FxHashMap<NodeId, Layout>,
}

impl<NodeId: Copy + Eq + Hash> Default for FragmentPlane<NodeId> {
    fn default() -> Self {
        Self {
            rects: FxHashMap::default(),
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
