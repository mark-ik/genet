/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Arena/table stats shared across engine lanes.
//!
//! These are cheap, lane-neutral diagnostic shapes: integer counts and rough
//! byte estimates, not raw plane internals.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

/// Per-kind live-node counts for a DOM arena.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub struct DomNodeKindStats {
    pub documents: usize,
    pub document_fragments: usize,
    pub doctypes: usize,
    pub elements: usize,
    pub text: usize,
    pub comments: usize,
    pub processing_instructions: usize,
}

/// Cheap live stats for a DOM arena.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub struct DomArenaStats {
    pub live_nodes: usize,
    pub node_kinds: DomNodeKindStats,
    pub attribute_count: usize,
    /// Approximate bytes owned by the arena and its reachable dynamic buffers.
    pub estimated_bytes: usize,
}

/// What the last incremental-layout batch did.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub enum LayoutApplyKind {
    #[default]
    Unchanged,
    RepaintOnly,
    Restyled,
    Spliced,
    FullRecompute,
}

/// Coarse damage class for the last batch.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub enum LayoutDamageClass {
    #[default]
    None,
    PaintOnly,
    Relayout,
}

/// Cheap stats for the last incremental-layout batch.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct LayoutBatchStats {
    pub applied: LayoutApplyKind,
    pub damage: LayoutDamageClass,
    pub mutations_in: usize,
    pub coalesced_invalidations: usize,
    pub restyled_elements: usize,
    pub boxes_rebuilt: usize,
    pub fragment_count: usize,
    /// Present only when the retained box-tree side-table matches the current
    /// fragment plane. Structural splices deliberately invalidate that side-table.
    pub box_tree_nodes: Option<usize>,
}
