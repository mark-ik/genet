/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The lanes as inker session engines: `SessionEngine<Scene>` /
//! `DocumentSession<Scene>` impls wrapping [`LoadedDocument`], the scripted
//! document, and [`SmolwebDocument`](crate::SmolwebDocument).
//!
//! Construction seams (fetchers, themes, cookie jars) live on the engine at
//! registration time; the spawn request stays plain data (session-engines
//! plan, review-resolved 2026-07-10).

use std::any::Any;

use inker::session_engine::{
    DocumentSession, SessionClick, SessionEngine, SessionError, SessionLink, SessionScrollKey,
    SessionSpawnRequest,
};
use netrender::Scene;
use pelt_core::ResourceFetcher;
use serval_layout::ScrollKey;

use crate::document::{ClickOutcome, LoadedDocument};

/// Map the host-neutral scroll-key vocabulary onto serval-layout's.
pub(crate) fn layout_scroll_key(key: SessionScrollKey) -> ScrollKey {
    match key {
        SessionScrollKey::LineUp => ScrollKey::Up,
        SessionScrollKey::LineDown => ScrollKey::Down,
        SessionScrollKey::PageUp => ScrollKey::PageUp,
        SessionScrollKey::PageDown => ScrollKey::PageDown,
        SessionScrollKey::Home => ScrollKey::Home,
        SessionScrollKey::End => ScrollKey::End,
    }
}

/// Map the static lane's click outcome onto the unified enum. The host
/// resolves a relative href against the current URL (see
/// [`resolve_href`](crate::href::resolve_href)), same contract as today.
pub fn session_click_from_outcome(outcome: ClickOutcome) -> SessionClick {
    match outcome {
        ClickOutcome::None => SessionClick::Miss,
        ClickOutcome::Scrolled => SessionClick::Handled,
        ClickOutcome::Navigate(href) => SessionClick::Navigate(href),
    }
}

// ── Static lane (serval.web) ───────────────────────────────────────────────

/// Session engine for the static HTML lane. Holds the shell's fetcher.
pub struct StaticSessionEngine<Fetch> {
    fetcher: Fetch,
}

impl<Fetch> StaticSessionEngine<Fetch> {
    pub fn new(fetcher: Fetch) -> Self {
        Self { fetcher }
    }
}

impl<Fetch: ResourceFetcher + Send + Sync> SessionEngine<Scene> for StaticSessionEngine<Fetch> {
    fn engine_id(&self) -> &str {
        inker::routing::ENGINE_SERVAL_WEB
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        let doc = match &request.body {
            Some(body) => LoadedDocument::parse(body),
            None => LoadedDocument::load(&self.fetcher, &request.address)
                .map_err(SessionError::SpawnFailed)?,
        };
        Ok(Box::new(StaticDocumentSession { doc }))
    }
}

struct StaticDocumentSession {
    doc: LoadedDocument,
}

impl DocumentSession<Scene> for StaticDocumentSession {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.doc.frame(width, height)
    }
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        self.doc.scroll_by(dx, dy)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        self.doc.scroll_at(x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool {
        self.doc.scroll_for_key(layout_scroll_key(key))
    }
    fn click_at(&mut self, x: f32, y: f32) -> SessionClick {
        session_click_from_outcome(self.doc.click_at(x, y))
    }
    /// The static lane resolves links through hit-testing (`click_at`); a
    /// retained link table is additive follow-up, so the table is empty
    /// rather than pretended.
    fn links(&self) -> Vec<SessionLink> {
        Vec::new()
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

// ── Scripted lane (serval.scripted / serval.scripted.nova) ────────────────

/// Session engine for the scripted lane, generic over the JS engine `E` (the
/// per-engine monomorphization serval-scripted already uses: the host
/// registers `ScriptedSessionEngine::<BoaEngine, _>` under `serval.scripted`
/// and, on 64-bit targets with the `scripted-nova` feature,
/// `ScriptedSessionEngine::<NovaEngine, _>` under `serval.scripted.nova`).
/// Holds the shell's fetcher for external `<script src>` resolution.
#[cfg(feature = "scripted")]
pub struct ScriptedSessionEngine<E, Fetch> {
    engine_id: String,
    fetcher: Fetch,
    _engine: std::marker::PhantomData<fn() -> E>,
}

#[cfg(feature = "scripted")]
impl<E, Fetch> ScriptedSessionEngine<E, Fetch> {
    pub fn new(engine_id: impl Into<String>, fetcher: Fetch) -> Self {
        Self {
            engine_id: engine_id.into(),
            fetcher,
            _engine: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "scripted")]
impl<E, Fetch> SessionEngine<Scene> for ScriptedSessionEngine<E, Fetch>
where
    E: script_engine_api::ScriptEngine + 'static,
    Fetch: serval_scripted::ResourceFetcher + Send + Sync,
{
    fn engine_id(&self) -> &str {
        &self.engine_id
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        let doc = match &request.body {
            Some(body) => serval_scripted::ScriptedDocument::<E>::from_body(
                body,
                &self.fetcher,
                &request.address,
                None,
            ),
            None => serval_scripted::ScriptedDocument::<E>::load(&self.fetcher, &request.address),
        }
        .map_err(SessionError::SpawnFailed)?;
        let mut session = ScriptedDocumentSession { doc };
        if request.hidden {
            session.doc.set_hidden(true);
        }
        Ok(Box::new(session))
    }
}

/// The scripted document as a session. Public so a host with richer
/// construction seams (per-spawn fetchers, cookie jars) builds the document
/// itself and wraps it; the engine above is the simple-seam path.
#[cfg(feature = "scripted")]
pub struct ScriptedDocumentSession<E: script_engine_api::ScriptEngine> {
    doc: serval_scripted::ScriptedDocument<E>,
}

#[cfg(feature = "scripted")]
impl<E: script_engine_api::ScriptEngine + 'static> ScriptedDocumentSession<E> {
    pub fn new(doc: serval_scripted::ScriptedDocument<E>) -> Self {
        Self { doc }
    }
}

#[cfg(feature = "scripted")]
impl<E: script_engine_api::ScriptEngine + 'static> DocumentSession<Scene>
    for ScriptedDocumentSession<E>
{
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.doc.frame(width, height)
    }
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        self.doc.scroll_by(dx, dy)
    }
    fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool {
        self.doc.scroll_for_key(layout_scroll_key(key))
    }
    fn click_at(&mut self, x: f32, y: f32) -> SessionClick {
        // The scripted lane's bool is "a handler consumed it"; navigation
        // flows through the links table, same as the host does today.
        if self.doc.click_at(x, y) {
            SessionClick::Handled
        } else {
            SessionClick::Miss
        }
    }
    fn links(&self) -> Vec<SessionLink> {
        self.doc
            .links()
            .into_iter()
            .map(|(url, rect)| SessionLink { url, rect })
            .collect()
    }
    fn pump(&mut self, now_ms: f64) {
        let _ = self.doc.pump(now_ms);
    }
    fn settled(&mut self) -> bool {
        !self.doc.has_pending_work()
    }
    fn set_hidden(&mut self, hidden: bool) {
        self.doc.set_hidden(hidden);
    }
    /// Observation extras (extract, dom_snapshot, dispatch_event, dom stats)
    /// stay on the concrete type until the observation contract lands
    /// (session-engines plan phase 3 rescope); hosts reach them here.
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(feature = "scripted")]
impl<E: script_engine_api::ScriptEngine> ScriptedDocumentSession<E> {
    /// The concrete document, for observation downcasts (phase 3 rescope:
    /// extract / dom_snapshot / dispatch_event stay concrete until the
    /// observation contract lands).
    pub fn document_mut(&mut self) -> &mut serval_scripted::ScriptedDocument<E> {
        &mut self.doc
    }
}

// ── Smolweb native lane (per-format ids) ───────────────────────────────────

/// Session engine for the smolweb native lane. One instance per format id
/// (`nematic.gemtext` / `nematic.gopher` / `nematic.feed` today) so routing
/// decisions map directly; the same ids keep their block engines for cards —
/// the kind index reports both and the host picks by surface context.
#[cfg(feature = "smolweb")]
pub struct SmolwebSessionEngine<Fetch> {
    engine_id: String,
    fetcher: Fetch,
    theme: crate::SmolwebTheme,
}

#[cfg(feature = "smolweb")]
impl<Fetch> SmolwebSessionEngine<Fetch> {
    pub fn new(engine_id: impl Into<String>, fetcher: Fetch, theme: crate::SmolwebTheme) -> Self {
        Self {
            engine_id: engine_id.into(),
            fetcher,
            theme,
        }
    }
}

#[cfg(feature = "smolweb")]
impl<Fetch: ResourceFetcher + Send + Sync> SessionEngine<Scene> for SmolwebSessionEngine<Fetch> {
    fn engine_id(&self) -> &str {
        &self.engine_id
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        let doc = match &request.body {
            Some(body) => {
                crate::SmolwebDocument::parse(&request.address, body, self.theme.clone())
            }
            None => {
                crate::SmolwebDocument::load(&self.fetcher, &request.address, self.theme.clone())
                    .map_err(SessionError::SpawnFailed)?
            }
        };
        Ok(Box::new(SmolwebDocumentSession {
            doc,
            viewport: request.viewport,
        }))
    }
}

/// The smolweb document as a session. Public so a host that themes per
/// content (meerkat's palette-derived themes) parses the document itself and
/// wraps it; the engine above is the fixed-theme path.
#[cfg(feature = "smolweb")]
pub struct SmolwebDocumentSession {
    doc: crate::SmolwebDocument,
    /// Last framed size: the lane's click/content-height APIs take the
    /// viewport, which the trait carries implicitly through `frame`.
    viewport: (u32, u32),
}

#[cfg(feature = "smolweb")]
impl SmolwebDocumentSession {
    pub fn new(doc: crate::SmolwebDocument, viewport: (u32, u32)) -> Self {
        Self { doc, viewport }
    }

    /// The concrete document, for observation downcasts (link tables with
    /// host-side banding math, DOM handles).
    pub fn document_mut(&mut self) -> &mut crate::SmolwebDocument {
        &mut self.doc
    }
}

#[cfg(feature = "smolweb")]
impl DocumentSession<Scene> for SmolwebDocumentSession {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.viewport = (width, height);
        self.doc.frame(width, height)
    }
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        self.doc.scroll_by(dx, dy)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        self.doc.scroll_at(x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool {
        self.doc.scroll_for_key(layout_scroll_key(key))
    }
    fn scroll_to(&mut self, y: f32) {
        self.doc.scroll_to(y);
    }
    fn click_at(&mut self, x: f32, y: f32) -> SessionClick {
        let (w, h) = self.viewport;
        match self.doc.click_at(x, y, w, h) {
            Some(url) => SessionClick::Navigate(url),
            None => SessionClick::Miss,
        }
    }
    fn links(&self) -> Vec<SessionLink> {
        self.doc
            .links()
            .into_iter()
            .map(|(url, rect)| SessionLink { url, rect })
            .collect()
    }
    fn content_height(&mut self, width: u32, height: u32) -> u32 {
        self.doc.content_height(width, height)
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inker::session_engine::SessionRegistry;

    /// Byte source for spawn-with-body tests; never fetches.
    struct NoFetch;
    impl ResourceFetcher for NoFetch {
        fn fetch(&self, _url: &str) -> Option<Vec<u8>> {
            None
        }
    }

    #[test]
    fn static_session_spawns_from_body_and_navigates() {
        let mut registry: SessionRegistry<Scene> = SessionRegistry::new();
        registry.register(Box::new(StaticSessionEngine::new(NoFetch)));

        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(r#"<html><body><a href="/next">next</a></body></html>"#)
            .with_viewport(640, 480);
        let mut session = registry
            .spawn(inker::routing::ENGINE_SERVAL_WEB, &request)
            .expect("static lane spawns from body");

        let _scene = session.frame(640, 480);
        assert!(session.settled(), "static lane is always settled");

        // The anchor is the document's first (only) inline box: probe a few
        // points inside the first line rather than betting on font metrics.
        let click = [(12.0, 14.0), (14.0, 18.0), (10.0, 12.0), (20.0, 16.0)]
            .into_iter()
            .map(|(x, y)| session.click_at(x, y))
            .find(|c| *c != SessionClick::Miss)
            .expect("a probe point lands on the only link");
        match click {
            SessionClick::Navigate(href) => assert_eq!(href, "/next"),
            other => panic!("expected the link to navigate, got {other:?}"),
        }
    }

    #[test]
    fn static_session_scrolls_long_content() {
        let engine = StaticSessionEngine::new(NoFetch);
        let body = format!(
            "<html><body>{}</body></html>",
            "<p>line</p>".repeat(200)
        );
        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(&body)
            .with_viewport(320, 240);
        let mut session = engine.spawn(&request).expect("spawns");
        let _ = session.frame(320, 240);
        assert!(session.scroll_by(0.0, 120.0), "long content scrolls");
        assert!(
            session.scroll_for_key(SessionScrollKey::Home),
            "home returns to the top"
        );
    }
}
