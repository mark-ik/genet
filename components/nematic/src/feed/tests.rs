/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

fn render(body: &str) -> EngineDocument {
    FeedEngine::new()
        .render(&EngineInput::new("feed:test", body))
        .expect("render")
}

const RSS_SAMPLE: &str = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Example Feed</title>
    <link>https://example.test/</link>
    <description>Example feed description</description>
    <language>en-US</language>
    <item>
      <title>First post</title>
      <link>https://example.test/first</link>
      <description>This is &lt;b&gt;the first&lt;/b&gt; post.</description>
    </item>
    <item>
      <title>Second post</title>
      <link>https://example.test/second</link>
      <description>Plain second-post body.</description>
    </item>
  </channel>
</rss>"#;

const ATOM_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Example</title>
  <link href="https://example.test/" rel="alternate"/>
  <updated>2026-05-08T00:00:00Z</updated>
  <entry>
    <title>Atom one</title>
    <link href="https://example.test/atom-one" rel="alternate"/>
    <summary>First atom entry summary.</summary>
  </entry>
  <entry>
    <title>Atom two</title>
    <link href="https://example.test/atom-two" rel="alternate"/>
    <content type="html">&lt;p&gt;HTML content.&lt;/p&gt;</content>
  </entry>
</feed>"#;

const JSON_FEED_SAMPLE: &str = r#"{
  "version": "https://jsonfeed.org/version/1.1",
  "title": "JSON Example",
  "home_page_url": "https://example.test/",
  "description": "Example JSON feed description",
  "language": "en-US",
  "items": [
    {
      "id": "1",
      "url": "https://example.test/first",
      "title": "First post",
      "content_html": "This is <b>the first</b> post.",
      "date_published": "2026-05-08T00:00:00Z"
    },
    {
      "id": "2",
      "url": "https://example.test/second",
      "title": "Second post",
      "content_text": "Plain second-post body."
    }
  ]
}"#;

#[test]
fn engine_id_is_stable() {
    assert_eq!(FeedEngine::new().engine_id(), "nematic.feed");
}

#[test]
fn json_feed_extracts_title_lang_and_entries() {
    let doc = render(JSON_FEED_SAMPLE);
    assert_eq!(doc.title.as_deref(), Some("JSON Example"));
    assert_eq!(doc.lang.as_deref(), Some("en-US"));

    let urls = doc.outgoing_links();
    assert_eq!(
        urls,
        vec![
            "https://example.test/",
            "https://example.test/first",
            "https://example.test/second",
        ]
    );

    let entry_titles: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::FeedEntry { title, .. } => Some(title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(entry_titles, vec!["First post", "Second post"]);
}

#[test]
fn json_feed_emits_header_with_subtitle() {
    let doc = render(JSON_FEED_SAMPLE);
    let header = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedHeader {
                title,
                subtitle,
                source_url,
                ..
            } => Some((title.as_str(), subtitle.as_deref(), source_url.as_deref())),
            _ => None,
        })
        .expect("expected FeedHeader block");
    assert_eq!(header.0, "JSON Example");
    assert_eq!(header.1, Some("Example JSON feed description"));
    assert_eq!(header.2, Some("https://example.test/"));
}

#[test]
fn json_feed_content_html_is_stripped_and_flagged() {
    let doc = render(JSON_FEED_SAMPLE);
    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedEntry { title, summary, .. } if title == "First post" => summary.as_deref(),
            _ => None,
        })
        .expect("expected first-post summary");
    assert_eq!(summary, "This is the first post.");

    let degraded = doc
        .diagnostics
        .iter()
        .any(|d| matches!(d, DocumentDiagnostic::DegradedRendering(msg) if msg.contains("HTML")));
    assert!(
        degraded,
        "expected DegradedRendering diagnostic for stripped HTML"
    );
}

#[test]
fn json_feed_content_text_is_kept_verbatim() {
    let doc = render(JSON_FEED_SAMPLE);
    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedEntry { title, summary, .. } if title == "Second post" => summary.as_deref(),
            _ => None,
        })
        .expect("expected second-post summary");
    assert_eq!(summary, "Plain second-post body.");
}

#[test]
fn json_feed_detected_via_content_type_over_body_sniff() {
    // Declared JSON content type routes to the JSON path even though the
    // body sniff alone would also catch the leading `{`.
    let doc = FeedEngine::new()
        .render(
            &EngineInput::new("https://example.test/feed.json", JSON_FEED_SAMPLE)
                .with_content_type("application/feed+json"),
        )
        .expect("render");
    assert_eq!(doc.title.as_deref(), Some("JSON Example"));
}

#[test]
fn malformed_json_feed_yields_invalid_content_error() {
    let err = FeedEngine::new()
        .render(
            &EngineInput::new("feed:bad", "{ \"title\": ")
                .with_content_type("application/feed+json"),
        )
        .expect_err("expected error");
    assert!(matches!(err, EngineError::InvalidContent(_)));
}

#[test]
fn rss_feed_extracts_title_and_entries() {
    let doc = render(RSS_SAMPLE);
    assert_eq!(doc.title.as_deref(), Some("Example Feed"));
    assert_eq!(doc.lang.as_deref(), Some("en-US"));

    let urls = doc.outgoing_links();
    // Feed-header source link (from RSS channel <link>) plus each entry's
    // article URL. Entries don't carry a separate source URL in v1.
    assert_eq!(
        urls,
        vec![
            "https://example.test/",
            "https://example.test/first",
            "https://example.test/second",
        ]
    );

    let entry_titles: Vec<&str> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::FeedEntry { title, .. } => Some(title.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(entry_titles, vec!["First post", "Second post"]);
}

#[test]
fn rss_emits_feed_header_with_subtitle() {
    let doc = render(RSS_SAMPLE);
    let header = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedHeader {
                title,
                subtitle,
                source_url,
                ..
            } => Some((title.as_str(), subtitle.as_deref(), source_url.as_deref())),
            _ => None,
        })
        .expect("expected FeedHeader block");
    assert_eq!(header.0, "Example Feed");
    assert_eq!(header.1, Some("Example feed description"));
    assert_eq!(header.2, Some("https://example.test/"));
}

#[test]
fn rss_summary_strips_html_tags_into_feed_entry_summary() {
    let doc = render(RSS_SAMPLE);
    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedEntry { title, summary, .. } if title == "First post" => summary.as_deref(),
            _ => None,
        })
        .expect("expected first-post summary");
    assert_eq!(summary, "This is the first post.");
}

#[test]
fn rss_html_strip_emits_degraded_rendering_diagnostic() {
    let doc = render(RSS_SAMPLE);
    let degraded = doc.diagnostics.iter().find_map(|d| match d {
        DocumentDiagnostic::DegradedRendering(msg) => Some(msg.as_str()),
        _ => None,
    });
    let msg = degraded.expect("expected DegradedRendering diagnostic");
    assert!(msg.contains("HTML"), "got: {msg}");
}

#[test]
fn atom_feed_extracts_title_and_entries() {
    let doc = render(ATOM_SAMPLE);
    assert_eq!(doc.title.as_deref(), Some("Atom Example"));
    let urls = doc.outgoing_links();
    assert_eq!(
        urls,
        vec![
            "https://example.test/",
            "https://example.test/atom-one",
            "https://example.test/atom-two"
        ]
    );
}

#[test]
fn atom_content_html_is_stripped_into_entry_summary() {
    let doc = render(ATOM_SAMPLE);
    let summary = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::FeedEntry { title, summary, .. } if title == "Atom two" => summary.as_deref(),
            _ => None,
        })
        .expect("expected atom-two summary");
    assert_eq!(summary, "HTML content.");
}

#[test]
fn empty_feed_emits_only_a_header_block() {
    // A channel with just a title produces a FeedHeader (so the title is
    // visible as document chrome) but no entries.
    let doc = render(
        r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Empty</title></channel></rss>"#,
    );
    assert_eq!(doc.title.as_deref(), Some("Empty"));
    assert_eq!(doc.blocks.len(), 1);
    assert!(matches!(doc.blocks[0], Block::FeedHeader { .. }));
}

#[test]
fn malformed_xml_yields_invalid_content_error() {
    let err = FeedEngine::new()
        .render(&EngineInput::new("feed:bad", "<rss><channel><title>X"))
        .expect_err("expected error");
    assert!(matches!(err, EngineError::InvalidContent(_)));
}

#[test]
fn dispatches_through_inker_registry() {
    use inker::EngineRegistry;
    use inker::routing::{
        EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
    };

    let mut registry = EngineRegistry::new();
    registry.register(Box::new(FeedEngine::new()));
    let decision = EngineRouteDecision {
        engine_id: ENGINE_ID.to_string(),
        surface_contract: SurfaceContract {
            target: SurfaceTargetId::new("feed:1"),
            mode: SurfaceContractMode::CompositedTexture,
        },
    };
    let doc = registry
        .dispatch(&decision, &EngineInput::new("feed:1", RSS_SAMPLE))
        .expect("dispatch");
    assert_eq!(doc.title.as_deref(), Some("Example Feed"));
}

#[test]
fn end_to_end_via_default_policy_with_content_type() {
    use crate::engines;
    use inker::EngineRegistry;
    use inker::routing::{EngineRoutePolicy, EngineRouteRequest, WorkspaceRouteId};

    let policy = EngineRoutePolicy::default();
    let request = EngineRouteRequest {
        workspace_id: WorkspaceRouteId::new("main"),
        view: None,
        node: None,
        address: "https://example.test/feed.xml".to_string(),
        content_type: Some("application/rss+xml".to_string()),
        pinned_engine: None,
    };
    let decision = policy.route(&request);
    assert_eq!(decision.engine_id, ENGINE_ID);

    let mut registry = EngineRegistry::new();
    for engine in engines() {
        registry.register(engine);
    }
    let doc = registry
        .dispatch(
            &decision,
            &EngineInput::new("https://example.test/feed.xml", RSS_SAMPLE),
        )
        .expect("dispatch");
    assert_eq!(doc.title.as_deref(), Some("Example Feed"));
}
