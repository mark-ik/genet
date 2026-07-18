/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Session-engine traits and registry — the third engine kind
//! (2026-07-10 session-engines plan).
//!
//! Document engines ([`crate::Engine`]) are request/response: bytes in,
//! serializable [`crate::EngineDocument`] blocks out — the stored/authored
//! lane. Surface engines ([`crate::SurfaceEngine`]) stream GPU textures from
//! external producers. Session engines sit between: **retained document
//! sessions** that lay content out once and then produce paint frames on
//! demand, with scroll, activation, and (for scripted lanes) a tick +
//! quiescence seam. The genet HTML lanes and the smolweb native lane are
//! session engines.
//!
//! The frame type is generic (`F`) so this crate keeps zero paint
//! dependencies: a netrender host instantiates `F = netrender::Scene`; a
//! different host picks its own frame type. Lane-specific construction seams
//! (resource fetchers, cookie jars, themes) are injected into the concrete
//! `SessionEngine` at registration time, not carried in the spawn request —
//! the request stays plain data.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::a11y::A11yCapability;

// ── Errors ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionError {
    EngineNotFound(String),
    SpawnFailed(String),
    Unsupported(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EngineNotFound(id) => write!(f, "session engine not registered: {id}"),
            Self::SpawnFailed(reason) => write!(f, "session spawn failed: {reason}"),
            Self::Unsupported(reason) => write!(f, "unsupported: {reason}"),
        }
    }
}

impl std::error::Error for SessionError {}

// ── Spawn request ──────────────────────────────────────────────────────────

/// Plain-data request to open a document session. The body is already
/// fetched when the host has it (mirroring [`crate::EngineInput`]); a session
/// engine whose lane fetches for itself (subresources, redirects) uses the
/// seams it was constructed with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSpawnRequest {
    pub address: String,
    /// Fetched body, when the host fetched it. `None` asks the engine to
    /// load via its own fetcher seam.
    pub body: Option<String>,
    pub content_type: Option<String>,
    /// Initial viewport, so the first `frame` call needs no resize dance.
    pub viewport: (u32, u32),
    /// Spawn hidden (a background tile): the session may defer work the
    /// visible path would do eagerly.
    pub hidden: bool,
}

impl SessionSpawnRequest {
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            body: None,
            content_type: None,
            viewport: (0, 0),
            hidden: false,
        }
    }

    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = Some(content_type.into());
        self
    }

    pub fn with_viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport = (width, height);
        self
    }
}

// ── Interaction vocabulary ─────────────────────────────────────────────────

/// A link the session exposes for the host's hit table: url + viewport-space
/// rect (`[x, y, w, h]`, the shape the lanes already emit).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionLink {
    pub url: String,
    pub rect: [f32; 4],
}

/// A structural report of a session's addressed content — title, an outline
/// of role + name, outgoing links, headings. The introspection CONTRACT for
/// [`DocumentSession::inspect`]: pure data, so it lives here rather than in a
/// render crate (genet-render's `content_report` builds one from a LayoutDom
/// and re-exports these types). Hosts that cannot downcast a session to its
/// concrete type (the type may be private to its engine crate) read this
/// instead — merecat's Inspector pane is the first such consumer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentReport {
    /// The `<title>` text, if any.
    pub title: Option<String>,
    /// The element outline (painted elements only; metadata tags are skipped),
    /// in document order.
    pub outline: Vec<OutlineEntry>,
    /// Outgoing `<a href>` targets, in document order.
    pub links: Vec<String>,
    /// Heading (`<h1>`..`<h6>`) text, in document order.
    pub headings: Vec<String>,
}

/// One element in the structural outline.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    /// Nesting depth among painted elements (the document root is depth 0).
    pub depth: usize,
    /// A coarse semantic role (`"link"`, `"heading"`, `"paragraph"`, …).
    pub role: &'static str,
    /// The element's accessible name — its direct text content, trimmed.
    pub name: String,
}

/// What a click did, unifying the lanes' divergent returns
/// (`ClickOutcome` / `bool` / `Option<String>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionClick {
    /// The click resolved to a navigation the HOST performs (a link).
    Navigate(String),
    /// The session consumed the click itself (focus, a scripted handler).
    Handled,
    /// Nothing interactive at that point.
    Miss,
}

/// Keyboard scroll intents, host-neutral. Adapters map these onto their
/// layout engine's own key vocabulary (genet-layout's `ScrollKey` today);
/// defined here so the contract does not drag a layout dependency in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionScrollKey {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Home,
    End,
}

// ── Traits ─────────────────────────────────────────────────────────────────

/// Spawns retained document sessions for the engine id it claims. Registered
/// once per host; holds its lane's construction seams (fetcher, cookie jar,
/// theme) so the spawn request stays plain data.
pub trait SessionEngine<F>: Send + Sync {
    /// Stable engine identifier. Must match the `engine_id` of the
    /// [`crate::routing::EngineRouteDecision`] that selected this engine.
    fn engine_id(&self) -> &str;

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<F>>, SessionError>;

    /// Sessions lay real content out through a real layout engine, so unlike
    /// surface engines they default to [`A11yCapability::Partial`]; a lane
    /// with a full semantic tree overrides to `Full`.
    fn a11y_capability(&self) -> A11yCapability {
        A11yCapability::Partial
    }
}

/// A live document: a retained layout session producing paint frames.
///
/// All methods take `&mut self`; the session is single-owner, driven from the
/// host's content thread — exactly how the lane types are driven today. Not
/// `Send` by default (scripted lanes hold JS engine state).
pub trait DocumentSession<F>: Any {
    /// Lay out (if needed) and paint at the given viewport. Resize is
    /// implicit: a size change re-lays-out, same as the lanes today.
    fn frame(&mut self, width: u32, height: u32) -> F;

    /// Scroll the viewport; `true` if the offset changed.
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool;

    /// Scroll the scrollable under `(x, y)` (nested scrollers); `true` if an
    /// offset changed. Defaults to viewport scroll for single-scroller lanes.
    fn scroll_at(&mut self, _x: f32, _y: f32, dx: f32, dy: f32) -> bool {
        self.scroll_by(dx, dy)
    }

    fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool;

    /// Jump to an absolute vertical offset (anchor / fragment navigation).
    /// Defaulted no-op for lanes without absolute addressing; lanes that
    /// track their offset override.
    fn scroll_to(&mut self, _y: f32) {}

    fn click_at(&mut self, x: f32, y: f32) -> SessionClick;

    /// The link hit-table off the retained layout (no live-DOM query per
    /// click) — the mechanism all three lanes already share.
    fn links(&self) -> Vec<SessionLink>;

    /// Full laid-out content height at this viewport, for hosts that band
    /// scenes. Sessions that scroll internally return the viewport height.
    fn content_height(&mut self, _width: u32, height: u32) -> u32 {
        height
    }

    /// Drive timers / pending script work (scripted lanes). No-op default.
    fn pump(&mut self, _now_ms: f64) {}

    /// The quiescence contract (native automation plan): no pending script
    /// work, layout clean. Static lanes are always settled.
    fn settled(&mut self) -> bool {
        true
    }

    /// Visibility hint (a hidden tile may skip raster-adjacent work).
    fn set_hidden(&mut self, _hidden: bool) {}

    /// A structural [`ContentReport`] of the addressed content, for hosts that
    /// cannot downcast to the concrete session type (it may be private to its
    /// engine crate — merecat's one registered lane is exactly that case, which
    /// is why this is a trait method and not an `as_any` detour). `None` for
    /// lanes without a structural read; the host reports the absence honestly
    /// rather than synthesizing one.
    fn inspect(&self) -> Option<ContentReport> {
        None
    }

    /// Lane-specific extras (a scripted lane's DOM stats, a static lane's
    /// content report) stay on the concrete type; hosts that need them
    /// downcast through here rather than the trait growing every lane's
    /// diagnostics.
    fn as_any(&mut self) -> &mut dyn Any;
}

// ── Registry ───────────────────────────────────────────────────────────────

/// Session engines keyed by engine id, one registry per host frame type.
#[derive(Default)]
pub struct SessionRegistry<F> {
    engines: HashMap<String, Box<dyn SessionEngine<F>>>,
}

impl<F> SessionRegistry<F> {
    pub fn new() -> Self {
        Self {
            engines: HashMap::new(),
        }
    }

    /// Register an engine under its own id. Last registration wins, matching
    /// [`crate::EngineRegistry`] semantics.
    pub fn register(&mut self, engine: Box<dyn SessionEngine<F>>) {
        self.engines.insert(engine.engine_id().to_string(), engine);
    }

    pub fn contains(&self, engine_id: &str) -> bool {
        self.engines.contains_key(engine_id)
    }

    pub fn get(&self, engine_id: &str) -> Option<&dyn SessionEngine<F>> {
        self.engines.get(engine_id).map(|e| e.as_ref())
    }

    pub fn spawn(
        &self,
        engine_id: &str,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<F>>, SessionError> {
        self.engines
            .get(engine_id)
            .ok_or_else(|| SessionError::EngineNotFound(engine_id.to_string()))?
            .spawn(request)
    }

    pub fn engine_ids(&self) -> impl Iterator<Item = &str> {
        self.engines.keys().map(String::as_str)
    }
}

// ── Kind facade ────────────────────────────────────────────────────────────

/// Which registries hold an engine id. An id may be held by more than one
/// kind (a smolweb format can have both a block engine for cards and a
/// session engine for tiles); the HOST picks by surface context, so this is
/// reported as flags, not a single kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineKinds {
    pub document: bool,
    pub session: bool,
    pub surface: bool,
}

impl EngineKinds {
    pub fn any(&self) -> bool {
        self.document || self.session || self.surface
    }
}

/// Non-generic id-to-kind resolution: which registries hold each id is just
/// a map, so hosts resolve kinds without threading the frame type through
/// code that never touches frames. Built after registration from the
/// registries' id sets; host-handled ids (internal pages, ingest markers)
/// are the host's own vocabulary and deliberately absent.
#[derive(Clone, Debug, Default)]
pub struct EngineKindIndex {
    kinds: HashMap<String, EngineKinds>,
}

impl EngineKindIndex {
    pub fn build<'a>(
        document_ids: impl IntoIterator<Item = &'a str>,
        session_ids: impl IntoIterator<Item = &'a str>,
        surface_ids: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut kinds: HashMap<String, EngineKinds> = HashMap::new();
        for id in document_ids {
            kinds.entry(id.to_string()).or_default().document = true;
        }
        for id in session_ids {
            kinds.entry(id.to_string()).or_default().session = true;
        }
        for id in surface_ids {
            kinds.entry(id.to_string()).or_default().surface = true;
        }
        Self { kinds }
    }

    pub fn kinds_of(&self, engine_id: &str) -> EngineKinds {
        self.kinds.get(engine_id).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame type that just records what was rendered.
    type TextFrame = String;

    struct EchoSession {
        address: String,
        scroll: f32,
        hidden: bool,
    }

    impl DocumentSession<TextFrame> for EchoSession {
        fn frame(&mut self, width: u32, height: u32) -> TextFrame {
            format!("{} @ {width}x{height} scroll={}", self.address, self.scroll)
        }
        fn scroll_by(&mut self, _dx: f32, dy: f32) -> bool {
            self.scroll += dy;
            dy != 0.0
        }
        fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool {
            self.scroll_by(
                0.0,
                if key == SessionScrollKey::PageDown {
                    100.0
                } else {
                    0.0
                },
            )
        }
        fn click_at(&mut self, x: f32, _y: f32) -> SessionClick {
            if x < 10.0 {
                SessionClick::Navigate("gemini://example.test/".into())
            } else {
                SessionClick::Miss
            }
        }
        fn links(&self) -> Vec<SessionLink> {
            vec![SessionLink {
                url: "gemini://example.test/".into(),
                rect: [0.0, 0.0, 10.0, 10.0],
            }]
        }
        fn set_hidden(&mut self, hidden: bool) {
            self.hidden = hidden;
        }
        fn as_any(&mut self) -> &mut dyn Any {
            self
        }
    }

    struct EchoSessionEngine;

    impl SessionEngine<TextFrame> for EchoSessionEngine {
        fn engine_id(&self) -> &str {
            "echo.session"
        }
        fn spawn(
            &self,
            request: &SessionSpawnRequest,
        ) -> Result<Box<dyn DocumentSession<TextFrame>>, SessionError> {
            if request.address.is_empty() {
                return Err(SessionError::SpawnFailed("empty address".into()));
            }
            Ok(Box::new(EchoSession {
                address: request.address.clone(),
                scroll: 0.0,
                hidden: request.hidden,
            }))
        }
    }

    #[test]
    fn registry_spawns_and_drives_a_session() {
        let mut registry = SessionRegistry::new();
        registry.register(Box::new(EchoSessionEngine));

        let request = SessionSpawnRequest::new("https://example.test").with_viewport(800, 600);
        let mut session = registry.spawn("echo.session", &request).expect("spawns");

        assert_eq!(
            session.frame(800, 600),
            "https://example.test @ 800x600 scroll=0"
        );
        assert!(session.scroll_by(0.0, 42.0));
        assert!(session.frame(800, 600).ends_with("scroll=42"));
        assert_eq!(
            session.click_at(5.0, 5.0),
            SessionClick::Navigate("gemini://example.test/".into())
        );
        assert_eq!(session.click_at(50.0, 5.0), SessionClick::Miss);
        assert_eq!(session.links().len(), 1);
        // Static-lane defaults: settled immediately, pump is a no-op.
        assert!(session.settled());
        session.pump(16.0);
    }

    #[test]
    fn unknown_engine_is_a_named_error() {
        let registry: SessionRegistry<TextFrame> = SessionRegistry::new();
        let err = match registry.spawn("nope", &SessionSpawnRequest::new("x")) {
            Ok(_) => panic!("unknown engine must not spawn"),
            Err(err) => err,
        };
        assert_eq!(err, SessionError::EngineNotFound("nope".into()));
    }

    #[test]
    fn downcast_reaches_lane_extras() {
        let mut registry = SessionRegistry::new();
        registry.register(Box::new(EchoSessionEngine));
        let mut session = registry
            .spawn("echo.session", &SessionSpawnRequest::new("a"))
            .unwrap();
        session.set_hidden(true);
        let echo = session
            .as_any()
            .downcast_mut::<EchoSession>()
            .expect("concrete lane type reachable");
        assert!(echo.hidden);
    }

    #[test]
    fn kind_index_reports_flags_not_a_single_kind() {
        let mut sessions: SessionRegistry<TextFrame> = SessionRegistry::new();
        sessions.register(Box::new(EchoSessionEngine));

        // An id may be held by more than one kind (block engine for cards +
        // session engine for tiles); the index reports flags, host picks.
        let index = EngineKindIndex::build(
            ["nematic.gemtext"],
            sessions.engine_ids().chain(["nematic.gemtext"]),
            ["scrying.web"],
        );
        assert!(index.kinds_of("echo.session").session);
        let both = index.kinds_of("nematic.gemtext");
        assert!(both.document && both.session && !both.surface);
        assert!(index.kinds_of("scrying.web").surface);
        assert!(!index.kinds_of("absent").any());
    }
}
