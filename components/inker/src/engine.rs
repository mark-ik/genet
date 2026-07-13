/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Engine trait, input/error vocabulary, and registry.
//!
//! Routing decides which engine handles an address. This module defines what
//! an engine *is*: a thing that takes raw content (already fetched) and
//! produces a portable [`crate::EngineDocument`] that downstream consumers
//! (document-canvas, platen, the host) can present. Concrete engines live
//! outside this crate (`nematic` for smolweb / markdown / file lanes;
//! `genet` for full web; etc).
//!
//! Network and disk I/O are deliberately the host's job, not the engine's:
//! engines stay portable to wasm32 / browser / PWA targets where network
//! shape varies, and routing stays separable from rendering.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::a11y::A11yCapability;
use crate::document::EngineDocument;
use crate::routing::EngineRouteDecision;

/// Raw input to an engine. Content is already fetched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineInput {
    pub address: String,
    pub body: String,
    pub content_type: Option<String>,
}

impl EngineInput {
    pub fn new(address: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            body: body.into(),
            content_type: None,
        }
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineError {
    EngineNotFound(String),
    Unsupported(String),
    InvalidContent(String),
    NotFound(String),
    Io(String),
    Network(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EngineNotFound(id) => write!(f, "engine not registered: {id}"),
            Self::Unsupported(reason) => write!(f, "unsupported content: {reason}"),
            Self::InvalidContent(reason) => write!(f, "invalid content: {reason}"),
            Self::NotFound(address) => write!(f, "not found: {address}"),
            Self::Io(reason) => write!(f, "io error: {reason}"),
            Self::Network(reason) => write!(f, "network error: {reason}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Concrete content engine: parses raw bytes into a portable
/// [`EngineDocument`].
pub trait Engine: Send + Sync {
    /// Stable engine identifier. Must match the `engine_id` field of the
    /// [`EngineRouteDecision`] that selected this engine.
    fn engine_id(&self) -> &str;

    /// Parse raw content into a portable document.
    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError>;

    /// This engine's accessibility capability (see [`crate::a11y`]). Document
    /// engines default to [`A11yCapability::Full`] — their [`EngineDocument`]
    /// blocks *are* the semantic tree. Override to declare degradation (an
    /// engine that drops structure must not claim `Full`).
    fn a11y_capability(&self) -> A11yCapability {
        A11yCapability::Full
    }
}

/// Engine ID → engine instance dispatch.
#[derive(Default)]
pub struct EngineRegistry {
    engines: HashMap<String, Box<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, engine: Box<dyn Engine>) {
        let id = engine.engine_id().to_string();
        self.engines.insert(id, engine);
    }

    pub fn engine(&self, id: &str) -> Option<&dyn Engine> {
        self.engines.get(id).map(|engine| engine.as_ref())
    }

    /// True when an engine with the given ID is registered. Pair with
    /// [`crate::routing::EngineRoutePolicy::route_filtered`] to route only
    /// to engines that actually exist on this host.
    pub fn contains(&self, id: &str) -> bool {
        self.engines.contains_key(id)
    }

    pub fn engine_ids(&self) -> impl Iterator<Item = &str> {
        self.engines.keys().map(String::as_str)
    }

    /// Render `input` using the engine selected by `decision.engine_id`.
    #[tracing::instrument(
        level = "debug",
        skip(self, decision, input),
        fields(
            engine_id = %decision.engine_id,
            address = %input.address,
        ),
    )]
    pub fn dispatch(
        &self,
        decision: &EngineRouteDecision,
        input: &EngineInput,
    ) -> Result<EngineDocument, EngineError> {
        let engine = self.engine(&decision.engine_id).ok_or_else(|| {
            tracing::warn!(
                engine_id = %decision.engine_id,
                "engine not registered"
            );
            EngineError::EngineNotFound(decision.engine_id.clone())
        })?;
        engine.render(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Block, DocumentProvenance, DocumentTrustState, InlineSpan};
    use crate::routing::{SurfaceContract, SurfaceContractMode, SurfaceTargetId};

    struct EchoEngine;

    impl Engine for EchoEngine {
        fn engine_id(&self) -> &str {
            "test.echo"
        }

        fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
            Ok(EngineDocument {
                address: input.address.clone(),
                title: Some(input.body.clone()),
                content_type: "text/plain".to_string(),
                lang: None,
                provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
                trust: DocumentTrustState::Unknown,
                diagnostics: Vec::new(),
                blocks: vec![Block::Paragraph {
                    spans: vec![InlineSpan::Text(input.body.clone())],
                }],
            })
        }
    }

    fn decision(id: &str) -> EngineRouteDecision {
        EngineRouteDecision {
            engine_id: id.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("test:1"),
                mode: SurfaceContractMode::Headless,
            },
        }
    }

    #[test]
    fn registry_dispatches_by_engine_id() {
        let mut reg = EngineRegistry::new();
        reg.register(Box::new(EchoEngine));
        let document = reg
            .dispatch(&decision("test.echo"), &EngineInput::new("test:1", "hello"))
            .expect("dispatch");
        assert_eq!(document.title.as_deref(), Some("hello"));
        assert_eq!(document.address, "test:1");
        assert_eq!(document.blocks.len(), 1);
        assert_eq!(
            document.provenance.source_kind.as_deref(),
            Some("test.echo")
        );
    }

    #[test]
    fn registry_reports_missing_engine() {
        let reg = EngineRegistry::new();
        let err = reg
            .dispatch(&decision("test.absent"), &EngineInput::new("a", "b"))
            .expect_err("expected EngineNotFound");
        assert!(matches!(err, EngineError::EngineNotFound(_)));
    }

    /// Contract: document engines are `Full` a11y by default (their blocks are
    /// the semantic tree); an engine that degrades must *declare* it.
    #[test]
    fn a11y_capability_defaults_full_and_degrades_explicitly() {
        assert_eq!(EchoEngine.a11y_capability(), A11yCapability::Full);

        struct LossyEngine;
        impl Engine for LossyEngine {
            fn engine_id(&self) -> &str {
                "test.lossy"
            }
            fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
                EchoEngine.render(input)
            }
            // Declares its degradation rather than silently claiming Full.
            fn a11y_capability(&self) -> A11yCapability {
                A11yCapability::Partial
            }
        }
        assert_eq!(LossyEngine.a11y_capability(), A11yCapability::Partial);
        assert!(LossyEngine.a11y_capability().is_inspectable());
        assert!(!A11yCapability::Opaque.is_inspectable());
    }
}
