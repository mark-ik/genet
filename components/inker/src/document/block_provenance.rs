/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Per-block provenance.
//!
//! Some downstream features need to know not just where a *document*
//! came from but where each *block* in it came from. Examples:
//!
//! - **Mixed-source clips.** A knot built from `build_clip_knot` glues
//!   blocks from several sources into one document. The host wants to
//!   render the per-block source on hover.
//! - **Federated feed merge.** A view that fans across mootholds and
//!   shows entries from many feeds in one stream needs to label each
//!   entry's origin without duplicating the feed-level header per row.
//! - **Citation overlays.** A summarisation view that surfaces
//!   sentences from multiple documents wants to back each surfaced
//!   block to its source.
//!
//! Most documents have one source for every block (the document's own
//! [`crate::DocumentProvenance`]) — recording that per-block would be
//! redundant. Per-block provenance is therefore a **sparse sidecar**:
//! a [`BlockProvenanceMap`] keyed by [`crate::EngineDocument::blocks`]
//! index, where unrecorded entries fall back to the document-level
//! provenance. Construction sites that don't need the sidecar simply
//! don't allocate one — [`crate::EngineDocument`]'s shape stays
//! unchanged.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::DocumentProvenance;

/// Provenance for a single block within an [`crate::EngineDocument`].
///
/// Wraps [`DocumentProvenance`] and adds an optional in-source anchor
/// — the hash, byte range, or other locator the source format provides
/// — so a downstream view can scroll back to the precise origin.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BlockProvenance {
    /// Same fields as the document-level record, but scoped to this
    /// block. When a block came from a different source than its
    /// containing document, this is the block's source.
    #[serde(default, flatten)]
    pub provenance: DocumentProvenance,
    /// Best-available anchor inside the source — line range
    /// (`"42-58"`), header anchor (`"#installation"`), feed entry GUID,
    /// or empty when no anchor exists.
    #[serde(default)]
    pub anchor: Option<String>,
}

impl BlockProvenance {
    /// Build a block-provenance record from a document-level provenance
    /// (e.g. when a clip composer wants to mark "this whole block came
    /// from this source as-is").
    pub fn from_document(provenance: DocumentProvenance) -> Self {
        Self {
            provenance,
            anchor: None,
        }
    }

    /// Convenience: record an anchor (line range, fragment, GUID)
    /// alongside the provenance.
    pub fn with_anchor(mut self, anchor: impl Into<String>) -> Self {
        self.anchor = Some(anchor.into());
        self
    }
}

/// Sparse map from
/// [`crate::EngineDocument::blocks`] index to a per-block provenance
/// override.
///
/// Lookups for indices not present in the map should fall back to the
/// document's own [`DocumentProvenance`]. The map only records blocks
/// whose source differs from the document, keeping the common case
/// (single-source documents) free.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockProvenanceMap {
    by_block: HashMap<usize, BlockProvenance>,
}

impl BlockProvenanceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record per-block provenance for the block at `block_index`.
    pub fn insert(&mut self, block_index: usize, provenance: BlockProvenance) {
        self.by_block.insert(block_index, provenance);
    }

    /// Look up an override for `block_index`, if one was recorded.
    pub fn get(&self, block_index: usize) -> Option<&BlockProvenance> {
        self.by_block.get(&block_index)
    }

    /// Resolve provenance for a block: the per-block override if one
    /// was recorded, otherwise the document-level fallback.
    pub fn resolve<'a>(
        &'a self,
        block_index: usize,
        document_fallback: &'a DocumentProvenance,
    ) -> ResolvedProvenance<'a> {
        if let Some(block) = self.by_block.get(&block_index) {
            ResolvedProvenance {
                provenance: &block.provenance,
                anchor: block.anchor.as_deref(),
                from_block_override: true,
            }
        } else {
            ResolvedProvenance {
                provenance: document_fallback,
                anchor: None,
                from_block_override: false,
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_block.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_block.len()
    }

    /// Iterate `(block_index, &BlockProvenance)` pairs in unspecified
    /// order. Callers that need deterministic output (frontmatter
    /// emission, debug dumps) should collect and sort by index.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &BlockProvenance)> {
        self.by_block.iter().map(|(idx, prov)| (*idx, prov))
    }
}

/// Result of resolving provenance for a single block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedProvenance<'a> {
    pub provenance: &'a DocumentProvenance,
    pub anchor: Option<&'a str>,
    /// `true` when the resolution came from the sidecar map; `false`
    /// when it fell back to the document-level provenance.
    pub from_block_override: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_provenance() -> DocumentProvenance {
        DocumentProvenance::for_engine("nematic.markdown", "file:///doc.md")
    }

    fn other_provenance() -> DocumentProvenance {
        DocumentProvenance::for_engine("nematic.gemtext", "gemini://other/")
    }

    #[test]
    fn empty_map_resolves_to_document_fallback() {
        let map = BlockProvenanceMap::new();
        let doc = doc_provenance();
        let resolved = map.resolve(0, &doc);
        assert_eq!(resolved.provenance, &doc);
        assert!(resolved.anchor.is_none());
        assert!(!resolved.from_block_override);
    }

    #[test]
    fn override_wins_for_recorded_block_only() {
        let mut map = BlockProvenanceMap::new();
        let other = other_provenance();
        map.insert(
            2,
            BlockProvenance::from_document(other.clone()).with_anchor("L42-L58"),
        );

        let doc = doc_provenance();

        let block_2 = map.resolve(2, &doc);
        assert_eq!(block_2.provenance, &other);
        assert_eq!(block_2.anchor, Some("L42-L58"));
        assert!(block_2.from_block_override);

        // Unrecorded blocks fall back.
        let block_0 = map.resolve(0, &doc);
        assert_eq!(block_0.provenance, &doc);
        assert!(!block_0.from_block_override);
    }

    #[test]
    fn map_is_sparse_and_reports_size() {
        let mut map = BlockProvenanceMap::new();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);

        map.insert(5, BlockProvenance::from_document(other_provenance()));
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);

        // Inserting at the same index replaces, doesn't grow.
        map.insert(5, BlockProvenance::from_document(doc_provenance()));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn with_anchor_attaches_anchor() {
        let bp = BlockProvenance::from_document(other_provenance()).with_anchor("guid:abc");
        assert_eq!(bp.anchor.as_deref(), Some("guid:abc"));
        assert_eq!(
            bp.provenance.canonical_uri.as_deref(),
            Some("gemini://other/")
        );
    }

    #[test]
    fn get_returns_recorded_block_only() {
        let mut map = BlockProvenanceMap::new();
        map.insert(7, BlockProvenance::from_document(other_provenance()));
        assert!(map.get(7).is_some());
        assert!(map.get(0).is_none());
    }
}
