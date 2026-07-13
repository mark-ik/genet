/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;
use crate::types::InteractionKind;
use inker::{Block, DocumentProvenance, DocumentTrustState, EngineDocument, InlineSpan};

fn doc(blocks: Vec<Block>) -> EngineDocument {
    EngineDocument {
        address: "doc:test".into(),
        title: None,
        content_type: "text/plain".into(),
        lang: None,
        provenance: DocumentProvenance::default(),
        trust: DocumentTrustState::Unknown,
        diagnostics: Vec::new(),
        blocks,
    }
}

fn viewport() -> Viewport {
    Viewport::new(640.0, 480.0)
}

#[test]
fn empty_document_lays_out_to_empty_block_list() {
    let packet = layout_document(&doc(vec![]), viewport(), &DocumentStyleSheet::default()).packet;
    assert!(packet.blocks.is_empty());
    assert!(packet.interactions.is_empty());
    assert_eq!(packet.viewport.width, 640.0);
}

#[test]
fn single_paragraph_produces_one_text_block() {
    let packet = layout_document(
        &doc(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text("Hello, world.".into())],
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    assert_eq!(packet.blocks.len(), 1);
    let block = &packet.blocks[0];
    assert_eq!(block.source_block_index, 0);
    let RenderedBlockKind::Text { glyph_runs } = &block.kind else {
        panic!("expected Text kind, got {:?}", block.kind);
    };
    assert!(!glyph_runs.is_empty(), "expected at least one glyph run");
}

#[test]
fn heading_is_taller_than_paragraph() {
    let style = DocumentStyleSheet::default();
    let packet = layout_document(
        &doc(vec![
            Block::Heading {
                level: 1,
                spans: vec![InlineSpan::Text("Title".into())],
            },
            Block::Paragraph {
                spans: vec![InlineSpan::Text("Body.".into())],
            },
        ]),
        viewport(),
        &style,
    )
    .packet;
    assert_eq!(packet.blocks.len(), 2);
    let heading = &packet.blocks[0];
    let paragraph = &packet.blocks[1];
    assert!(
        heading.bounds.size.height > paragraph.bounds.size.height,
        "heading {:?} should be taller than paragraph {:?}",
        heading.bounds,
        paragraph.bounds
    );
}

#[test]
fn paragraph_with_link_emits_interaction_region() {
    let packet = layout_document(
        &doc(vec![Block::Paragraph {
            spans: vec![
                InlineSpan::Text("see ".into()),
                InlineSpan::Link {
                    url: "https://x.test/".into(),
                    title: None,
                    spans: vec![InlineSpan::Text("docs".into())],
                    predicate: None,
                },
                InlineSpan::Text(" please".into()),
            ],
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    assert_eq!(packet.interactions.len(), 1);
    let region = &packet.interactions[0];
    match &region.kind {
        InteractionKind::Link { url } => assert_eq!(url, "https://x.test/"),
    }
    assert!(region.bounds.size.width > 0.0);
    assert!(region.bounds.size.height > 0.0);
}

#[test]
fn list_emits_group_block_with_children() {
    let packet = layout_document(
        &doc(vec![Block::List {
            ordered: false,
            items: vec![
                vec![Block::Paragraph {
                    spans: vec![InlineSpan::Text("first".into())],
                }],
                vec![Block::Paragraph {
                    spans: vec![InlineSpan::Text("second".into())],
                }],
            ],
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    let RenderedBlockKind::Group { children } = &packet.blocks[0].kind else {
        panic!("expected Group kind");
    };
    assert_eq!(children.len(), 2);
}

#[test]
fn quote_emits_group_block_with_indented_children() {
    let style = DocumentStyleSheet::default();
    let packet = layout_document(
        &doc(vec![Block::Quote {
            blocks: vec![Block::Paragraph {
                spans: vec![InlineSpan::Text("quoted text".into())],
            }],
        }]),
        viewport(),
        &style,
    )
    .packet;
    let RenderedBlockKind::Group { children } = &packet.blocks[0].kind else {
        panic!("expected Group kind");
    };
    assert_eq!(children.len(), 1);
    // Indent should push the child's left edge inward.
    assert!(
        children[0].bounds.origin.x >= style.horizontal_padding + style.indent_per_level,
        "quote child should be indented; got x={}",
        children[0].bounds.origin.x
    );
}

#[test]
fn rule_emits_rule_block() {
    let packet = layout_document(
        &doc(vec![Block::Rule]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    assert!(matches!(packet.blocks[0].kind, RenderedBlockKind::Rule));
}

#[test]
fn image_emits_image_block_with_url_and_alt() {
    let packet = layout_document(
        &doc(vec![Block::Image {
            url: "https://x.test/pic.png".into(),
            alt: "a picture".into(),
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    let RenderedBlockKind::Image { url, alt } = &packet.blocks[0].kind else {
        panic!("expected Image kind");
    };
    assert_eq!(url, "https://x.test/pic.png");
    assert_eq!(alt, "a picture");
}

#[test]
fn feed_entry_composes_into_group_with_h2_summary_link() {
    let packet = layout_document(
        &doc(vec![Block::FeedEntry {
            title: "Article".into(),
            date: Some("2026-05-09".into()),
            summary: Some("Summary text.".into()),
            article_url: Some("https://feed.test/x".into()),
            source_url: None,
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    let RenderedBlockKind::Group { children } = &packet.blocks[0].kind else {
        panic!("expected Group kind");
    };
    // Heading + date + summary + article link = 4 children.
    assert_eq!(children.len(), 4);

    // Article URL surfaces as an interaction region.
    assert!(
        packet.interactions.iter().any(
            |r| matches!(&r.kind, InteractionKind::Link { url } if url == "https://feed.test/x")
        )
    );
}

#[test]
fn metadata_row_lays_out_label_and_value() {
    let packet = layout_document(
        &doc(vec![Block::MetadataRow {
            label: "Login".into(),
            value: "alice".into(),
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    assert_eq!(packet.blocks.len(), 1);
    let RenderedBlockKind::Text { glyph_runs } = &packet.blocks[0].kind else {
        panic!("expected Text kind");
    };
    assert!(!glyph_runs.is_empty());
}

#[test]
fn content_bounds_grow_with_blocks() {
    let style = DocumentStyleSheet::default();
    let single = layout_document(
        &doc(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text("one".into())],
        }]),
        viewport(),
        &style,
    )
    .packet;
    let several = layout_document(
        &doc(vec![
            Block::Paragraph {
                spans: vec![InlineSpan::Text("one".into())],
            },
            Block::Paragraph {
                spans: vec![InlineSpan::Text("two".into())],
            },
            Block::Paragraph {
                spans: vec![InlineSpan::Text("three".into())],
            },
        ]),
        viewport(),
        &style,
    )
    .packet;
    assert!(several.content_bounds.size.height > single.content_bounds.size.height);
}

#[test]
fn document_dedups_shared_face() {
    // Two body paragraphs shape against the same family/weight, so
    // parley returns the same face for both runs → one entry in the
    // sidecar, and both runs carry the same FontFaceId.
    let laid = layout_document(
        &doc(vec![
            Block::Paragraph {
                spans: vec![InlineSpan::Text("first".into())],
            },
            Block::Paragraph {
                spans: vec![InlineSpan::Text("second".into())],
            },
        ]),
        viewport(),
        &DocumentStyleSheet::default(),
    );
    assert_eq!(laid.fonts.len(), 1, "shared body face should intern once");
    let faces: Vec<_> = laid
        .packet
        .blocks
        .iter()
        .filter_map(|b| match &b.kind {
            RenderedBlockKind::Text { glyph_runs } => glyph_runs.first(),
            _ => None,
        })
        .map(|r| r.font_face)
        .collect();
    assert!(faces.len() >= 2, "expected a run per paragraph");
    assert!(
        faces.iter().all(|f| *f == faces[0]),
        "runs share one face id"
    );
}

#[test]
fn text_populates_font_sidecar() {
    // Mirrors genet's `emit_with_layouts_populates_font_table`: real
    // text yields a non-empty sidecar, every run's face resolves in
    // it, and the resolved face carries real bytes.
    let laid = layout_document(
        &doc(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text("hello".into())],
        }]),
        viewport(),
        &DocumentStyleSheet::default(),
    );
    assert!(
        !laid.fonts.is_empty(),
        "text should populate the font sidecar"
    );
    for block in &laid.packet.blocks {
        if let RenderedBlockKind::Text { glyph_runs } = &block.kind {
            for run in glyph_runs {
                let face = laid
                    .fonts
                    .get(run.font_face)
                    .expect("run face resolves in sidecar");
                assert!(!face.data.data().is_empty(), "face carries real bytes");
            }
        }
    }
}

#[test]
fn nowrap_role_overflows_instead_of_wrapping() {
    let long = "a long single line of body text that would otherwise wrap across many lines";
    let make = || {
        doc(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text(long.into())],
        }])
    };
    let narrow = Viewport::new(160.0, 800.0);

    // Wrap (default): the block is constrained to the content width and wraps tall.
    let wrapped = layout_document(&make(), narrow, &DocumentStyleSheet::default()).packet;

    // NoWrap: the body role lays out on its natural width, overflowing on one line.
    let mut sheet = DocumentStyleSheet::default();
    sheet.roles.body.wrap = crate::WrapPolicy::NoWrap;
    let unwrapped = layout_document(&make(), narrow, &sheet).packet;

    let wrapped_b = &wrapped.blocks[0].bounds.size;
    let unwrapped_b = &unwrapped.blocks[0].bounds.size;
    assert!(
        unwrapped_b.width > wrapped_b.width,
        "NoWrap block should be wider than Wrap: {} vs {}",
        unwrapped_b.width,
        wrapped_b.width
    );
    assert!(
        unwrapped_b.width > narrow.width,
        "NoWrap block overflows the viewport width for the host to scroll: {} vs {}",
        unwrapped_b.width,
        narrow.width
    );
    assert!(
        unwrapped_b.height < wrapped_b.height,
        "NoWrap is one line, so shorter than the wrapped block"
    );
}

#[test]
fn glyph_runs_carry_per_role_colors() {
    // Each run is colored by its block / inline role: a heading in
    // heading_text, body in body_text, a link in link_text, inline code in
    // code_text. parley segments the paragraph into separate runs at the
    // brush boundaries, so the link + code sub-runs get their own color.
    let palette = DocumentStyleSheet::default().colors;
    let packet = layout_document(
        &doc(vec![
            Block::Heading {
                level: 1,
                spans: vec![InlineSpan::Text("Title".into())],
            },
            Block::Paragraph {
                spans: vec![
                    InlineSpan::Text("see ".into()),
                    InlineSpan::Link {
                        url: "https://x.test/".into(),
                        title: None,
                        spans: vec![InlineSpan::Text("link".into())],
                        predicate: None,
                    },
                    InlineSpan::Text(" or ".into()),
                    InlineSpan::Code("snippet".into()),
                ],
            },
        ]),
        viewport(),
        &DocumentStyleSheet::default(),
    )
    .packet;
    let colors: Vec<[f32; 4]> = packet
        .blocks
        .iter()
        .filter_map(|b| match &b.kind {
            RenderedBlockKind::Text { glyph_runs } => Some(glyph_runs.iter().map(|r| r.color)),
            _ => None,
        })
        .flatten()
        .collect();
    assert!(
        colors.contains(&palette.heading_text),
        "heading run uses heading_text"
    );
    assert!(
        colors.contains(&palette.body_text),
        "body run uses body_text"
    );
    assert!(
        colors.contains(&palette.link_text),
        "link run uses link_text"
    );
    assert!(
        colors.contains(&palette.code_text),
        "inline code run uses code_text"
    );
}
