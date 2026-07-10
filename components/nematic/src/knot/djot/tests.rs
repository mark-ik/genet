/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Tests for the djot knot parser / serializer (design doc §10).

use super::*;

/// Extract the first [`InlineSpan::Link`] in the first paragraph of `body` as
/// `(url, predicate, display-text)`.
fn first_link(body: &str) -> (String, Option<String>, String) {
    let blocks = parse_djot_knot_body(body);
    let spans = blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph { spans } => Some(spans.clone()),
            _ => None,
        })
        .expect("expected a paragraph");
    spans
        .iter()
        .find_map(|s| match s {
            InlineSpan::Link {
                url,
                predicate,
                spans,
                ..
            } => Some((url.clone(), predicate.clone(), inline_text(spans))),
            _ => None,
        })
        .expect("expected a link span")
}

#[test]
fn inline_link_captures_destination_and_rel() {
    let (url, predicate, display) =
        first_link("[Topic](mere://node/topic){rel=\"schema:cites\"}\n");
    assert_eq!(url, "mere://node/topic");
    assert_eq!(predicate.as_deref(), Some("schema:cites"));
    assert_eq!(display, "Topic");
}

#[test]
fn plain_inline_link_has_no_predicate() {
    let (url, predicate, display) = first_link("[docs](https://x.test/)\n");
    assert_eq!(url, "https://x.test/");
    assert_eq!(predicate, None);
    assert_eq!(display, "docs");
}

#[test]
fn inline_link_sits_between_text_runs() {
    let blocks = parse_djot_knot_body("see [docs](https://x.test/){rel=\"schema:cites\"} now\n");
    let Block::Paragraph { spans } = blocks.first().expect("paragraph") else {
        panic!("expected paragraph, got {blocks:?}");
    };
    // Text-before, the link span, text-after.
    assert!(matches!(spans.first(), Some(InlineSpan::Text(t)) if t == "see "));
    assert!(matches!(spans.last(), Some(InlineSpan::Text(t)) if t == " now"));
    let link = spans.iter().find_map(|s| match s {
        InlineSpan::Link { url, predicate, .. } => Some((url.as_str(), predicate.as_deref())),
        _ => None,
    });
    assert_eq!(link, Some(("https://x.test/", Some("schema:cites"))));
}

#[test]
fn unknown_div_class_renders_generically_with_a_diagnostic() {
    let (blocks, diagnostics) = parse_djot_knot_body_validated("::: mystery\nx\n:::\n");
    assert!(matches!(blocks.first(), Some(Block::Quote { .. })));
    assert!(matches!(
        diagnostics.first(),
        Some(DocumentDiagnostic::UnsupportedConstruct(m)) if m.contains("mystery")
    ));
}

#[test]
fn feed_entry_missing_required_title_warns() {
    let (blocks, diagnostics) = parse_djot_knot_body_validated("::: feed-entry\nbody\n:::\n");
    assert!(matches!(blocks.first(), Some(Block::FeedEntry { .. })));
    assert!(matches!(
        diagnostics.first(),
        Some(DocumentDiagnostic::ParseWarning(m)) if m.contains("title")
    ));
}

#[test]
fn recognized_div_with_required_attrs_has_no_diagnostics() {
    let (_, diagnostics) =
        parse_djot_knot_body_validated("{title=\"Article\"}\n::: feed-entry\nbody\n:::\n");
    assert!(diagnostics.is_empty());
}

#[test]
fn djot_engine_expands_protocol_fences_for_parity() {
    let input = EngineInput {
        address: "knot:test".to_string(),
        body: "---\ntitle: T\n---\n\n```gemtext\n=> gemini://x/ a link\n```\n".to_string(),
        content_type: None,
    };
    let doc = DjotKnotEngine::new().render(&input).unwrap();
    // The gemtext fence expanded into real blocks (the gemini link), so no
    // raw gemtext code block remains — parity with the CommonMark knot.
    assert!(!doc.blocks.iter().any(|b| matches!(
        b,
        Block::CodeBlock { language, .. } if language.as_deref() == Some("gemtext")
    )));
    assert!(
        doc.outgoing_links()
            .iter()
            .any(|u| u.contains("gemini://x/"))
    );
}

#[test]
fn round_trip_preserves_semantic_blocks() {
    let source = concat!(
        "# My research\n\n",
        "A note about things.\n\n",
        "{title=\"Article\" url=\"https://blog.test/post\" date=\"2026-05-08\"}\n",
        "::: feed-entry\nA summary.\n:::\n\n",
        "::: badge\nresearch\n:::\n\n",
        ": Trust\n\n  tofu\n",
    );
    let blocks = parse_djot_knot_body(source);
    let reparsed = parse_djot_knot_body(&blocks_to_djot(&blocks));
    assert_eq!(blocks, reparsed);
}

#[test]
fn djot_engine_renders_frontmatter_and_body() {
    let input = EngineInput {
        address: "knot:test".to_string(),
        body: "---\ntitle: My Note\ntrust: tofu\nnote_kind: clip\n---\n\n# Heading\n\nBody.\n"
            .to_string(),
        content_type: None,
    };
    let doc = DjotKnotEngine::new().render(&input).unwrap();
    assert_eq!(doc.title.as_deref(), Some("My Note"));
    assert_eq!(doc.trust, DocumentTrustState::Tofu);
    assert_eq!(doc.content_type, "text/x-knot");
    assert_eq!(doc.provenance.source_kind.as_deref(), Some(ENGINE_ID));
    // `note_kind` prefixes a MetadataRow; the djot body follows.
    assert!(matches!(
        doc.blocks.first(),
        Some(Block::MetadataRow { label, value }) if label == "kind" && value == "clip"
    ));
    assert!(
        doc.blocks
            .iter()
            .any(|b| matches!(b, Block::Heading { .. }))
    );
}

#[test]
fn description_list_becomes_metadata_rows() {
    // Djot description-list syntax: `: term` then an indented definition.
    let blocks = parse_djot_knot_body(": Trust\n\n  tofu\n\n: Login\n\n  alice\n");
    assert_eq!(
        blocks,
        vec![
            Block::MetadataRow {
                label: "Trust".into(),
                value: "tofu".into(),
            },
            Block::MetadataRow {
                label: "Login".into(),
                value: "alice".into(),
            },
        ]
    );
}

#[test]
fn heading_and_paragraph_map_structurally() {
    let blocks = parse_djot_knot_body("# My research\n\nA note about things.\n");
    assert_eq!(
        blocks,
        vec![
            Block::Heading {
                level: 1,
                spans: vec![InlineSpan::Text("My research".into())],
            },
            Block::Paragraph {
                spans: vec![InlineSpan::Text("A note about things.".into())],
            },
        ]
    );
}

#[test]
fn badge_div_becomes_badge() {
    let blocks = parse_djot_knot_body("::: badge\nresearch\n:::\n");
    assert_eq!(
        blocks,
        vec![Block::Badge {
            text: "research".into()
        }]
    );
}

#[test]
fn feed_entry_div_reads_attributes() {
    let body = "{title=\"Article\" url=\"https://blog.test/post\" date=\"2026-05-08\"}\n::: feed-entry\nA summary.\n:::\n";
    let blocks = parse_djot_knot_body(body);
    assert_eq!(
        blocks,
        vec![Block::FeedEntry {
            title: "Article".into(),
            date: Some("2026-05-08".into()),
            summary: Some("A summary.".into()),
            article_url: Some("https://blog.test/post".into()),
            source_url: None,
        }]
    );
}
