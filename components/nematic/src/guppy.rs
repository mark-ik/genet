/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Guppy engine — body-shape parser for Guppy (`guppy://`), the UDP-based
//! smolweb protocol (<https://guppy.mozz.us>).
//!
//! Guppy responses arrive as a sequence of UDP packets that the host
//! transport reassembles. The reassembled body is gemtext-shaped, so the
//! engine just delegates body parsing to [`crate::GemtextEngine`].
//!
//! Reassembly itself (UDP packet sequencing, retries, completion detection)
//! is *upstream* of this engine — it lives wherever the network socket
//! lives, not in the portable parser. The engine therefore makes no
//! assumptions about delivery and adds no diagnostic about envelope
//! integrity beyond the trust state defaulting to `Unknown`.

use inker::{
    DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError, EngineInput,
};

use crate::GemtextEngine;

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.guppy";

/// Guppy body engine — wraps gemtext with guppy provenance tagging.
pub struct GuppyEngine {
    gemtext: GemtextEngine,
}

impl GuppyEngine {
    pub fn new() -> Self {
        Self {
            gemtext: GemtextEngine::new(),
        }
    }
}

impl Default for GuppyEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for GuppyEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let mut doc = self.gemtext.render(input)?;

        doc.provenance = DocumentProvenance {
            source_kind: Some(self.engine_id().to_string()),
            canonical_uri: Some(input.address.clone()),
            fetched_at: None,
            source_label: Some("nematic.gemtext".to_string()),
        };
        doc.trust = DocumentTrustState::Unknown;

        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        GuppyEngine::new()
            .render(&EngineInput::new("guppy://example.test/", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(GuppyEngine::new().engine_id(), "nematic.guppy");
    }

    #[test]
    fn body_parses_as_gemtext() {
        let doc = render("# Hi\n\n=> guppy://capsule.test/ Visit\n");
        assert_eq!(doc.title.as_deref(), Some("Hi"));
        assert_eq!(doc.outgoing_links(), vec!["guppy://capsule.test/"]);
    }

    #[test]
    fn provenance_records_guppy_with_gemtext_label() {
        let doc = render("body\n");
        assert_eq!(doc.provenance.source_kind.as_deref(), Some("nematic.guppy"));
        assert_eq!(
            doc.provenance.source_label.as_deref(),
            Some("nematic.gemtext")
        );
    }
}
