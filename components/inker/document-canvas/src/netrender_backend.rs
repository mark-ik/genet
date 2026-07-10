/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Netrender backend.
//!
//! Thin lowering shim: builds an [`crate::paint_list::InkerPaintList`]
//! from a [`crate::DocumentRenderPacket`] (plus its font sidecar) and
//! lowers it to a `netrender::Scene` via the shared `paint_list_render`
//! translator — the same path serval and any other engine use. All the
//! document → paint-command logic lives in [`crate::paint_list`]; this
//! module only exists to feed the result through the renderer-coupled
//! translator (behind the `netrender` feature).

use netrender::Scene;

use crate::font_table::FontTable;
use crate::paint_list::paint_list_from_packet;
use crate::style::ColorVocabulary;
use crate::types::DocumentRenderPacket;

/// Build a `netrender::Scene` from a laid-out document.
///
/// `fonts` is the [`FontTable`](crate::FontTable) sidecar produced by
/// [`layout_document`](crate::layout_document); it supplies the real face
/// bytes parley shaped each run against (so glyphs render against the
/// face they were shaped with). `colors` supplies the theme primitives.
pub fn scene_from_packet(
    packet: &DocumentRenderPacket,
    fonts: &FontTable,
    colors: &ColorVocabulary,
) -> Scene {
    let list = paint_list_from_packet(packet, fonts, colors);
    paint_list_render::translate_paint_list(&list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::layout_document;
    use crate::style_sheet::DocumentStyleSheet;
    use crate::types::Viewport;
    use inker::{Block, DocumentProvenance, DocumentTrustState, EngineDocument, InlineSpan};

    fn doc(blocks: Vec<Block>) -> EngineDocument {
        EngineDocument {
            address: "doc:netrender-test".into(),
            title: None,
            content_type: "text/plain".into(),
            lang: None,
            provenance: DocumentProvenance::default(),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        }
    }

    fn scene_for(blocks: Vec<Block>) -> Scene {
        let laid = layout_document(
            &doc(blocks),
            Viewport::new(640.0, 480.0),
            &DocumentStyleSheet::default(),
        );
        scene_from_packet(&laid.packet, &laid.fonts, &ColorVocabulary::default())
    }

    #[test]
    fn empty_packet_produces_empty_scene() {
        let scene = scene_for(vec![]);
        assert_eq!(scene.ops.len(), 0);
    }

    #[test]
    fn text_lowers_to_glyph_run() {
        // Parley's real face threads through to the side-table, so text
        // lowers to a SceneOp::GlyphRun — no resolver, no placeholder rect.
        let scene = scene_for(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text("hello".into())],
        }]);
        let glyph_runs = scene
            .ops
            .iter()
            .filter(|op| matches!(op, netrender::SceneOp::GlyphRun(_)))
            .count();
        assert!(glyph_runs >= 1, "expected at least one GlyphRun op");
    }

    #[test]
    fn viewport_rounds_to_u32() {
        let laid = layout_document(
            &doc(vec![]),
            Viewport::new(640.4, 480.6),
            &DocumentStyleSheet::default(),
        );
        let scene = scene_from_packet(&laid.packet, &laid.fonts, &ColorVocabulary::default());
        assert_eq!(scene.viewport_width, 640);
        assert_eq!(scene.viewport_height, 481);
    }
}
