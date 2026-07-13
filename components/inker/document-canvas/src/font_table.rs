/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The font sidecar: parley's actual shaped faces, out-of-band from the
//! serializable [`DocumentRenderPacket`](crate::DocumentRenderPacket).
//!
//! Each [`GlyphRun`](crate::GlyphRun) carries a [`FontFaceId`] (a plain
//! serializable `u32`); the real face bytes — `parley::FontData`, an
//! `Arc`-backed `Blob` plus a collection index — live here. Layout
//! produces the table; the paint-list producer reads it to populate the
//! `PaintList`'s `fonts()` side-table.
//!
//! ## Why a sidecar and not a packet field
//!
//! `DocumentRenderPacket` is `Serialize + Deserialize + PartialEq`;
//! `parley::FontData` is none of those trivially (it's an `Arc<Blob>`
//! handle, not owned bytes). Hanging it on the packet behind
//! `#[serde(skip)]` would make the packet silently lossy on round-trip
//! (deserialize → empty faces → placeholder-only text) and muddy its
//! `PartialEq`. Instead the handles ride beside the packet
//! ([`crate::LaidOutDocument`]); owned bytes materialize only at the
//! `paint_list_api` boundary. The **`PaintList`**, which *does* carry
//! owned `FontResource` bytes, is the IPC-self-contained form — not the
//! packet.
//!
//! Mirrors genet-layout's `FontCollector`: dedup by `parley::Blob::id()`
//! (a stable per-allocation id), so a face shared across many runs is
//! stored once.

use std::collections::HashMap;

use parley::FontData;

use crate::types::FontFaceId;

/// The font sidecar produced alongside a [`DocumentRenderPacket`]. Maps
/// each [`FontFaceId`] recorded on a `GlyphRun` to the `parley::FontData`
/// the run was shaped against. Holds `Arc`-cheap handles, not owned bytes.
#[derive(Clone, Debug, Default)]
pub struct FontTable {
    faces: Vec<FontData>,
}

impl FontTable {
    /// The face for `id`, or `None` if the id is out of range (shouldn't
    /// happen for ids minted by the matching [`FontInterner`]).
    pub fn get(&self, id: FontFaceId) -> Option<&FontData> {
        self.faces.get(id.0 as usize)
    }

    /// Number of distinct faces.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    /// Iterate `(FontFaceId, &FontData)` in id order.
    pub fn iter(&self) -> impl Iterator<Item = (FontFaceId, &FontData)> {
        self.faces
            .iter()
            .enumerate()
            .map(|(i, f)| (FontFaceId(i as u32), f))
    }
}

/// Build-time accumulator that dedups parley faces by `Blob::id()` and
/// hands back stable [`FontFaceId`]s. Lives in the layouter during a
/// `layout_document` pass; [`into_table`](Self::into_table) seals it into
/// the [`FontTable`] sidecar.
#[derive(Default)]
pub struct FontInterner {
    faces: Vec<FontData>,
    by_blob: HashMap<u64, FontFaceId>,
}

impl FontInterner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern parley's chosen face for a run, returning the id to record
    /// on the `GlyphRun`. Identical faces (same `Blob::id()`) collapse to
    /// one id. The `FontData` clone is an `Arc` bump, not a byte copy.
    pub fn intern(&mut self, font: &FontData) -> FontFaceId {
        let blob_id = font.data.id();
        if let Some(&id) = self.by_blob.get(&blob_id) {
            return id;
        }
        let id = FontFaceId(self.faces.len() as u32);
        self.faces.push(font.clone());
        self.by_blob.insert(blob_id, id);
        id
    }

    /// Seal the accumulated faces into a [`FontTable`].
    pub fn into_table(self) -> FontTable {
        FontTable { faces: self.faces }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Dedup behaviour is verified end-to-end against real parley faces in
    // `layout.rs` (`document_dedups_shared_face` /
    // `distinct_families_intern_distinct_faces`) and `paint_list.rs` —
    // `parley::FontData` can't be constructed synthetically without
    // pulling in `linebender_resource_handle` directly, and a real layout
    // is the more honest exercise of the intern path (mirrors
    // genet-layout's `FontCollector` tests).

    #[test]
    fn empty_table_get_is_none() {
        let table = FontInterner::new().into_table();
        assert!(table.is_empty());
        assert!(table.get(FontFaceId(0)).is_none());
    }
}
