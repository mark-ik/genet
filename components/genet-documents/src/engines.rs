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

use genet_host_api::ResourceFetcher;
use genet_layout::ScrollKey;
use inker::session_engine::{
    DocumentSession, SessionClick, SessionEngine, SessionError, SessionLink, SessionScrollKey,
    SessionSpawnRequest,
};
#[cfg(feature = "livery")]
use layout_dom_api::LayoutDom;
use netrender::Scene;

use crate::document::{ClickOutcome, LoadedDocument};

/// Map the host-neutral scroll-key vocabulary onto genet-layout's.
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

// ── Static lane (genet.web) ───────────────────────────────────────────────

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
        inker::routing::ENGINE_GENET_WEB
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
    /// The structural report, through the trait: this session type is private,
    /// so a host cannot take the `as_any` detour meerkat uses on its own types
    /// (the accessor merecat's Inspector pane needs — rung-5 plan, slice F's
    /// "genet ask").
    fn inspect(&self) -> Option<inker::ContentReport> {
        Some(self.doc.inspect())
    }
    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

// ── Clean-room static lane (genet.livery) ────────────────────────────────

/// Opt-in session engine for the clean-room Livery CSS/layout path.
#[cfg(feature = "livery")]
pub struct LiverySessionEngine<Fetch> {
    fetcher: Fetch,
    author_css: Vec<String>,
}

#[cfg(feature = "livery")]
impl<Fetch> LiverySessionEngine<Fetch> {
    pub fn new(fetcher: Fetch) -> Self {
        Self {
            fetcher,
            author_css: Vec::new(),
        }
    }

    /// Add host-supplied author sheets before the document's own inline
    /// sheets. This keeps lane policy configurable at registration time.
    pub fn with_author_css(
        fetcher: Fetch,
        sheets: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            fetcher,
            author_css: sheets.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(feature = "livery")]
impl<Fetch: ResourceFetcher + Send + Sync> SessionEngine<Scene> for LiverySessionEngine<Fetch> {
    fn engine_id(&self) -> &str {
        inker::routing::ENGINE_GENET_LIVERY
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        let base_resource = request
            .address
            .split_once('#')
            .map_or(request.address.as_str(), |(resource, _)| resource)
            .to_owned();
        let source = match &request.body {
            Some(body) => body.clone(),
            None => {
                let bytes = self.fetcher.fetch(&base_resource).ok_or_else(|| {
                    SessionError::SpawnFailed(format!("could not load {base_resource}"))
                })?;
                String::from_utf8_lossy(&bytes).into_owned()
            },
        };
        let dom = genet_static_dom::StaticDocument::parse(&source);
        let mut sheets = self.author_css.clone();
        sheets.extend(genet_layout::inline_stylesheets(&dom));
        let sheet_refs = sheets.iter().map(String::as_str).collect::<Vec<_>>();
        let (width, height) = request.viewport;
        let mut doc = genet_livery::LiveryDocument::new(
            dom,
            genet_livery::StyleSet::cambium(&sheet_refs),
            genet_livery::Device::screen(width as f32, height as f32),
        );
        for authored_url in livery_resource_urls(doc.dom(), &sheets) {
            if authored_url.starts_with("data:") || authored_url.starts_with('#') {
                continue;
            }
            let resolved_url = resolve_livery_resource_url(&base_resource, &authored_url);
            if let Some(bytes) = self.fetcher.fetch(&resolved_url) {
                doc.set_image_resource(authored_url.clone(), bytes.clone());
                if resolved_url != authored_url {
                    doc.set_image_resource(resolved_url, bytes);
                }
            }
        }
        Ok(Box::new(LiveryDocumentSession {
            doc,
            last_error: None,
        }))
    }
}

#[cfg(feature = "livery")]
fn resolve_livery_resource_url(base: &str, authored: &str) -> String {
    url::Url::parse(base)
        .ok()
        .and_then(|base| base.join(authored).ok())
        .map_or_else(|| authored.to_owned(), |resolved| resolved.to_string())
}

#[cfg(feature = "livery")]
fn livery_resource_urls(dom: &genet_static_dom::StaticDocument, sheets: &[String]) -> Vec<String> {
    let mut urls = Vec::new();
    for sheet in sheets {
        let lower = sheet.to_ascii_lowercase();
        let mut cursor = 0;
        while let Some(offset) = lower[cursor..].find("url(") {
            let start = cursor + offset + 4;
            let Some(close) = sheet[start..].find(')') else {
                break;
            };
            let raw = sheet[start..start + close].trim();
            let authored = raw
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .or_else(|| {
                    raw.strip_prefix('\'')
                        .and_then(|value| value.strip_suffix('\''))
                })
                .unwrap_or(raw)
                .trim();
            if !authored.is_empty() && !urls.iter().any(|seen| seen == authored) {
                urls.push(authored.to_owned());
            }
            cursor = start + close + 1;
        }
    }

    let mut stack = vec![dom.document()];
    while let Some(id) = stack.pop() {
        if dom
            .element_name(id)
            .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
        {
            if let Some(src) = dom.attributes(id).find_map(|attribute| {
                (attribute.name.ns.as_ref().is_empty()
                    && attribute.name.local.as_ref().eq_ignore_ascii_case("src"))
                .then_some(attribute.value)
            }) {
                if !src.is_empty() && !urls.iter().any(|seen| seen == src) {
                    urls.push(src.to_owned());
                }
            }
        }
        let children = dom.dom_children(id).collect::<Vec<_>>();
        stack.extend(children.into_iter().rev());
    }
    urls
}

/// Retained Livery document session. The document owns the resolved style and
/// fragment planes, so this adapter only translates the session contract.
#[cfg(feature = "livery")]
pub struct LiveryDocumentSession {
    doc: genet_livery::LiveryDocument<genet_static_dom::StaticDocument>,
    last_error: Option<String>,
}

#[cfg(feature = "livery")]
impl LiveryDocumentSession {
    pub fn document(&self) -> &genet_livery::LiveryDocument<genet_static_dom::StaticDocument> {
        &self.doc
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }
}

#[cfg(feature = "livery")]
impl DocumentSession<Scene> for LiveryDocumentSession {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        match self.doc.frame(width, height) {
            Ok(list) => {
                self.last_error = None;
                paint_list_render::translate_paint_list(&list)
            },
            Err(error) => {
                self.last_error = Some(error.to_string());
                Scene::new(width, height)
            },
        }
    }

    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        self.doc.scroll_by(dx, dy)
    }

    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        self.doc.scroll_at(x, y, dx, dy)
    }

    fn scroll_for_key(&mut self, key: SessionScrollKey) -> bool {
        match key {
            SessionScrollKey::LineUp => self.doc.scroll_line(-1),
            SessionScrollKey::LineDown => self.doc.scroll_line(1),
            SessionScrollKey::PageUp => self.doc.scroll_page(-1),
            SessionScrollKey::PageDown => self.doc.scroll_page(1),
            SessionScrollKey::Home => {
                let before = self.doc.scroll();
                self.doc.scroll_to(0.0);
                before != self.doc.scroll()
            },
            SessionScrollKey::End => {
                let before = self.doc.scroll();
                self.doc.scroll_to(f32::MAX);
                before != self.doc.scroll()
            },
        }
    }

    fn scroll_to(&mut self, y: f32) {
        self.doc.scroll_to(y);
    }

    fn click_at(&mut self, x: f32, y: f32) -> SessionClick {
        match self.doc.click_at(x, y) {
            genet_livery::ClickOutcome::None => SessionClick::Miss,
            genet_livery::ClickOutcome::Focused | genet_livery::ClickOutcome::Scrolled => {
                SessionClick::Handled
            },
            genet_livery::ClickOutcome::Navigate(href) => SessionClick::Navigate(href),
        }
    }

    fn links(&self) -> Vec<SessionLink> {
        self.doc
            .links()
            .into_iter()
            .map(|link| SessionLink {
                url: link.url,
                rect: link.rect,
            })
            .collect()
    }

    fn content_height(&mut self, _width: u32, height: u32) -> u32 {
        self.doc.content_height(height)
    }

    fn pump(&mut self, now_ms: f64) {
        self.doc.pump(now_ms);
    }

    fn settled(&mut self) -> bool {
        self.doc.settled()
    }

    /// The structural report through the trait, off the Livery document's own
    /// DOM — the same read the static lane serves, so a viewer-pinned livery
    /// session inspects (and a11y-projects) instead of answering "none for
    /// this lane".
    fn inspect(&self) -> Option<inker::ContentReport> {
        Some(genet_render::content_report(self.doc.dom()))
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

// ── Scripted lane (genet.scripted / genet.scripted.nova) ────────────────

/// Session engine for the scripted lane, generic over the JS engine `E` (the
/// per-engine monomorphization genet-scripted already uses: the host
/// registers `ScriptedSessionEngine::<BoaEngine, _>` under `genet.scripted`
/// and, on 64-bit targets with the `scripted-nova` feature,
/// `ScriptedSessionEngine::<NovaEngine, _>` under `genet.scripted.nova`).
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
    Fetch: genet_scripted::ResourceFetcher + Send + Sync,
{
    fn engine_id(&self) -> &str {
        &self.engine_id
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        let doc = match &request.body {
            Some(body) => genet_scripted::ScriptedDocument::<E>::from_body(
                body,
                &self.fetcher,
                &request.address,
                None,
            ),
            None => genet_scripted::ScriptedDocument::<E>::load(&self.fetcher, &request.address),
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
    doc: genet_scripted::ScriptedDocument<E>,
}

#[cfg(feature = "scripted")]
impl<E: script_engine_api::ScriptEngine + 'static> ScriptedDocumentSession<E> {
    pub fn new(doc: genet_scripted::ScriptedDocument<E>) -> Self {
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
    pub fn document_mut(&mut self) -> &mut genet_scripted::ScriptedDocument<E> {
        &mut self.doc
    }
}

// ── Smolweb engine-native document lane (per-format ids) ──────────────────

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
            Some(body) => crate::SmolwebDocument::parse(&request.address, body, self.theme.clone()),
            None => {
                crate::SmolwebDocument::load(&self.fetcher, &request.address, self.theme.clone())
                    .map_err(SessionError::SpawnFailed)?
            },
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

    /// The concrete document, for observation downcasts and host-side
    /// banding/link-table inspection.
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
    #[cfg(feature = "livery")]
    use std::sync::{Arc, Mutex};

    /// Byte source for spawn-with-body tests; never fetches.
    struct NoFetch;
    impl ResourceFetcher for NoFetch {
        fn fetch(&self, _url: &str) -> Option<Vec<u8>> {
            None
        }
    }

    #[cfg(feature = "livery")]
    struct ImageFetch {
        bytes: Vec<u8>,
        requests: Arc<Mutex<Vec<String>>>,
    }

    #[cfg(feature = "livery")]
    impl ResourceFetcher for ImageFetch {
        fn fetch(&self, url: &str) -> Option<Vec<u8>> {
            self.requests.lock().unwrap().push(url.to_owned());
            Some(self.bytes.clone())
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
            .spawn(inker::routing::ENGINE_GENET_WEB, &request)
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

    /// The structural report is reachable THROUGH THE TRAIT — the accessor a
    /// host without downcast access (merecat: this session type is private)
    /// stands on. Title, links, and headings come back from the live session.
    #[test]
    fn static_session_reports_structure_through_the_trait() {
        let engine = StaticSessionEngine::new(NoFetch);
        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(
                "<html><head><title>The Page</title></head>\
                 <body><h1>Heading</h1><a href=\"/next\">next</a></body></html>",
            )
            .with_viewport(640, 480);
        let session = engine.spawn(&request).expect("spawns");
        let report = session
            .inspect()
            .expect("the static lane has a structural read");
        assert_eq!(report.title.as_deref(), Some("The Page"));
        assert_eq!(report.headings, vec!["Heading"]);
        assert_eq!(report.links, vec!["/next"]);
    }

    #[test]
    fn static_session_scrolls_long_content() {
        let engine = StaticSessionEngine::new(NoFetch);
        let body = format!("<html><body>{}</body></html>", "<p>line</p>".repeat(200));
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

    #[cfg(feature = "livery")]
    #[test]
    fn livery_session_routes_retained_structural_and_text_paint() {
        let mut registry: SessionRegistry<Scene> = SessionRegistry::new();
        registry.register(Box::new(LiverySessionEngine::new(NoFetch)));
        assert!(registry.contains(inker::routing::ENGINE_GENET_LIVERY));

        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(
                r#"<html><head><style>.card { background-color: navy; color: white; width: 120px; }</style></head><body><div class="card">Livery <span>session</span></div></body></html>"#,
            )
            .with_viewport(320, 240);
        let mut session = registry
            .spawn(inker::routing::ENGINE_GENET_LIVERY, &request)
            .expect("registered Livery lane spawns from body");

        let first = session.frame(320, 240);
        assert!(
            first
                .ops
                .iter()
                .any(|operation| matches!(operation, netrender::SceneOp::Rect(_)))
        );
        assert!(
            first
                .ops
                .iter()
                .any(|operation| matches!(operation, netrender::SceneOp::GlyphRun(_)))
        );
        let concrete = session
            .as_any()
            .downcast_mut::<LiveryDocumentSession>()
            .expect("session keeps its concrete Livery owner");
        let generation = concrete.document().generation();
        let shape_count = concrete.document().text_system().shape_count();
        assert_eq!(concrete.last_error(), None);

        let _cached = session.frame(320, 240);
        let concrete = session
            .as_any()
            .downcast_mut::<LiveryDocumentSession>()
            .expect("session keeps its concrete Livery owner");
        assert_eq!(concrete.document().generation(), generation);
        assert_eq!(concrete.document().text_system().shape_count(), shape_count);
        assert!(!session.scroll_by(0.0, 100.0));
        assert_eq!(session.click_at(20.0, 20.0), SessionClick::Miss);
    }

    /// The livery lane's structural report through the trait — the same
    /// contract the static lane serves, so a viewer override to livery keeps
    /// the Inspector/a11y read instead of degrading to "none for this lane".
    #[cfg(feature = "livery")]
    #[test]
    fn livery_session_reports_structure_through_the_trait() {
        let engine = LiverySessionEngine::new(NoFetch);
        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(
                "<html><head><title>The Page</title></head>\
                 <body><h1>Heading</h1><a href=\"/next\">next</a></body></html>",
            )
            .with_viewport(640, 480);
        let session = engine.spawn(&request).expect("livery lane spawns");
        let report = session
            .inspect()
            .expect("the livery lane has a structural read");
        assert_eq!(report.title.as_deref(), Some("The Page"));
        assert_eq!(report.headings, vec!["Heading"]);
        assert_eq!(report.links, vec!["/next"]);
    }

    #[cfg(feature = "livery")]
    #[test]
    fn livery_session_routes_scroll_focus_and_links() {
        let engine = LiverySessionEngine::new(NoFetch);
        let request = SessionSpawnRequest::new("https://example.test/")
            .with_body(
                r##"<html><head><style>
                    html, body { margin: 0; padding: 0; }
                    .link, .target { display: block; width: 100px; height: 20px; }
                    .spacer { height: 500px; }
                </style></head><body>
                    <a class="link" href="#target">top</a>
                    <div class="spacer"></div>
                    <div id="target" class="target">target</div>
                </body></html>"##,
            )
            .with_viewport(320, 240);
        let mut session = engine.spawn(&request).expect("spawns");
        let _scene = session.frame(320, 240);

        assert!(session.content_height(320, 240) > 240);
        let link = session.links().into_iter().next().expect("retained link");
        let click = session.click_at(link.rect[0] + 5.0, link.rect[1] + 5.0);
        assert_eq!(click, SessionClick::Handled);
        assert!(session.scroll_for_key(SessionScrollKey::Home));
        assert!(session.scroll_by(0.0, 100.0));
        assert!(session.scroll_at(10.0, 10.0, 0.0, -100.0));
    }

    #[cfg(feature = "livery")]
    #[test]
    fn livery_session_fetches_remote_image_resources_through_the_host() {
        let image = image::RgbaImage::from_pixel(2, 3, image::Rgba([0, 0, 255, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode test PNG");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = LiverySessionEngine::new(ImageFetch {
            bytes,
            requests: requests.clone(),
        });
        let request = SessionSpawnRequest::new("https://example.test/docs/index.html")
            .with_body(
                r#"<html><head><style>
                    .card { display: block; width: 80px; height: 40px;
                            background-repeat: no-repeat;
                            background-image: url(hero.png); }
                </style></head><body><div class="card"></div></body></html>"#,
            )
            .with_viewport(320, 240);
        let mut session = engine.spawn(&request).expect("Livery lane spawns");

        let scene = session.frame(320, 240);
        assert!(
            scene
                .ops
                .iter()
                .any(|operation| matches!(operation, netrender::SceneOp::Image(_))),
            "host-fetched image reaches the scene"
        );
        assert_eq!(
            requests.lock().unwrap().as_slice(),
            ["https://example.test/docs/hero.png"]
        );
    }
}
