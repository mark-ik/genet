/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Misfin engine — body-shape parser for Misfin (`misfin://`) gemini-style
//! peer-to-peer mail (<https://misfin.org>).
//!
//! Misfin messages are a sender / recipient / timestamp / certificate
//! envelope wrapped around a gemtext body. Addressing is `name@domain`-shaped
//! and tied to client certificates.
//!
//! Envelope decoding (sender / recipient / timestamp / cert verification)
//! happens in the host transport. The engine receives the body and parses
//! it as gemtext, attaching a [`DocumentDiagnostic`] noting that the
//! envelope wasn't validated here. Trust stays `Unknown` until the host
//! overrides it after cert verification.

use inker::{
    DocumentDiagnostic, DocumentProvenance, DocumentTrustState, Engine, EngineDocument,
    EngineError, EngineInput,
};

use crate::GemtextEngine;

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.misfin";

/// Misfin message body engine.
pub struct MisfinEngine {
    gemtext: GemtextEngine,
}

impl MisfinEngine {
    pub fn new() -> Self {
        Self {
            gemtext: GemtextEngine::new(),
        }
    }
}

impl Default for MisfinEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for MisfinEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let mut doc = self.gemtext.render(input)?;

        // Override provenance with this engine; preserve gemtext's ID as the
        // dispatched-through label.
        doc.provenance = DocumentProvenance {
            source_kind: Some(self.engine_id().to_string()),
            canonical_uri: Some(input.address.clone()),
            fetched_at: None,
            source_label: Some("nematic.gemtext".to_string()),
        };

        // Tag the content type with the misfin-specific media type when the
        // host hasn't supplied one.
        if input.content_type.is_none() {
            doc.content_type = "message/x-misfin".to_string();
        }

        doc.trust = DocumentTrustState::Unknown;
        doc.diagnostics
            .push(DocumentDiagnostic::UnsupportedConstruct(
                "misfin envelope (sender / recipient / timestamp / certificate) not parsed by this engine"
                    .to_string(),
            ));

        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        MisfinEngine::new()
            .render(&EngineInput::new("misfin://alice@example.test", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(MisfinEngine::new().engine_id(), "nematic.misfin");
    }

    #[test]
    fn body_parses_as_gemtext() {
        let doc = render("# Hello, Bob\n\n=> gemini://example.test/ Capsule\n");
        assert_eq!(doc.title.as_deref(), Some("Hello, Bob"));
        assert_eq!(doc.outgoing_links(), vec!["gemini://example.test/"]);
    }

    #[test]
    fn provenance_records_misfin_with_gemtext_label() {
        let doc = render("Hi.\n");
        assert_eq!(
            doc.provenance.source_kind.as_deref(),
            Some("nematic.misfin")
        );
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.gemtext")
        );
    }

    #[test]
    fn content_type_defaults_to_misfin_message() {
        let doc = render("Hi.\n");
        assert_eq!(doc.content_type, "message/x-misfin");
    }

    #[test]
    fn missing_envelope_verification_emits_diagnostic() {
        let doc = render("Hi.\n");
        let has_warning = doc.diagnostics.iter().any(|d| {
            matches!(
                d,
                DocumentDiagnostic::UnsupportedConstruct(msg)
                    if msg.contains("envelope") && msg.contains("certificate")
            )
        });
        assert!(has_warning, "expected envelope-not-parsed diagnostic");
    }
}
