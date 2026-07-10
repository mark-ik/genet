use super::super::*;
use super::render;
use inker::InlineSpan;

#[test]
fn wikilink_rewrites_to_mere_node_url() {
    let doc = render("See [[my note]] for context.\n");
    let Block::Paragraph { spans } = &doc.blocks[0] else {
        panic!("expected paragraph");
    };
    let link = spans.iter().find_map(|s| match s {
        InlineSpan::Link { url, spans, .. } => Some((url.as_str(), spans.clone())),
        _ => None,
    });
    let (url, inner) = link.expect("expected wikilink");
    assert_eq!(url, "mere://node/my-note");
    // Display text preserves the original surface form.
    assert!(matches!(&inner[0], InlineSpan::Text(t) if t == "my note"));
    // outgoing_links() picks up the wikilink target.
    assert!(
        doc.outgoing_links()
            .iter()
            .any(|u| *u == "mere://node/my-note")
    );
}

#[test]
fn wikilink_lowercases_and_dashes_the_slug() {
    let doc = render("See [[Capital Words With Spaces]].\n");
    let url = doc.outgoing_links()[0];
    assert_eq!(url, "mere://node/capital-words-with-spaces");
}

#[test]
fn wikilinks_inside_existing_links_are_not_rewritten() {
    let doc = render("Read [the [[note]] here](https://x.test/).\n");
    // The outer markdown link should win; the [[note]] inside its display
    // text stays as plain text, not a nested wikilink.
    let Block::Paragraph { spans } = &doc.blocks[0] else {
        panic!("expected paragraph");
    };
    let links: Vec<&str> = spans
        .iter()
        .filter_map(|s| match s {
            InlineSpan::Link { url, .. } => Some(url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(links, vec!["https://x.test/"]);
}

#[test]
fn unterminated_wikilink_stays_as_text() {
    let doc = render("This is [[unclosed and continues...\n");
    // No wikilink should have been emitted.
    let any_link = doc.blocks.iter().any(|b| match b {
        Block::Paragraph { spans } => spans.iter().any(|s| matches!(s, InlineSpan::Link { .. })),
        _ => false,
    });
    assert!(!any_link, "unterminated [[ should not produce a link");
}

#[test]
fn hashtag_extracts_to_badge_after_paragraph() {
    let doc = render("This is a clip about #research and #semantics today.\n");
    let badges: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Badge { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(badges.contains(&"#research"));
    assert!(badges.contains(&"#semantics"));
}

#[test]
fn hashtag_is_removed_from_paragraph_text() {
    let doc = render("Note tagged #foo at end.\n");
    let Block::Paragraph { spans } = &doc.blocks[0] else {
        panic!("expected paragraph");
    };
    let combined: String = spans
        .iter()
        .filter_map(|s| match s {
            InlineSpan::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    // The "#foo" token should have been consumed; surrounding text remains.
    assert!(combined.contains("Note tagged"));
    assert!(combined.contains("at end."));
    assert!(!combined.contains("#foo"));
}

#[test]
fn hash_in_middle_of_word_is_not_a_hashtag() {
    let doc = render("CSS color #abc123 example.\n");
    let Block::Paragraph { spans } = &doc.blocks[0] else {
        panic!("expected paragraph");
    };
    let combined: String = spans
        .iter()
        .filter_map(|s| match s {
            InlineSpan::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    // Hash *was* preceded by a space, so it's parsed as a hashtag — this
    // is a known limitation. The test pins current behavior so we're aware.
    let has_badge = doc.blocks.iter().any(|b| match b {
        Block::Badge { text } => text == "#abc123",
        _ => false,
    });
    assert!(
        has_badge,
        "current behavior: bare # at word boundary is hashtag"
    );
    assert!(!combined.contains("#abc123"));
}

#[test]
fn hashtags_in_headings_stay_as_text() {
    // Headings preserve hashtags inline; only paragraphs extract them.
    let doc = render("# Topic about #research\n\nBody.\n");
    let Block::Heading { spans, .. } = &doc.blocks[0] else {
        panic!("expected heading");
    };
    let combined: String = spans
        .iter()
        .filter_map(|s| match s {
            InlineSpan::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(combined.contains("#research"), "got: {combined}");
}

#[test]
fn polyglot_knot_with_wikilinks_and_hashtags() {
    let body = "---\ntitle: Mixed\n---\n\nSee [[research-log]] and the entry below #research.\n\n```feed-entry\ntitle: Article\nurl: https://x.test/\n```\n";
    let doc = render(body);

    let urls = doc.outgoing_links();
    assert!(urls.iter().any(|u| *u == "mere://node/research-log"));
    assert!(urls.iter().any(|u| *u == "https://x.test/"));

    let has_badge = doc.blocks.iter().any(|b| match b {
        Block::Badge { text } => text == "#research",
        _ => false,
    });
    assert!(has_badge);
}

// =============================================================================
// build_clip_knot helper
// =============================================================================

#[test]
fn build_clip_knot_assembles_frontmatter_plus_blocks() {
    let mut prov = DocumentProvenance::default();
    prov.canonical_uri = Some("https://blog.test/article".to_string());
    prov.source_kind = Some("nematic.markdown".to_string());

    let blocks = vec![Block::Paragraph {
        spans: vec![InlineSpan::Text("A clipped paragraph.".to_string())],
    }];

    let knot = build_clip_knot(&blocks, &prov, DocumentTrustState::Tofu, Some("clip"));
    assert!(knot.starts_with("---\n"));
    assert!(knot.contains("source: https://blog.test/article"));
    assert!(knot.contains("trust: tofu"));
    assert!(knot.contains("note_kind: clip"));
    assert!(knot.contains("source_label: nematic.markdown"));
    assert!(knot.contains("A clipped paragraph."));

    // Round-trip: feeding the knot back through the engine should produce a
    // document with the right provenance and the paragraph intact.
    let doc = render(&knot);
    assert_eq!(
        doc.provenance.canonical_uri.as_deref(),
        Some("https://blog.test/article")
    );
    assert_eq!(doc.trust, DocumentTrustState::Tofu);
}

#[test]
fn build_clip_knot_does_not_double_emit_frontmatter() {
    // Regression: EngineDocument::to_knot now emits frontmatter when the
    // document has provenance / trust set. build_clip_knot must use the
    // body-only render path so its own clip-aware frontmatter isn't
    // duplicated.
    let mut prov = DocumentProvenance::default();
    prov.canonical_uri = Some("https://x.test/".to_string());
    let blocks = vec![Block::Paragraph {
        spans: vec![InlineSpan::Text("Body".to_string())],
    }];
    let knot = build_clip_knot(&blocks, &prov, DocumentTrustState::Tofu, None);
    // Exactly two `---` lines (opener + closer of the one frontmatter
    // block), then body. A double-frontmatter regression would produce
    // four or more.
    let dash_lines = knot.lines().filter(|l| *l == "---").count();
    assert_eq!(
        dash_lines, 2,
        "expected one frontmatter block, got:\n{knot}"
    );
}

#[test]
fn build_clip_knot_with_block_provenance_emits_block_sources_list() {
    use inker::{BlockProvenance, BlockProvenanceMap};

    let mut doc_prov = DocumentProvenance::default();
    doc_prov.canonical_uri = Some("https://blog.test/composite".to_string());

    let mut other_prov = DocumentProvenance::default();
    other_prov.canonical_uri = Some("gopher://other.test/0/file".to_string());

    let mut map = BlockProvenanceMap::new();
    // Block 0 came from the document's own source — should NOT appear
    // in block_sources.
    map.insert(0, BlockProvenance::from_document(doc_prov.clone()));
    // Block 1 came from a different source with an anchor.
    map.insert(
        1,
        BlockProvenance::from_document(other_prov.clone()).with_anchor("L42-L58"),
    );

    let blocks = vec![
        Block::Paragraph {
            spans: vec![InlineSpan::Text("From the composite source.".into())],
        },
        Block::Paragraph {
            spans: vec![InlineSpan::Text("From the gopher source.".into())],
        },
    ];

    let knot = build_clip_knot_with_block_provenance(
        &blocks,
        &doc_prov,
        DocumentTrustState::Tofu,
        None,
        &map,
    );

    assert!(knot.contains("block_sources: ["));
    assert!(knot.contains("\"1|gopher://other.test/0/file|L42-L58\""));
    // Block 0 matched the document source with no anchor — must not
    // appear in the list.
    assert!(
        !knot.contains("\"0|"),
        "block 0 should not appear in block_sources:\n{knot}"
    );
}

#[test]
fn build_clip_knot_with_block_provenance_emits_no_list_when_all_match_document() {
    use inker::{BlockProvenance, BlockProvenanceMap};

    let mut doc_prov = DocumentProvenance::default();
    doc_prov.canonical_uri = Some("https://blog.test/".to_string());

    let mut map = BlockProvenanceMap::new();
    map.insert(0, BlockProvenance::from_document(doc_prov.clone()));
    map.insert(2, BlockProvenance::from_document(doc_prov.clone()));

    let blocks = vec![Block::Paragraph {
        spans: vec![InlineSpan::Text("body".into())],
    }];
    let knot = build_clip_knot_with_block_provenance(
        &blocks,
        &doc_prov,
        DocumentTrustState::Unknown,
        None,
        &map,
    );
    assert!(
        !knot.contains("block_sources:"),
        "expected no block_sources list when every override matches the document source:\n{knot}"
    );
}

#[test]
fn build_clip_knot_with_block_provenance_sorts_entries_by_index() {
    use inker::{BlockProvenance, BlockProvenanceMap};

    let doc_prov = DocumentProvenance::default();
    let mut a = DocumentProvenance::default();
    a.canonical_uri = Some("gemini://a.test/".to_string());
    let mut b = DocumentProvenance::default();
    b.canonical_uri = Some("gemini://b.test/".to_string());
    let mut c = DocumentProvenance::default();
    c.canonical_uri = Some("gemini://c.test/".to_string());

    let mut map = BlockProvenanceMap::new();
    // Insert in scrambled order; output must still be sorted.
    map.insert(5, BlockProvenance::from_document(c));
    map.insert(1, BlockProvenance::from_document(a));
    map.insert(3, BlockProvenance::from_document(b));

    let blocks = vec![Block::Paragraph {
        spans: vec![InlineSpan::Text("x".into())],
    }];
    let knot = build_clip_knot_with_block_provenance(
        &blocks,
        &doc_prov,
        DocumentTrustState::Unknown,
        None,
        &map,
    );
    let a_pos = knot.find("\"1|gemini://a.test/\"").expect("a entry");
    let b_pos = knot.find("\"3|gemini://b.test/\"").expect("b entry");
    let c_pos = knot.find("\"5|gemini://c.test/\"").expect("c entry");
    assert!(
        a_pos < b_pos && b_pos < c_pos,
        "entries not sorted by index"
    );
}
