/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Style plane skeleton.
//!
//! Per the planes architecture, computed style lives in a `serval-layout`-
//! owned side table keyed by `D::NodeId`. The real implementation will be
//! populated by Stylo's cascade running over `NodeRef` (Stylo trait impls
//! live in `adapter_stylo.rs`, currently a draft). For the probe slice,
//! `StylePlane` is populated by hand — the test constructs the entries
//! directly, bypassing the cascade. This validates the construct + Taffy
//! pipeline without committing to the Stylo adapter shape yet.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use std::hash::Hash;

use rustc_hash::FxHashMap;
use taffy::Style as TaffyStyle;

/// Per-node style entry. The probe stores only the Taffy-shaped style;
/// the eventual full impl will store Stylo's `ComputedValues` here and
/// derive the Taffy style on demand (or cache both).
#[derive(Clone, Debug, Default)]
pub struct StyleEntry {
    pub taffy: TaffyStyle,
}

/// Sparse storage of computed style keyed by `D::NodeId`. Sparse for the
/// probe; the eventual impl picks dense `IndexVec` storage when
/// `D::NodeId` is dense (per `NodeIdSpace` in the planes doc).
pub struct StylePlane<NodeId: Copy + Eq + Hash> {
    entries: FxHashMap<NodeId, StyleEntry>,
}

impl<NodeId: Copy + Eq + Hash> Default for StylePlane<NodeId> {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
        }
    }
}

impl<NodeId: Copy + Eq + Hash> StylePlane<NodeId> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, id: NodeId, entry: StyleEntry) {
        self.entries.insert(id, entry);
    }

    pub fn get(&self, id: NodeId) -> Option<&StyleEntry> {
        self.entries.get(&id)
    }

    /// The Taffy style for a node, or Taffy's default style if no entry.
    /// Defaulting (rather than panicking) lets construct.rs handle nodes
    /// without explicit style entries (text nodes, anonymous boxes, etc.).
    pub fn taffy_style(&self, id: NodeId) -> TaffyStyle {
        self.entries
            .get(&id)
            .map(|e| e.taffy.clone())
            .unwrap_or_default()
    }
}
