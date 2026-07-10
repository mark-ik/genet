/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::super::{Block, DocumentProvenance, DocumentTrustState, EngineDocument, InlineSpan};

fn doc(blocks: Vec<Block>) -> EngineDocument {
    EngineDocument {
        address: "doc:1".into(),
        title: None,
        content_type: "text/plain".into(),
        lang: None,
        provenance: DocumentProvenance::default(),
        trust: DocumentTrustState::Unknown,
        diagnostics: Vec::new(),
        blocks,
    }
}

#[test]
fn to_markdown_renders_heading_paragraph_and_link() {
    let document = doc(vec![
        Block::Heading {
            level: 1,
            spans: vec![InlineSpan::Text("Hello".into())],
        },
        Block::Paragraph {
            spans: vec![
                InlineSpan::Text("see ".into()),
                InlineSpan::Link {
                    url: "https://x.test/".into(),
                    title: None,
                    spans: vec![InlineSpan::Text("docs".into())],
                    predicate: None,
                },
            ],
        },
    ]);
    let md = document.to_markdown();
    assert!(md.contains("# Hello"));
    assert!(md.contains("[docs](https://x.test/)"));
}

#[test]
fn to_gemini_renders_paragraph_with_link_lines() {
    let document = doc(vec![Block::Paragraph {
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
    }]);
    let gem = document.to_gemini();
    assert!(gem.contains("see docs please\n"));
    assert!(gem.contains("=> https://x.test/ docs\n"));
}

#[test]
fn to_markdown_renders_feed_entry_as_h2_block() {
    let document = doc(vec![Block::FeedEntry {
        title: "Title".into(),
        date: Some("2026-05-08".into()),
        summary: Some("Summary text.".into()),
        article_url: Some("https://feed.test/x".into()),
        source_url: None,
    }]);
    let md = document.to_markdown();
    assert!(md.contains("## Title"));
    assert!(md.contains("*2026-05-08*"));
    assert!(md.contains("Summary text."));
    assert!(md.contains("[Open article](https://feed.test/x)"));
}

#[test]
fn to_gemini_renders_metadata_row_as_label_value() {
    let document = doc(vec![Block::MetadataRow {
        label: "Login".into(),
        value: "alice".into(),
    }]);
    assert_eq!(document.to_gemini(), "Login: alice\n");
}

// -----------------------------------------------------------------
// to_knot frontmatter round-trip
// -----------------------------------------------------------------

fn doc_with_metadata(
    title: Option<&str>,
    provenance: DocumentProvenance,
    trust: DocumentTrustState,
) -> EngineDocument {
    EngineDocument {
        address: "doc:1".into(),
        title: title.map(String::from),
        content_type: "text/x-knot".into(),
        lang: None,
        provenance,
        trust,
        diagnostics: Vec::new(),
        blocks: vec![Block::Paragraph {
            spans: vec![InlineSpan::Text("Body.".into())],
        }],
    }
}

#[test]
fn to_knot_omits_frontmatter_when_no_metadata() {
    let document = doc(vec![Block::Paragraph {
        spans: vec![InlineSpan::Text("Just body.".into())],
    }]);
    let knot = document.to_knot();
    assert!(
        !knot.starts_with("---"),
        "expected no frontmatter; got: {knot:?}"
    );
}

#[test]
fn to_knot_emits_title_in_frontmatter() {
    let document = doc_with_metadata(
        Some("My Title"),
        DocumentProvenance::default(),
        DocumentTrustState::Unknown,
    );
    let knot = document.to_knot();
    assert!(knot.starts_with("---\n"));
    assert!(knot.contains("title: My Title"));
    assert!(knot.contains("Body."));
}

#[test]
fn to_knot_emits_provenance_fields_in_frontmatter() {
    let provenance = DocumentProvenance {
        source_kind: Some("nematic.knot".into()),
        canonical_uri: Some("https://example.test/article".into()),
        fetched_at: Some("2026-05-10T14:23:00Z".into()),
        source_label: Some("Example Blog".into()),
    };
    let document = doc_with_metadata(None, provenance, DocumentTrustState::Tofu);
    let knot = document.to_knot();
    assert!(knot.contains("source: https://example.test/article"));
    assert!(knot.contains("captured: 2026-05-10T14:23:00Z"));
    assert!(knot.contains("source_label: Example Blog"));
    assert!(knot.contains("trust: tofu"));
}

#[test]
fn to_knot_omits_trust_when_unknown() {
    let document = doc_with_metadata(
        Some("Title"),
        DocumentProvenance::default(),
        DocumentTrustState::Unknown,
    );
    let knot = document.to_knot();
    assert!(!knot.contains("trust:"));
}

#[test]
fn to_knot_emits_each_trust_state_correctly() {
    for (state, expected) in [
        (DocumentTrustState::Trusted, "trust: trusted"),
        (DocumentTrustState::Tofu, "trust: tofu"),
        (DocumentTrustState::Insecure, "trust: insecure"),
        (DocumentTrustState::Broken, "trust: broken"),
    ] {
        let document = doc_with_metadata(None, DocumentProvenance::default(), state);
        assert!(document.to_knot().contains(expected));
    }
}

// ── export.rs (gophermap + plain text) ──────────────────────────────────────

use super::export::GophermapContext;

fn ctx() -> GophermapContext {
    GophermapContext {
        host: "gopher.example".into(),
        port: 70,
    }
}

#[test]
fn to_gophermap_renders_info_lines_links_and_terminator() {
    let document = doc(vec![
        Block::Heading {
            level: 1,
            spans: vec![InlineSpan::Text("Notes".into())],
        },
        Block::Paragraph {
            spans: vec![
                InlineSpan::Text("see ".into()),
                InlineSpan::Link {
                    url: "https://x.test/page".into(),
                    title: None,
                    spans: vec![InlineSpan::Text("docs".into())],
                    predicate: None,
                },
            ],
        },
    ]);
    let map = document.to_gophermap(&ctx());
    assert!(
        map.contains("iNotes\tfake\t(NULL)\t0\r\n"),
        "heading is an info line: {map}"
    );
    assert!(
        map.contains("isee docs\tfake\t(NULL)\t0\r\n"),
        "paragraph text flattens: {map}"
    );
    assert!(
        map.contains("hdocs\tURL:https://x.test/page\tgopher.example\t70\r\n"),
        "non-gopher link uses the URL: form on the serving host: {map}"
    );
    assert!(map.ends_with(".\r\n"), "terminator line: {map:?}");
}

#[test]
fn to_gophermap_decomposes_native_gopher_links() {
    let document = doc(vec![Block::Paragraph {
        spans: vec![InlineSpan::Link {
            url: "gopher://floodgap.com/1/gopher".into(),
            title: None,
            spans: vec![InlineSpan::Text("floodgap".into())],
            predicate: None,
        }],
    }]);
    let map = document.to_gophermap(&ctx());
    assert!(
        map.contains("1floodgap\t/gopher\tfloodgap.com\t70\r\n"),
        "gopher links become native menu entries: {map}"
    );
}

#[test]
fn to_text_flattens_structure_readably() {
    let document = doc(vec![
        Block::Heading {
            level: 2,
            spans: vec![InlineSpan::Text("Reading".into())],
        },
        Block::Paragraph {
            spans: vec![
                InlineSpan::Text("see ".into()),
                InlineSpan::Link {
                    url: "https://x.test/".into(),
                    title: None,
                    spans: vec![InlineSpan::Text("docs".into())],
                    predicate: None,
                },
            ],
        },
        Block::List {
            ordered: true,
            items: vec![
                vec![Block::Paragraph {
                    spans: vec![InlineSpan::Text("first".into())],
                }],
                vec![Block::Paragraph {
                    spans: vec![InlineSpan::Text("second".into())],
                }],
            ],
        },
        Block::Quote {
            blocks: vec![Block::Paragraph {
                spans: vec![InlineSpan::Text("quoted".into())],
            }],
        },
    ]);
    let text = document.to_text();
    assert!(text.contains("Reading\n\n"));
    assert!(text.contains("see docs <https://x.test/>"));
    assert!(text.contains("1. first\n"));
    assert!(text.contains("2. second\n"));
    assert!(text.contains("> quoted"));
}
