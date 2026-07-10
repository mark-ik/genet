/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Finger engine — RFC 1288 finger protocol responses.
//!
//! Finger responses are plain text — login info, idle time, mail status,
//! `.plan` and `.project` files. There is no structured format beyond the
//! lines themselves, so this engine delegates to [`TextEngine`] and tags the
//! content type as `text/x-finger` when the host hasn't supplied one.
//!
//! The engine exists as a distinct lane (rather than routing `finger://`
//! straight to `nematic.text`) so telemetry / logging / future
//! finger-specific structure handling have a stable engine ID to attach to.

use inker::{DocumentProvenance, Engine, EngineDocument, EngineError, EngineInput};

use crate::TextEngine;

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.finger";

/// Finger protocol engine. Wraps [`TextEngine`] with finger-specific
/// content-type tagging.
pub struct FingerEngine {
    text: TextEngine,
}

impl FingerEngine {
    pub fn new() -> Self {
        Self {
            text: TextEngine::new(),
        }
    }
}

impl Default for FingerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for FingerEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        let mut doc = self.text.render(input)?;
        if input.content_type.is_none() {
            doc.content_type = "text/x-finger".to_string();
        }
        // Override the inner text-engine provenance with this engine's own
        // ID so consumers see "nematic.finger" as the source kind.
        doc.provenance = DocumentProvenance::for_engine(self.engine_id(), &input.address);
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        FingerEngine::new()
            .render(&EngineInput::new("finger://user@host", body))
            .expect("render")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(FingerEngine::new().engine_id(), "nematic.finger");
    }

    #[test]
    fn default_content_type_is_finger_specific() {
        let doc = render(
            "Login: alice              Name: Alice Example\nDirectory: /home/alice    Shell: /bin/zsh\n",
        );
        assert_eq!(doc.content_type, "text/x-finger");
        // Body parsed as text — single paragraph with soft breaks.
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn host_supplied_content_type_wins() {
        let doc = FingerEngine::new()
            .render(&EngineInput::new("finger://user@host", "x").with_content_type("text/plain"))
            .expect("render");
        assert_eq!(doc.content_type, "text/plain");
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(FingerEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("finger:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let doc = registry
            .dispatch(
                &decision,
                &EngineInput::new("finger://alice@host", "Alice info"),
            )
            .expect("dispatch");
        assert_eq!(doc.content_type, "text/x-finger");
    }
}
