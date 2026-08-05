// Copyright 2026 Mark AB (markik)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Scroll engine — body-shape dispatch for the Scroll smolweb protocol
//! (<https://scrollprotocol.us.to/>, port 5699, TLS mandatory).
//!
//! **Corrected 2026-08-04.** This module previously described a scroll
//! response as "a binary envelope (sender / signature / timestamp /
//! content-type)" and emitted a diagnostic about unverified signatures. No
//! source supports that. What the protocol catalogue at
//! zzo38computer.org actually records is: the request is the full URL, a
//! space, and the acceptable languages in BCP47 separated by commas; and
//! **the first four lines of the response are metadata**. Line-oriented text,
//! not a binary envelope, and metadata (author and dates) rather than
//! cryptographic signatures. The old description was invented, and a
//! diagnostic that reports on a feature the protocol may not have is worse
//! than no diagnostic at all.
//!
//! The engine receives a body with its content-type already resolved, and
//! dispatches:
//!
//! - `text/markdown` / `text/x-markdown` → [`crate::MarkdownEngine`]
//! - `text/scroll` → gemtext, **degraded and reported** (see below)
//! - anything else → [`crate::GemtextEngine`]
//!
//! ## The `text/scroll` gap
//!
//! Scroll defines its own document format: "a bit more complicated than
//! Gemini, and the inline formatting means that escaping will be required",
//! with document abstracts and Universal Decimal Classification. **We do not
//! implement it**, and reading it as gemtext silently loses every inline
//! construct.
//!
//! So it is no longer silent. A `text/scroll` body still renders through
//! gemtext, because a degraded document beats a blank one, but the engine now
//! emits [`DocumentDiagnostic::DegradedRendering`] saying exactly what was
//! lost. Implementing the format properly is blocked on the specification:
//! `scrollprotocol.us.to` refuses connections, and a spec-accurate parser
//! must not be written from a summary.

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
        let is_scroll_markup = input
            .content_type
            .as_deref()
            .is_some_and(matches_scroll_markup);

        let inner: &dyn Engine = match input.content_type.as_deref() {
            Some(ct) if matches_markdown(ct) => &self.markdown,
            // `text/scroll` lands here too, on purpose: gemtext is the closest
            // reading we have, and a degraded document beats a blank one. The
            // loss is reported below rather than hidden.
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
        // Trust is the transport's to establish (scroll mandates TLS and
        // permits client certificates); an engine handed a body has nothing to
        // judge, so it says so rather than implying a verdict.
        doc.trust = DocumentTrustState::Unknown;

        if is_scroll_markup {
            doc.diagnostics
                .push(DocumentDiagnostic::DegradedRendering(
                    "text/scroll read as gemtext: scroll's inline formatting, abstracts and \
                     classification are not implemented, so inline constructs are lost"
                        .to_string(),
                ));
        }

        Ok(doc)
    }
}

fn matches_markdown(content_type: &str) -> bool {
    matches!(primary_type(content_type).as_str(), "text/markdown" | "text/x-markdown")
}

/// Scroll's own document format, which this engine does not implement.
fn matches_scroll_markup(content_type: &str) -> bool {
    matches!(primary_type(content_type).as_str(), "text/scroll" | "text/x-scroll")
}

/// The MIME type without its parameters, lowercased.
fn primary_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
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

    #[test]
    fn a_scroll_body_reports_what_it_lost_instead_of_pretending() {
        let input = EngineInput::new("scroll://t/", "# Hello
")
            .with_content_type("text/scroll");
        let doc = ScrollEngine::new().render(&input).unwrap();

        let degraded = doc.diagnostics.iter().any(|d| {
            matches!(d, DocumentDiagnostic::DegradedRendering(m) if m.contains("text/scroll"))
        });
        assert!(degraded, "got {:?}", doc.diagnostics);
    }

    #[test]
    fn a_gemtext_body_carries_no_degradation_notice() {
        // Only scroll's own format is degraded; a gemtext body is read exactly.
        let input = EngineInput::new("scroll://t/", "# Hello
")
            .with_content_type("text/gemini");
        let doc = ScrollEngine::new().render(&input).unwrap();
        assert!(
            !doc.diagnostics
                .iter()
                .any(|d| matches!(d, DocumentDiagnostic::DegradedRendering(_))),
            "got {:?}",
            doc.diagnostics
        );
    }

    #[test]
    fn no_diagnostic_claims_anything_about_signatures() {
        // The old engine reported on envelope signatures the protocol may not
        // have. Nothing here may resurrect that claim.
        for content_type in ["text/scroll", "text/gemini", "text/markdown"] {
            let input = EngineInput::new("scroll://t/", "# Hi
")
                .with_content_type(content_type);
            let doc = ScrollEngine::new().render(&input).unwrap();
            for diagnostic in &doc.diagnostics {
                let text = format!("{diagnostic:?}").to_lowercase();
                assert!(!text.contains("signature"), "{content_type}: {diagnostic:?}");
                assert!(!text.contains("envelope"), "{content_type}: {diagnostic:?}");
            }
        }
    }

    #[test]
    fn the_scroll_markup_type_is_recognised_with_parameters() {
        assert!(matches_scroll_markup("text/scroll"));
        assert!(matches_scroll_markup("Text/Scroll; charset=utf-8"));
        assert!(!matches_scroll_markup("text/gemini"));
    }
}
