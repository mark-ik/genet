/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # Nematic
//!
//! Portable smolweb engine for the [`mere`](https://crates.io/crates/mere)
//! browser — renders Gemini, Gopher, static HTML, Markdown, RSS/Atom, and
//! other lightweight protocols where content can be rendered with minimal
//! layout cost.
//!
//! The name *nematic* is borrowed from liquid-crystal physics: a nematic
//! phase has *orientational* order without *positional* order — rod-shaped
//! molecules all point the same way but otherwise flow freely. Light passes
//! through aligned nematic crystals coherently; that's the basis of LCDs.
//! The metaphor for a smolweb rendering engine: organized-but-flowing,
//! threads aligned, the reader sees through to the meaning without
//! layout-noise getting in the way.
//!
//! ## Engines
//!
//! Nematic implements concrete [`inker::Engine`]s. Currently shipped:
//!
//! - [`MarkdownEngine`] — CommonMark via `pulldown-cmark`
//! - [`GemtextEngine`] — Gemini's `text/gemini` line-oriented format
//! - [`GopherEngine`] — RFC 1436 gopher menu parser
//! - [`FeedEngine`] — RSS 2.0, Atom 1.0, and JSON Feed 1.x syndication feeds
//! - [`TextEngine`] — plain text with paragraph splitting
//! - [`FileEngine`] — extension-based dispatch for `file://` content
//! - [`FingerEngine`] — RFC 1288 finger protocol responses
//! - [`KnotEngine`] — Mere's native note / clip format (frontmatter + markdown)
//! - [`ScrollEngine`] — Scroll smolweb body (gemtext or markdown)
//! - [`SpartanEngine`] — Spartan smolweb body (gemtext or markdown)
//! - [`TitanEngine`] — Titan response body (gemtext; upload is transport-side)
//! - [`MisfinEngine`] — Misfin gemini-style mail body (gemtext)
//! - [`NexEngine`] — Nex directory listings + content
//! - [`GuppyEngine`] — Guppy UDP-smolweb body (gemtext)
//!
//! Use [`engines`] to get a `Vec<Box<dyn Engine>>` of all default nematic
//! engines for one-call registration with [`inker::EngineRegistry`].
//!
//! ## Status
//!
//! Pre-1.0. All seven smolweb / file / text protocol lanes shipped. Reader-
//! mode HTML simplification (a separate concern from full-web rendering,
//! which Genet owns) is the only major lane still pending.

#![doc(html_root_url = "https://docs.rs/nematic/0.0.1")]

pub mod feed;
pub mod file;
pub mod finger;
pub mod gemtext;
pub mod gopher;
pub mod guppy;
pub mod knot;

/// The native render lane (was the `smolweb-views` component, merged
/// 2026-07-11 per the session-engines plan phase 5): per-format
/// xilem-serval views over errand's ASTs — the idiom carriers — plus the
/// theme vocabulary. Feature-gated: the block engines stay light; hosts
/// that render natively opt in.
#[cfg(feature = "views")]
pub mod views;
pub mod markdown;
pub mod misfin;
pub mod nex;
pub mod scroll;
pub mod spartan;
pub mod text;
pub mod titan;

pub use feed::{ENGINE_ID as ENGINE_FEED, FeedEngine};
pub use file::{ENGINE_ID as ENGINE_FILE, FileEngine};
pub use finger::{ENGINE_ID as ENGINE_FINGER, FingerEngine};
pub use gemtext::{ENGINE_ID as ENGINE_GEMTEXT, GemtextEngine};
pub use gopher::{ENGINE_ID as ENGINE_GOPHER, GopherEngine};
pub use guppy::{ENGINE_ID as ENGINE_GUPPY, GuppyEngine};
pub use knot::{ENGINE_ID as ENGINE_KNOT, KnotEngine};
// The djot-bodied knot engine (design doc §10). Registered in `engines()` below; it shares the
// `text/x-knot` content-type with `KnotEngine`, and routing resolves that content-type to this
// djot engine as the default knot grammar.
pub use knot::djot::{DjotKnotEngine, ENGINE_ID as ENGINE_KNOT_DJOT};
pub use markdown::{ENGINE_ID as ENGINE_MARKDOWN, MarkdownEngine};
pub use misfin::{ENGINE_ID as ENGINE_MISFIN, MisfinEngine};
pub use nex::{ENGINE_ID as ENGINE_NEX, NexEngine};
pub use scroll::{ENGINE_ID as ENGINE_SCROLL, ScrollEngine};
pub use spartan::{ENGINE_ID as ENGINE_SPARTAN, SpartanEngine};
pub use text::{ENGINE_ID as ENGINE_TEXT, TextEngine};
pub use titan::{ENGINE_ID as ENGINE_TITAN, TitanEngine};

use inker::Engine;

/// All default nematic engines, ready to register with an
/// [`inker::EngineRegistry`].
pub fn engines() -> Vec<Box<dyn Engine>> {
    vec![
        Box::new(MarkdownEngine::new()),
        Box::new(GemtextEngine::new()),
        Box::new(GopherEngine::new()),
        Box::new(FeedEngine::new()),
        Box::new(TextEngine::new()),
        Box::new(FileEngine::new()),
        Box::new(FingerEngine::new()),
        Box::new(KnotEngine::new()),
        Box::new(DjotKnotEngine::new()),
        Box::new(ScrollEngine::new()),
        Box::new(SpartanEngine::new()),
        Box::new(TitanEngine::new()),
        Box::new(MisfinEngine::new()),
        Box::new(NexEngine::new()),
        Box::new(GuppyEngine::new()),
    ]
}

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Lifecycle stage marker.
pub const STAGE: &str = "pre-alpha";

#[cfg(test)]
mod tests {
    use super::*;
    use inker::routing::{
        EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
    };
    use inker::{EngineInput, EngineRegistry};

    fn decision(engine_id: &str) -> EngineRouteDecision {
        EngineRouteDecision {
            engine_id: engine_id.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("test:1"),
                mode: SurfaceContractMode::Headless,
            },
        }
    }

    #[test]
    fn engines_helper_registers_all_default_engines() {
        let mut registry = EngineRegistry::new();
        for engine in engines() {
            registry.register(engine);
        }
        let mut ids: Vec<&str> = registry.engine_ids().collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                ENGINE_FEED,
                ENGINE_FILE,
                ENGINE_FINGER,
                ENGINE_GEMTEXT,
                ENGINE_GOPHER,
                ENGINE_GUPPY,
                ENGINE_KNOT,
                ENGINE_KNOT_DJOT,
                ENGINE_MARKDOWN,
                ENGINE_MISFIN,
                ENGINE_NEX,
                ENGINE_SCROLL,
                ENGINE_SPARTAN,
                ENGINE_TEXT,
                ENGINE_TITAN
            ]
        );
    }

    #[test]
    fn end_to_end_routing_with_default_policy_dispatches_to_gemtext() {
        use inker::routing::{EngineRoutePolicy, EngineRouteRequest, WorkspaceRouteId};

        let policy = EngineRoutePolicy::default();
        let request = EngineRouteRequest {
            workspace_id: WorkspaceRouteId::new("main"),
            view: None,
            node: None,
            address: "gemini://capsule.test/".to_string(),
            content_type: None,
            pinned_engine: None,
        };
        let decision = policy.route(&request);

        let mut registry = EngineRegistry::new();
        for engine in engines() {
            registry.register(engine);
        }

        let doc = registry
            .dispatch(
                &decision,
                &EngineInput::new("gemini://capsule.test/", "# Hello\n\ntext"),
            )
            .expect("dispatch");
        assert_eq!(doc.title.as_deref(), Some("Hello"));
        assert_eq!(decision.engine_id, ENGINE_GEMTEXT);
    }

    #[test]
    fn engines_helper_can_dispatch_each_id() {
        let mut registry = EngineRegistry::new();
        for engine in engines() {
            registry.register(engine);
        }
        for id in [
            ENGINE_MARKDOWN,
            ENGINE_GEMTEXT,
            ENGINE_GOPHER,
            ENGINE_TEXT,
            ENGINE_FILE,
            ENGINE_FINGER,
            ENGINE_KNOT,
            ENGINE_SCROLL,
            ENGINE_MISFIN,
            ENGINE_GUPPY,
        ] {
            let result = registry.dispatch(&decision(id), &EngineInput::new("a:b", "hi"));
            assert!(result.is_ok(), "dispatch for {id} failed: {result:?}");
        }
        // Feed needs a minimal valid XML body to avoid `InvalidContent`.
        let result = registry.dispatch(
            &decision(ENGINE_FEED),
            &EngineInput::new(
                "feed:1",
                r#"<?xml version="1.0"?><rss version="2.0"><channel/></rss>"#,
            ),
        );
        assert!(
            result.is_ok(),
            "dispatch for {ENGINE_FEED} failed: {result:?}"
        );
    }
}
