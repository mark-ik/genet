/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! File viewer engine.
//!
//! Routes file content to the right format engine by file extension.
//! Hosts that load a file body without knowing its MIME type can hand the
//! whole input to this engine and it picks markdown / gemtext / gopher /
//! feed / text based on the extension in the address.
//!
//! When the host *does* know the content type, the host should set
//! [`EngineInput::content_type`] and let [`inker::EngineRoutePolicy`]'s
//! content-type rules win — they take precedence over the scheme-level
//! `file://` → `nematic.file` route, so explicit MIME beats extension sniff.

use inker::{DocumentProvenance, Engine, EngineDocument, EngineError, EngineInput};

use crate::{FeedEngine, GemtextEngine, GopherEngine, KnotEngine, MarkdownEngine, TextEngine};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.file";

/// File viewer engine. Owns one instance of each delegate engine and
/// dispatches by extension.
pub struct FileEngine {
    markdown: MarkdownEngine,
    gemtext: GemtextEngine,
    gopher: GopherEngine,
    feed: FeedEngine,
    knot: KnotEngine,
    text: TextEngine,
}

impl FileEngine {
    pub fn new() -> Self {
        Self {
            markdown: MarkdownEngine::new(),
            gemtext: GemtextEngine::new(),
            gopher: GopherEngine::new(),
            feed: FeedEngine::new(),
            knot: KnotEngine::new(),
            text: TextEngine::new(),
        }
    }

    fn pick(&self, address: &str) -> &dyn Engine {
        match extension(address).as_deref() {
            Some("md") | Some("markdown") | Some("mkd") | Some("mdown") => &self.markdown,
            Some("gmi") | Some("gemini") => &self.gemtext,
            Some("gophermap") | Some("goph") => &self.gopher,
            Some("xml") | Some("rss") | Some("atom") => &self.feed,
            Some("knot") => &self.knot,
            _ => &self.text,
        }
    }
}

impl Default for FileEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for FileEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let mut doc = self.pick(&input.address).render(input)?;
        // The inner engine sets provenance to its own ID (markdown, gemtext,
        // etc.). Tag the file engine as the *outer* source so consumers can
        // tell content arrived through the file viewer rather than directly
        // from a network engine. Inner engine ID is preserved in the
        // `source_label` so the dispatch path stays visible.
        let inner_kind = doc.provenance.source_kind.clone();
        doc.provenance = DocumentProvenance {
            source_kind: Some(self.engine_id().to_string()),
            canonical_uri: Some(input.address.clone()),
            fetched_at: None,
            source_label: inner_kind,
        };
        Ok(doc)
    }
}

/// Extract the lowercase extension (without the leading dot) from a URI or
/// path. Returns `None` if there is no extension component.
fn extension(address: &str) -> Option<String> {
    // Strip the scheme so `file://` doesn't trip on the colon, then drop
    // any query / fragment / trailing slash.
    let without_scheme = address.split_once("://").map(|(_, p)| p).unwrap_or(address);
    let path = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme)
        .trim_end_matches('/');

    let last_segment = path.rsplit('/').next()?;
    let dot_idx = last_segment.rfind('.')?;
    if dot_idx == 0 {
        // Hidden files like ".gitignore" have no useful extension.
        return None;
    }
    let ext = &last_segment[dot_idx + 1..];
    if ext.is_empty() {
        None
    } else {
        Some(ext.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inker::Block;

    fn render(address: &str, body: &str) -> EngineDocument {
        FileEngine::new()
            .render(&EngineInput::new(address, body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(FileEngine::new().engine_id(), "nematic.file");
    }

    #[test]
    fn md_extension_dispatches_to_markdown() {
        let doc = render("file:///home/user/notes.md", "# Hello\n\ntext");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert_eq!(doc.content_type, "text/markdown");
    }

    #[test]
    fn gmi_extension_dispatches_to_gemtext() {
        let doc = render(
            "file:///home/user/page.gmi",
            "# Hi\n\n=> https://t.test/ here",
        );
        assert_eq!(doc.title.as_deref(), Some("Hi"));
        assert_eq!(doc.content_type, "text/gemini");
        assert_eq!(doc.outgoing_links(), vec!["https://t.test/"]);
    }

    #[test]
    fn xml_extension_dispatches_to_feed() {
        let body =
            r#"<?xml version="1.0"?><rss version="2.0"><channel><title>X</title></channel></rss>"#;
        let doc = render("file:///feeds/weekly.xml", body);
        assert_eq!(doc.title.as_deref(), Some("X"));
    }

    #[test]
    fn unknown_extension_falls_back_to_text() {
        let doc = render(
            "file:///home/user/notes.unknown",
            "first line\n\nsecond paragraph",
        );
        assert_eq!(doc.content_type, "text/plain");
        assert_eq!(doc.blocks.len(), 2);
    }

    #[test]
    fn no_extension_falls_back_to_text() {
        let doc = render("file:///home/user/Makefile", "all:\n\techo hi\n");
        assert_eq!(doc.content_type, "text/plain");
    }

    #[test]
    fn extension_extraction_handles_query_strings() {
        let doc = render("file:///foo/page.md?a=1&b=2", "# Title\n");
        assert_eq!(doc.title.as_deref(), Some("Title"));
    }

    #[test]
    fn extension_match_is_case_insensitive() {
        let doc = render("file:///Notes.MD", "# Hello\n");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
    }

    #[test]
    fn hidden_files_have_no_extension() {
        // `.gitignore` should not be mistaken for a `.gitignore` extension.
        assert_eq!(extension("file:///proj/.gitignore"), None);
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(FileEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("file:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let doc = registry
            .dispatch(
                &decision,
                &EngineInput::new("file:///docs/readme.md", "# Readme\n\nbody"),
            )
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("Readme"));
        // Ensure the *internal* dispatch produced a Heading, proving the
        // markdown engine ran (not the text fallback).
        assert!(matches!(
            doc.blocks.first(),
            Some(Block::Heading { level: 1, .. })
        ));
    }

    #[test]
    fn end_to_end_via_default_policy_routes_file_scheme() {
        use crate::engines;
        use inker::EngineRegistry;
        use inker::routing::{EngineRoutePolicy, EngineRouteRequest, WorkspaceRouteId};

        let policy = EngineRoutePolicy::default();
        let request = EngineRouteRequest {
            workspace_id: WorkspaceRouteId::new("main"),
            view: None,
            node: None,
            address: "file:///home/user/notes.md".to_string(),
            content_type: None,
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
                &EngineInput::new("file:///home/user/notes.md", "# Title\n\ntext"),
            )
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("Title"));
    }
}
