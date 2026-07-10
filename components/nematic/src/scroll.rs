/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Scroll engine — body-shape parser for the Scroll smolweb protocol
//! (<https://scroll.mozz.us>).
//!
//! Scroll responses are a binary envelope (sender / signature / timestamp /
//! content-type) wrapped around a body. The envelope side is the host
//! transport's job — the engine receives the body with the envelope's
//! content-type already in [`EngineInput::content_type`] when known.
//!
//! Body content-type → delegate:
//!
//! - `text/gemini` (default) → [`crate::GemtextEngine`]
//! - `text/markdown` / `text/x-markdown` → [`crate::MarkdownEngine`]
//!
//! Until envelope decoding lands in the transport layer, the engine emits
//! a [`DocumentDiagnostic::UnsupportedConstruct`] noting that signature
//! verification was not performed; trust stays `Unknown`. Once the host
//! has a verified envelope it overrides [`EngineDocument::trust`] before
//! handing the document to the projection layer.

use inker::{
    DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput,
};

use crate::{GemtextEngine, MarkdownEngine};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.scroll";

/// Scroll body engine. Owns inner gemtext / markdown engines for body
/// dispatch.
pub struct ScrollEngine {
    gemtext: GemtextEngine,
    markdown: MarkdownEngine,
}

impl ScrollEngine {
    pub fn new() -> Self {
        Self {
            gemtext: GemtextEngine::new(),
            markdown: MarkdownEngine::new(),
        }
    }
}

impl Default for ScrollEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for ScrollEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let inner: &dyn Engine = match input.content_type.as_deref() {
            Some(ct) if matches_markdown(ct) => &self.markdown,
            _ => &self.gemtext,
        };
        let mut doc = inner.render(input)?;

        // Override the inner provenance with this engine's own ID so
        // consumers see "nematic.scroll" as the source kind. Inner engine
        // ID is preserved as `source_label` so the dispatch path stays
        // visible.
        let inner_kind = doc.provenance.source_kind.clone();
        doc.provenance = DocumentProvenance {
            source_kind: Some(self.engine_id().to_string()),
            canonical_uri: Some(input.address.clone()),
            fetched_at: None,
            source_label: inner_kind,
        };
        doc.trust = DocumentTrustState::Unknown;
        doc.diagnostics
            .push(DocumentDiagnostic::UnsupportedConstruct(
                "scroll envelope signature verification not performed by this engine".to_string(),
            ));

        Ok(doc)
    }
}

fn matches_markdown(content_type: &str) -> bool {
    let primary = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    primary == "text/markdown" || primary == "text/x-markdown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(ScrollEngine::new().engine_id(), "nematic.scroll");
    }

    #[test]
    fn default_body_treated_as_gemtext() {
        let doc = ScrollEngine::new()
            .render(&EngineInput::new("scroll://t/", "# Hello\n"))
            .expect("render");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert_eq!(
            doc.provenance.source_kind.as_deref(),
            Some("nematic.scroll")
        );
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.gemtext")
        );
    }

    #[test]
    fn markdown_content_type_routes_to_markdown_engine() {
        let doc = ScrollEngine::new()
            .render(
                &EngineInput::new("scroll://t/", "# Hello\n\n*emphasis*\n")
                    .with_content_type("text/markdown"),
            )
            .expect("render");
        // pulldown-cmark sets content_type to "text/markdown" by default,
        // but scroll override... actually we pass through inner doc's content
        // type. Markdown engine sets it from input.content_type, so it'll be
        // "text/markdown" here.
        assert_eq!(doc.content_type, "text/markdown");
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.markdown")
        );
    }

    #[test]
    fn missing_envelope_verification_emits_diagnostic() {
        let doc = ScrollEngine::new()
            .render(&EngineInput::new("scroll://t/", "# Hi\n"))
            .expect("render");
        let has_warning = doc.diagnostics.iter().any(|d| {
            matches!(
                d,
                DocumentDiagnostic::UnsupportedConstruct(msg)
                    if msg.contains("envelope") && msg.contains("signature")
            )
        });
        assert!(has_warning, "expected envelope-not-verified diagnostic");
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(ScrollEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("scroll:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let doc = registry
            .dispatch(&decision, &EngineInput::new("scroll://t/", "# T\n"))
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("T"));
    }
}
