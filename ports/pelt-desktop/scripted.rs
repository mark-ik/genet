/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The scripted document profile (V4): a live, script-mutated document.
//!
//! The content half of `pelt --engine scripted <url>`: a [`ScriptedDocument`] that
//! parses HTML into a live [`ScriptedDom`], runs its `<script>`s — inline *and*
//! external `<script src>` (fetched through the same [`ResourceFetcher`] the page
//! loaded over, in document order) — through [`script_runtime_api::Runtime`] on a
//! chosen JS engine (Boa by default, Nova behind `scripted-nova`), and renders the
//! *mutated* DOM each frame through `serval-render`. GPU-free and testable here; the
//! windowed present loop drives it exactly as it drives the static `LoadedDocument`
//! (the `document` module, present under `tile-surface`).
//!
//! Script timing follows the classic-script model: parser-blocking scripts (inline,
//! and external with neither `async` nor `defer`) run in document order; `defer` and
//! `async` external scripts run after that pass (`defer` in document order — the
//! guaranteed contract; `async` is unordered, and since the fetcher is synchronous,
//! document order is a faithful realization). `async`/`defer` are ignored on inline
//! scripts, per spec. A `type` that is neither empty nor a JavaScript MIME type nor
//! `module` is a data block and is not executed. `type=module` scripts (inline or
//! `src`) are **deferred** (run after the parser-blocking pass, in document order)
//! and evaluated with module scope via the engine's module path (`eval_module`); a
//! backend without module support logs and skips. Cross-module `import` works on a
//! module-capable backend: the engine's loader resolves each specifier against the
//! importing module's URL and pulls its source through this document's fetcher (the
//! `resolve` closure below), caching by URL so a diamond / cycle loads once. An
//! unresolvable or throwing import rejects the module, which is reported and skipped.
//! A failed/missing/integrity-rejected external script is likewise reported and
//! skipped, like an inline error, and the document keeps rendering.
//!
//! The script/layout split (recorded on [`script_runtime_api::HostState`]): the host
//! owns the real viewport. Each frame it syncs the current scroll *into* the runtime
//! (so `window.scrollX|Y` read true values), lays the live DOM out, reconciles back
//! the scroll script set (`scrollTo`/`scrollBy`) or the element it asked for
//! (`scrollIntoView`) against the real scroll range, and renders. The GC tick
//! (`Runtime::collect_garbage`) runs at frame cadence in [`ScriptedDocument::pump`] —
//! the first real frame-cadence caller the gc-arena plan was waiting on.

use layout_dom_api::{LayoutDom, LocalName, Namespace};
use netrender::Scene;
use pelt_core::ResourceFetcher;
use std::cell::RefCell;
use std::rc::Rc;

use script_engine_api::ScriptEngine;
use script_runtime_api::{ComputedStyleHandler, Runtime};
use serval_layout::{inline_stylesheets, IncrementalLayout, ScrollKey, ScrollOffsets};
use serval_render::scene_from_session_dom;
use serval_scripted_dom::NodeId;
use serval_static_dom::{StaticDocument, StaticNodeId};

/// Shared handle to the most recently laid-out frame, so the `getComputedStyle`
/// bridge can read computed values off it. `None` before the first frame.
type RetainedLayout = Rc<RefCell<Option<IncrementalLayout<NodeId>>>>;

/// The host side of `script_runtime_api`'s computed-style seam: serves
/// `getComputedStyle` reads off the last rendered frame's cascade. One frame
/// stale by construction (script runs before layout), the standard tradeoff for
/// the split; `None` (no frame yet / unstyled / unsupported longhand) -> "".
struct ComputedStyleBridge {
    layout: RetainedLayout,
}

impl ComputedStyleHandler for ComputedStyleBridge {
    fn computed_value(&self, node: u64, property: &str) -> Option<String> {
        self.layout.borrow().as_ref()?.computed_value(NodeId::from_raw(node as usize), property)
    }
}

/// A live document driven by script: a [`Runtime`] holding the mutable DOM, plus the
/// host-owned viewport scroll the runtime mirrors. Generic over the JS engine `E`
/// (the monomorphization the `--engine` selection picks, exactly as serval-wpt's
/// harness does); the bin instantiates `ScriptedDocument<BoaEngine>` or
/// `ScriptedDocument<NovaEngine>`.
pub struct ScriptedDocument<E: ScriptEngine> {
    /// The engine + browser host surface; owner of the live [`ScriptedDom`] that the
    /// page's script mutates and that every frame renders.
    rt: Runtime<E>,
    /// Structural UA defaults plus the document's inline `<style>` sheets, resolved
    /// once at load. (Script-added stylesheets are a follow-up.)
    sheets: Vec<String>,
    /// The host-owned document scroll in device px — the authority the runtime's
    /// `viewport_scroll` mirror is synced from/to each frame.
    scroll: (f32, f32),
    /// The scrollable-overflow extent from the last laid-out frame, so a wheel
    /// `scroll_by` between frames clamps without re-running layout. The next frame
    /// re-clamps exactly against the freshly laid-out range.
    scroll_range: (f32, f32),
    /// The size the document was last laid out at (`(0, 0)` before the first frame),
    /// so keyboard / click scrolling can build a transient layout at the right size.
    size: (u32, u32),
    /// A `url#id` fragment to scroll to once on the first frame (anchor-fragment
    /// navigation on load); cleared after applying.
    pending_fragment: Option<String>,
    /// The last rendered frame's layout, shared with the `getComputedStyle` bridge
    /// so script reads computed values off the most recent cascade.
    layout: RetainedLayout,
}

impl<E: ScriptEngine> ScriptedDocument<E> {
    /// Fetch `url` through `fetcher`, parse it, and run its scripts — inline and
    /// external `<script src>` (each resolved against `url` and fetched through the
    /// same `fetcher`). `Err` on a failed fetch of the document, or a runtime that
    /// would not initialize.
    pub fn load(fetcher: &impl ResourceFetcher, url: &str) -> Result<Self, String> {
        // Split a `url#id` fragment off before fetching (the fetcher takes the
        // resource, not the fragment).
        let (resource, fragment) = match url.split_once('#') {
            Some((res, frag)) => (res, (!frag.is_empty()).then(|| frag.to_string())),
            None => (url, None),
        };
        let bytes = fetcher
            .fetch(resource)
            .ok_or_else(|| format!("could not load {resource}"))?;
        // External scripts resolve against the document URL and fetch through the
        // same fetcher; pass both into the builder.
        let mut me = Self::build(&String::from_utf8_lossy(&bytes), Some((fetcher, resource)))?;
        me.pending_fragment = fragment;
        Ok(me)
    }

    /// Parse already-loaded HTML into a live DOM, then run its **inline** `<script>`s
    /// against it (settling microtasks). The fetch-free half, for tests and inline
    /// `data:` content — with no fetcher, external `<script src>` is reported and
    /// skipped. `Err` if the runtime fails to initialize.
    pub fn parse(html: &str) -> Result<Self, String> {
        Self::build(html, None)
    }

    /// Parse `html` into a live DOM and run its scripts in document order. With a
    /// `loader` (`(fetcher, base_url)`), external `<script src>` is resolved against
    /// `base_url` and fetched; without one (the [`parse`](Self::parse) path), an
    /// external script is reported and skipped. A script that errors (or whose fetch
    /// fails) is reported but does not abort the load — a browser keeps rendering the
    /// document. `Err` only if the runtime fails to initialize.
    fn build(html: &str, loader: Option<(&dyn ResourceFetcher, &str)>) -> Result<Self, String> {
        let doc = StaticDocument::parse(html);
        let mut sheets: Vec<String> =
            crate::STRUCTURAL_SHEET.iter().map(|s| s.to_string()).collect();
        sheets.extend(inline_stylesheets(&doc));

        let mut rt = Runtime::<E>::new().map_err(|e| format!("script runtime init: {e:?}"))?;
        // The computed-style seam: a bridge over the most recent rendered frame's
        // cascade, so `getComputedStyle` returns real computed values (one frame
        // stale). Set before scripts run (they see "" until the first frame).
        let layout: RetainedLayout = Rc::new(RefCell::new(None));
        rt.set_computed_style_handler(Box::new(ComputedStyleBridge { layout: layout.clone() }));
        // The document URL is the base for reflected URL attributes (`a.href`,
        // `img.src`, …) and for resolving fetches; set it from the loader when present
        // (the `parse()` path has no URL, so those reflect their raw values).
        if let Some((_, base)) = loader {
            let _ = rt.set_base_url(base);
        }
        // The parsed body becomes the live DOM, so script querying it (document.body,
        // getElementById, querySelector) sees the page's elements.
        rt.load_dom(&doc);

        // Run scripts by the classic-script timing model. Parser-blocking pass:
        // inline (run now) and classic external with no async/defer (fetch + run now),
        // in document order. `defer`/`async` externals are queued and run after the
        // pass — `defer` keeps document order (its guarantee); `async` is unordered,
        // and document order is a faithful realization of our synchronous fetch.
        let scripts = collect_scripts(&doc);
        let mut deferred: Vec<&ScriptSource> = Vec::new();
        for script in &scripts {
            match script {
                ScriptSource::Inline(text) => eval_reporting(&mut rt, text),
                ScriptSource::External { src, timing: ScriptTiming::Blocking, charset, integrity } => {
                    if let Some(source) =
                        fetch_external(loader, src, charset.as_deref(), integrity.as_deref())
                    {
                        eval_reporting(&mut rt, &source);
                    }
                },
                // defer / async classic, and all modules: run after the parser-
                // blocking pass, in document order.
                ScriptSource::External { .. }
                | ScriptSource::ModuleInline(_)
                | ScriptSource::ModuleExternal { .. } => deferred.push(script),
            }
        }
        for script in deferred {
            match script {
                ScriptSource::External { src, charset, integrity, .. } => {
                    if let Some(source) =
                        fetch_external(loader, src, charset.as_deref(), integrity.as_deref())
                    {
                        eval_reporting(&mut rt, &source);
                    }
                },
                ScriptSource::ModuleInline(text) => {
                    // An inline module's imports resolve against the document URL.
                    let base = loader.map(|(_, page)| page.to_string()).unwrap_or_default();
                    eval_module_reporting(&mut rt, loader, &base, text);
                },
                ScriptSource::ModuleExternal { src, charset, integrity } => {
                    // An external module's imports resolve against its own URL.
                    let base = loader
                        .map(|(_, page)| crate::href::resolve_href(page, src))
                        .unwrap_or_default();
                    if let Some(source) =
                        fetch_external(loader, src, charset.as_deref(), integrity.as_deref())
                    {
                        eval_module_reporting(&mut rt, loader, &base, &source);
                    }
                },
                // Inline classic never defers.
                ScriptSource::Inline(_) => {},
            }
        }
        rt.run_microtasks();

        Ok(Self {
            rt,
            sheets,
            scroll: (0.0, 0.0),
            scroll_range: (0.0, 0.0),
            size: (0, 0),
            pending_fragment: None,
            layout,
        })
    }

    /// Drive the runtime one frame's worth: fire due timers against the `now_ms`
    /// virtual clock, settle microtasks, then take the GC tick. Returns
    /// `(reflectors_unpinned, nodes_collected)` from the collection. This is the
    /// frame-cadence caller of [`Runtime::collect_garbage`] (gc-arena carve-out #1):
    /// a long-lived document churning nodes under `setInterval` is collected here,
    /// not at an explicit one-off call.
    pub fn pump(&mut self, now_ms: f64) -> (usize, usize) {
        self.rt.run_timers(64, now_ms);
        self.rt.run_microtasks();
        self.rt.collect_garbage()
    }

    /// Render the live DOM to a [`Scene`] at `width`×`height`, laying it out and
    /// painting at the reconciled document scroll. Re-lays-out each frame because the
    /// DOM may have changed under script (a retain-until-dirty fast path is a
    /// follow-up).
    pub fn frame(&mut self, width: u32, height: u32) -> Scene {
        let (w, h) = (width.max(1), height.max(1));
        // Sync the host-owned scroll into the script-visible mirror, and take any
        // pending scrollIntoView target. Short mutable borrow, dropped before layout.
        let into_view = {
            let mut host = self.rt.host().borrow_mut();
            host.viewport_scroll = self.scroll;
            host.scroll_into_view.take()
        };
        let fragment = self.pending_fragment.take();

        // Lay the live DOM out and render (immutable borrow of the runtime's DOM).
        let host = self.rt.host().borrow();
        let dom = &host.dom;
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let mut session = IncrementalLayout::new(dom, &sheets, w as f32, h as f32);
        // Resolve the scroll for this frame: a one-shot load anchor, else a
        // script-requested element, else the carried document scroll (re-clamped).
        if let Some(frag) = fragment.as_deref() {
            session.scroll_to_id(dom, frag);
        } else if let Some(node) = into_view {
            session.scroll_to_element(dom, node);
        } else {
            session.set_viewport_scroll(dom, self.scroll);
        }
        let scroll = session.viewport_scroll();
        let range = session.scroll_range(dom);
        let scene = scene_from_session_dom(&session, dom, w, h);
        drop(host);

        // Retain this frame's cascade so the `getComputedStyle` bridge can read
        // computed values off it until the next frame replaces it.
        *self.layout.borrow_mut() = Some(session);
        self.scroll = scroll;
        self.scroll_range = range;
        self.size = (w, h);
        scene
    }

    /// Scroll by a device-px wheel delta, clamped to the last frame's scrollable
    /// range (no re-layout — the next frame reconciles exactly). Returns whether the
    /// offset moved. A no-op before the first [`frame`](Self::frame).
    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let nx = (self.scroll.0 + dx).clamp(0.0, self.scroll_range.0);
        let ny = (self.scroll.1 + dy).clamp(0.0, self.scroll_range.1);
        let moved = (nx, ny) != self.scroll;
        self.scroll = (nx, ny);
        moved
    }

    /// Apply a keyboard scroll default ([`ScrollKey`]) to the document viewport,
    /// through a transient layout at the last frame's size (so PageDown knows the
    /// page height). Returns whether the offset moved; a no-op before the first frame.
    pub fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        if self.size == (0, 0) {
            return false;
        }
        let (w, h) = self.size;
        let host = self.rt.host().borrow();
        let dom = &host.dom;
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let mut session = IncrementalLayout::new(dom, &sheets, w as f32, h as f32);
        session.set_viewport_scroll(dom, self.scroll);
        let moved = session.scroll_for_key(dom, key);
        let scroll = session.viewport_scroll();
        let range = session.scroll_range(dom);
        drop(host);
        self.scroll = scroll;
        self.scroll_range = range;
        moved
    }

    /// Handle a left click at scene point `(x, y)`: if it lands on an in-page link
    /// (`<a href="#id">`), scroll its target into view. Returns whether the document
    /// scrolled. A no-op before the first frame, or off a link. (Click → script event
    /// dispatch is a V4 follow-up; this keeps the static viewer's anchor-nav parity.)
    pub fn click_at(&mut self, x: f32, y: f32) -> bool {
        if self.size == (0, 0) {
            return false;
        }
        let (w, h) = self.size;
        let host = self.rt.host().borrow();
        let dom = &host.dom;
        let sheets: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
        let mut session = IncrementalLayout::new(dom, &sheets, w as f32, h as f32);
        session.set_viewport_scroll(dom, self.scroll);
        let moved = match session.link_fragment_at(dom, x, y, &ScrollOffsets::default()) {
            Some(frag) => session.scroll_to_id(dom, &frag),
            None => false,
        };
        let scroll = session.viewport_scroll();
        drop(host);
        self.scroll = scroll;
        moved
    }

    /// Whether the runtime has pending time-based work (a scheduled timer), so the
    /// shell should keep requesting frames. `setInterval` re-arms each fire, so a
    /// churning soak page stays animated; a quiescent page lets the loop idle.
    pub fn has_pending_work(&mut self) -> bool {
        self.rt.next_timer_delay().is_some()
    }

    /// The current document scroll offset in device px.
    pub fn scroll(&self) -> (f32, f32) {
        self.scroll
    }

    /// The number of live nodes in the document — the soak's bounded-memory readout
    /// (after churn + GC it must not grow without bound).
    pub fn live_node_count(&self) -> usize {
        self.rt.host().borrow().dom.live_node_count()
    }

    /// The `console.log` / `console.error` output the page's script produced, in call
    /// order (for tests and a future devtools surface).
    pub fn console(&self) -> Vec<String> {
        self.rt.host().borrow().console.clone()
    }
}

/// Which JS engine the scripted profile runs on. Boa is pure Rust (all targets, the
/// default conformance oracle); Nova is native-only and gated behind the
/// `scripted-nova` feature, so the default build links a single engine (Boa + Nova +
/// wgpu together exceed the Windows image-size link limit). Selected at the call site,
/// exactly as serval-wpt's `--engine` picks the monomorphization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScriptedEngine {
    #[default]
    Boa,
    Nova,
}

impl ScriptedEngine {
    /// Parse a `--js` value (`boa` / `nova`), case-insensitively.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "boa" => Some(Self::Boa),
            "nova" => Some(Self::Nova),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Boa => "boa",
            Self::Nova => "nova",
        }
    }
}

/// When a `<script>` runs relative to document parsing (the classic-script model).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptTiming {
    /// Parser-blocking: runs at its document position (inline, or external with
    /// neither `async` nor `defer`).
    Blocking,
    /// `defer`: runs after the parser-blocking pass, in document order.
    Defer,
    /// `async`: runs after the parser-blocking pass, order unspecified (we fetch
    /// synchronously, so document order is a faithful realization).
    Async,
}

/// One runnable `<script>` in document order: inline classic text, or an external
/// classic `src` (raw attribute value, resolved against the document URL at fetch
/// time) with its timing and post-fetch processing (`charset` decode + `integrity`
/// SRI check). An external `src` takes precedence over inline content (per HTML).
/// `type=module` and non-JS `type`s are not runnable and are dropped at collection
/// (see [`classify_script_type`]).
enum ScriptSource {
    Inline(String),
    External {
        src: String,
        timing: ScriptTiming,
        /// `<script charset>` — the encoding to decode the fetched bytes with
        /// (default UTF-8).
        charset: Option<String>,
        /// `<script integrity>` — Subresource-Integrity metadata the fetched bytes
        /// must match, else the script is blocked.
        integrity: Option<String>,
    },
    /// `<script type=module>…</script>` — inline module source. Modules are always
    /// deferred (run after the parser-blocking pass) and evaluated with module scope
    /// via the engine's module path.
    ModuleInline(String),
    /// `<script type=module src=…>` — external module: fetched (with `charset` /
    /// `integrity`) like a classic external, then evaluated as a module.
    ModuleExternal {
        src: String,
        charset: Option<String>,
        integrity: Option<String>,
    },
}

/// How a `<script>`'s `type` attribute classifies it.
enum ScriptKind {
    /// Empty/absent `type`, or a JavaScript MIME type — a runnable classic script.
    Classic,
    /// `type=module` — recognized but not yet executed (module loading is a
    /// follow-up); deferred timing when it lands.
    Module,
    /// Any other `type` (`application/json`, `text/plain`, an import map, …) — a
    /// data block, never executed.
    Data,
}

/// Classify a `<script type>` value. Per HTML: empty/absent or a JavaScript MIME
/// type essence → classic; `module` → module; anything else → a data block. The JS
/// MIME essences mirror the WHATWG list (cf. `net::mime_classifier::is_javascript`).
fn classify_script_type(ty: Option<&str>) -> ScriptKind {
    let ty = match ty.map(str::trim) {
        None | Some("") => return ScriptKind::Classic,
        Some(t) => t.to_ascii_lowercase(),
    };
    if ty == "module" {
        return ScriptKind::Module;
    }
    // Match on the MIME essence (drop any `;`-params), against the WHATWG JS set.
    const JS_MIME: &[&str] = &[
        "application/ecmascript",
        "application/javascript",
        "application/x-ecmascript",
        "application/x-javascript",
        "text/ecmascript",
        "text/javascript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/jscript",
        "text/livescript",
        "text/x-ecmascript",
        "text/x-javascript",
    ];
    let essence = ty.split(';').next().unwrap_or("").trim();
    if JS_MIME.contains(&essence) {
        ScriptKind::Classic
    } else {
        ScriptKind::Data
    }
}

/// Every runnable classic `<script>` in document order, with its timing. `src`
/// scripts become [`ScriptSource::External`]; inline-text scripts
/// [`ScriptSource::Inline`]. Empty inline scripts, non-JS `type` data blocks, and
/// `type=module` (logged, execution unsupported) are dropped. One ordered list is
/// what lets external and inline scripts interleave in authored order.
fn collect_scripts(doc: &StaticDocument) -> Vec<ScriptSource> {
    let mut out = Vec::new();
    collect_scripts_rec(doc, doc.document(), &mut out);
    out
}

fn collect_scripts_rec(dom: &StaticDocument, node: StaticNodeId, out: &mut Vec<ScriptSource>) {
    if dom.element_name(node).is_some_and(|q| q.local.as_ref() == "script") {
        let attr = |name: &str| dom.attribute(node, &Namespace::default(), &LocalName::from(name));
        match classify_script_type(attr("type")) {
            ScriptKind::Data => {} // a data block: not executed
            ScriptKind::Module => {
                let nonempty =
                    |name: &str| attr(name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                match nonempty("src") {
                    Some(src) => out.push(ScriptSource::ModuleExternal {
                        src,
                        charset: nonempty("charset"),
                        integrity: nonempty("integrity"),
                    }),
                    None => {
                        let mut text = String::new();
                        for child in dom.dom_children(node) {
                            if let Some(t) = dom.text(child) {
                                text.push_str(t);
                            }
                        }
                        if !text.trim().is_empty() {
                            out.push(ScriptSource::ModuleInline(text));
                        }
                    },
                }
            },
            ScriptKind::Classic => {
                let src = attr("src").map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                match src {
                    // A `src` script ignores its element content (HTML spec). `async`
                    // takes precedence over `defer` when both are present.
                    Some(src) => {
                        let timing = if attr("async").is_some() {
                            ScriptTiming::Async
                        } else if attr("defer").is_some() {
                            ScriptTiming::Defer
                        } else {
                            ScriptTiming::Blocking
                        };
                        let nonempty = |name: &str| {
                            attr(name).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
                        };
                        out.push(ScriptSource::External {
                            src,
                            timing,
                            charset: nonempty("charset"),
                            integrity: nonempty("integrity"),
                        });
                    },
                    // Inline classic: `async`/`defer` are ignored — it runs in place.
                    None => {
                        let mut text = String::new();
                        for child in dom.dom_children(node) {
                            if let Some(t) = dom.text(child) {
                                text.push_str(t);
                            }
                        }
                        if !text.trim().is_empty() {
                            out.push(ScriptSource::Inline(text));
                        }
                    },
                }
            },
        }
    }
    for child in dom.dom_children(node) {
        collect_scripts_rec(dom, child, out);
    }
}

/// Fetch an external script's source through the `loader` (`(fetcher, base_url)`),
/// resolving `src` against `base_url`, verifying any `integrity` (SRI) metadata, and
/// decoding the bytes per `charset` (default UTF-8). `None` (with a log) when there
/// is no loader (the fetch-free [`ScriptedDocument::parse`] path), the fetch fails,
/// or the integrity check rejects the bytes.
fn fetch_external(
    loader: Option<(&dyn ResourceFetcher, &str)>,
    src: &str,
    charset: Option<&str>,
    integrity: Option<&str>,
) -> Option<String> {
    let Some((fetcher, base)) = loader else {
        eprintln!("[pelt-scripted] skipping external <script src=\"{src}\"> (no fetcher)");
        return None;
    };
    let url = crate::href::resolve_href(base, src);
    let bytes = match fetcher.fetch(&url) {
        Some(bytes) => bytes,
        None => {
            eprintln!("[pelt-scripted] could not fetch script {url}");
            return None;
        },
    };
    if let Some(metadata) = integrity {
        if !integrity_matches(metadata, &bytes) {
            eprintln!("[pelt-scripted] integrity mismatch for {url}; script blocked");
            return None;
        }
    }
    Some(decode_script_bytes(&bytes, charset))
}

/// Whether `bytes` satisfy a Subresource-Integrity `integrity` attribute. Per SRI:
/// parse the space-separated `alg-base64hash[?opts]` tokens, take the **strongest**
/// algorithm present (sha512 > sha384 > sha256), and accept if the digest matches
/// **any** of that algorithm's hashes. Unrecognized/empty metadata imposes no
/// requirement (returns `true`). Compares raw digest bytes, so base64 padding
/// variance does not matter.
fn integrity_matches(metadata: &str, bytes: &[u8]) -> bool {
    use base64::Engine as _;
    use sha2::Digest as _;

    let mut strongest = 0u8; // 1 = sha256, 2 = sha384, 3 = sha512
    let mut expected: Vec<&str> = Vec::new();
    for token in metadata.split_whitespace() {
        let Some((alg, rest)) = token.split_once('-') else { continue };
        let strength = match alg {
            "sha256" => 1u8,
            "sha384" => 2,
            "sha512" => 3,
            _ => continue,
        };
        let hash = rest.split('?').next().unwrap_or(rest); // drop any `?options`
        if strength > strongest {
            strongest = strength;
            expected.clear();
            expected.push(hash);
        } else if strength == strongest {
            expected.push(hash);
        }
    }
    if strongest == 0 {
        return true; // no valid metadata → no integrity requirement
    }
    let digest: Vec<u8> = match strongest {
        1 => sha2::Sha256::digest(bytes).to_vec(),
        2 => sha2::Sha384::digest(bytes).to_vec(),
        _ => sha2::Sha512::digest(bytes).to_vec(),
    };
    let std = base64::engine::general_purpose::STANDARD;
    let nopad = base64::engine::general_purpose::STANDARD_NO_PAD;
    expected.iter().any(|h| {
        std.decode(h).or_else(|_| nopad.decode(h)).map(|d| d == digest).unwrap_or(false)
    })
}

/// Decode fetched script bytes into source text using the `<script charset>`
/// encoding (resolved through `encoding_rs`), defaulting to UTF-8. An unknown label
/// also falls back to UTF-8.
fn decode_script_bytes(bytes: &[u8], charset: Option<&str>) -> String {
    let encoding = charset
        .and_then(|label| encoding_rs::Encoding::for_label(label.trim().as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// Evaluate `source`, reporting (but not propagating) a script error — a browser
/// keeps rendering the document after a script throws.
fn eval_reporting<E: ScriptEngine>(rt: &mut Runtime<E>, source: &str) {
    if let Err(e) = rt.eval(source) {
        eprintln!("[pelt-scripted] script error: {e:?}");
    }
}

/// Evaluate `source` as a module (`<script type=module>`) with `base_url` as the
/// base its `import`s resolve against, fetching each dependency through `loader`'s
/// fetcher. Reports (but does not propagate) failures: an engine without module
/// support (`Ok(None)`) is logged and skipped; a module that throws — or a
/// dependency that fails to fetch — is reported, like a classic script error.
fn eval_module_reporting<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    loader: Option<(&dyn ResourceFetcher, &str)>,
    base_url: &str,
    source: &str,
) {
    // Resolve an import specifier against the importing module's URL (`referrer`, or
    // `base_url` for the entry), then fetch its source through the page fetcher.
    // WHATWG URL join (not the naive `resolve_href`) so `./` and `../` normalize.
    let mut resolve = |specifier: &str, referrer: &str| -> Option<(String, String)> {
        let (fetcher, _page) = loader?;
        let base = if referrer.is_empty() { base_url } else { referrer };
        let url = url::Url::parse(base).ok()?.join(specifier).ok()?.to_string();
        let bytes = fetcher.fetch(&url)?;
        Some((url, String::from_utf8_lossy(&bytes).into_owned()))
    };
    match rt.eval_module(source, base_url, &mut resolve) {
        Ok(Some(_)) => {},
        Ok(None) => eprintln!(
            "[pelt-scripted] <script type=module> not supported by this engine; skipped"
        ),
        Err(e) => eprintln!("[pelt-scripted] module error: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use script_engine_boa::BoaEngine;

    /// A page whose inline script injects a `<p>` with text: the rendered scene gains
    /// glyph runs that an empty body would not have — the load → run-script → mutate →
    /// render path end to end.
    fn mutation_renders<E: ScriptEngine>() {
        let html = "<body><script>\
            var p = document.createElement('p');\
            p.appendChild(document.createTextNode('injected'));\
            document.body.appendChild(p);\
            </script></body>";
        let mut doc = ScriptedDocument::<E>::parse(html).expect("runtime inits");
        let scene = doc.frame(400, 300);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "script-injected text renders as glyphs",
        );
    }

    /// Control: with no script, the same empty body paints no text — so the glyphs in
    /// [`mutation_renders`] came from the script, not the markup.
    fn empty_body_has_no_text<E: ScriptEngine>() {
        let mut doc = ScriptedDocument::<E>::parse("<body></body>").expect("runtime inits");
        let scene = doc.frame(400, 300);
        assert!(
            !scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "an empty body paints no text",
        );
    }

    /// Script that builds tall content makes the document scrollable: the offset
    /// advances on a wheel delta and clamps at the bottom.
    fn scripted_content_scrolls<E: ScriptEngine>() {
        let html = "<body><script>\
            var d = document.createElement('div');\
            d.setAttribute('style', 'height: 2000px');\
            document.body.appendChild(d);\
            </script></body>";
        let mut doc = ScriptedDocument::<E>::parse(html).expect("runtime inits");
        let _ = doc.frame(400, 300);
        assert_eq!(doc.scroll(), (0.0, 0.0), "starts at the top");
        assert!(doc.scroll_by(0.0, 250.0), "tall scripted content scrolls");
        assert!((doc.scroll().1 - 250.0).abs() < 0.5, "offset advanced: {:?}", doc.scroll());
        let _ = doc.scroll_by(0.0, 100_000.0);
        assert!(!doc.scroll_by(0.0, 100.0), "clamped at the bottom edge");
    }

    /// The GC tick reaps a node the script orphaned and dropped its only reference to:
    /// after building then detaching + dereferencing a subtree, [`pump`] collects it.
    fn pump_collects_orphans<E: ScriptEngine>() {
        let html = "<body><script>\
            var keep = document.createElement('div');\
            document.body.appendChild(keep);\
            var gone = document.createElement('span');\
            keep.appendChild(gone);\
            keep.removeChild(gone);\
            gone = null;\
            </script></body>";
        let mut doc = ScriptedDocument::<E>::parse(html).expect("runtime inits");
        let _ = doc.frame(400, 300);
        let before = doc.live_node_count();
        // Drive a frame's worth: forcing the engine GC drops the dropped <span>
        // wrapper, the weak reflector cache reports it dead, the pin retires, and the
        // orphan is reaped — the live set actually shrinks (the WeakMap-cache contract;
        // a strong cache would leave it flat).
        let (unpinned, collected) = doc.pump(16.0);
        let after = doc.live_node_count();
        assert!(after < before, "the orphaned node is reaped: {before} -> {after}");
        assert!(collected >= 1, "collect_garbage reaped at least the orphan (got {collected})");
        let _ = unpinned;
    }

    /// The gc-arena soak (carve-out #2): a page that churns nodes under `setInterval`
    /// is driven through [`pump`](ScriptedDocument::pump) at frame cadence; the GC tick
    /// keeps the live set bounded rather than growing one batch per frame. Without a
    /// working frame-cadence collector this peaks in the thousands; with it, a handful.
    fn gc_soak_bounds_memory<E: ScriptEngine>() {
        // Each tick: append a batch of fresh nodes to a host, then remove them all.
        // The removed nodes are orphaned + unreachable from script (the locals fall out
        // of scope), so the collector should reap them.
        let html = "<body><script>\
            var host = document.createElement('div');\
            document.body.appendChild(host);\
            function churn() {\
                for (var i = 0; i < 50; i++) {\
                    var n = document.createElement('span');\
                    n.appendChild(document.createTextNode('x'));\
                    host.appendChild(n);\
                }\
                while (host.firstChild) { host.removeChild(host.firstChild); }\
            }\
            setInterval(churn, 16);\
            </script></body>";
        let mut doc = ScriptedDocument::<E>::parse(html).expect("runtime inits");
        let _ = doc.frame(400, 300);
        let mut now = 0.0;
        let mut peak = 0;
        for _ in 0..120 {
            now += 16.0;
            doc.pump(now);
            let _ = doc.frame(400, 300);
            peak = peak.max(doc.live_node_count());
        }
        // Bounded: a few structural nodes + at most a batch or two in flight — not the
        // ~6000 (50 × 120) an uncollected churn would accumulate.
        assert!(peak < 1000, "frame-cadence GC bounds the churned DOM; peak live = {peak}");
    }

    /// Node identity survives the WeakMap wrapper cache: the same node yields the same
    /// JS wrapper (`getElementById('x') === getElementById('x')`) and a created node's
    /// `parentNode` round-trips. Guards the strong-Map → WeakMap change (a broken cache
    /// would mint a fresh wrapper per call and `===` would be false).
    fn node_identity_is_stable<E: ScriptEngine>() {
        let html = "<body><div id=\"x\"></div><script>\
            var same = document.getElementById('x') === document.getElementById('x');\
            var p = document.createElement('p');\
            document.body.appendChild(p);\
            var parented = p.parentNode === document.body;\
            console.log('same:' + same + ' parented:' + parented);\
            </script></body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("runtime inits");
        assert!(
            doc.console().iter().any(|l| l == "same:true parented:true"),
            "node identity preserved through the WeakMap cache: {:?}",
            doc.console(),
        );
    }

    /// In-memory [`ResourceFetcher`] for the external-script tests: a fixed
    /// URL→bytes map, so a `load` resolves the page and its `<script src>`s without
    /// touching the network or disk.
    struct MapFetcher(std::collections::HashMap<String, Vec<u8>>);
    impl ResourceFetcher for MapFetcher {
        fn fetch(&self, url: &str) -> Option<Vec<u8>> {
            self.0.get(url).cloned()
        }
    }
    fn map_fetcher(files: &[(&str, &str)]) -> MapFetcher {
        MapFetcher(files.iter().map(|(u, b)| (u.to_string(), b.as_bytes().to_vec())).collect())
    }

    /// An external `<script src>` is fetched and executed: the script injects a `<p>`,
    /// so the rendered scene gains glyph runs an empty body would not have — the
    /// load → fetch-script → run → mutate → render path end to end. (This is the gap
    /// item 3 closes: an inline-only driver rendered nothing for this page.)
    fn external_script_runs<E: ScriptEngine>() {
        let files = map_fetcher(&[
            ("http://x/index.html", "<body><script src=\"app.js\"></script></body>"),
            (
                "http://x/app.js",
                "var p=document.createElement('p');\
                 p.appendChild(document.createTextNode('ext'));\
                 document.body.appendChild(p);",
            ),
        ]);
        let mut doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        let scene = doc.frame(400, 300);
        assert!(
            scene.ops.iter().any(|op| matches!(op, netrender::SceneOp::GlyphRun(_))),
            "external-script-injected text renders as glyphs",
        );
    }

    /// Inline and external scripts run in document order: three scripts (inline,
    /// external, inline) each log a letter, and the console shows `A`, `B`, `C` in
    /// order — proving inline and external interleave in authored order (the ordering
    /// the old inline-only path explicitly could not guarantee).
    fn scripts_run_in_document_order<E: ScriptEngine>() {
        let files = map_fetcher(&[
            (
                "http://x/index.html",
                "<body>\
                    <script>console.log('A');</script>\
                    <script src=\"b.js\"></script>\
                    <script>console.log('C');</script>\
                 </body>",
            ),
            ("http://x/b.js", "console.log('B');"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert_eq!(
            doc.console(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
            "scripts ran in document order (inline A, external B, inline C)",
        );
    }

    /// A relative `src` resolves against the document URL's directory, not the host
    /// root: `sub/app.js` on `http://x/dir/index.html` fetches
    /// `http://x/dir/sub/app.js`.
    fn relative_src_resolves_against_page_url<E: ScriptEngine>() {
        let files = map_fetcher(&[
            ("http://x/dir/index.html", "<body><script src=\"sub/app.js\"></script></body>"),
            ("http://x/dir/sub/app.js", "console.log('relative-ok');"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/dir/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "relative-ok"),
            "relative src resolved against the page directory: {:?}",
            doc.console(),
        );
    }

    /// A missing external script is reported and skipped, not fatal: the page still
    /// loads and its inline siblings still run (browser resilience).
    fn missing_external_script_is_skipped<E: ScriptEngine>() {
        let files = map_fetcher(&[(
            "http://x/index.html",
            "<body><script src=\"gone.js\"></script><script>console.log('still-here');</script></body>",
        )]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads anyway");
        assert!(
            doc.console().iter().any(|l| l == "still-here"),
            "inline sibling runs despite the missing external script: {:?}",
            doc.console(),
        );
    }

    /// `defer` runs the external script *after* the parser-blocking pass: a deferred
    /// script that appears *before* a later inline script nonetheless runs *after* it.
    /// Document-order execution would log `defer` first; deferral logs `inline` first.
    fn defer_runs_after_parser_blocking<E: ScriptEngine>() {
        let files = map_fetcher(&[
            (
                "http://x/index.html",
                "<body>\
                    <script src=\"defer.js\" defer></script>\
                    <script>console.log('inline');</script>\
                 </body>",
            ),
            ("http://x/defer.js", "console.log('defer');"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert_eq!(
            doc.console(),
            vec!["inline".to_string(), "defer".to_string()],
            "the inline (parser-blocking) script runs before the earlier-positioned defer",
        );
    }

    /// `defer` scripts run in document order among themselves (the deferral guarantee).
    fn defer_scripts_run_in_document_order<E: ScriptEngine>() {
        let files = map_fetcher(&[
            (
                "http://x/index.html",
                "<body>\
                    <script src=\"d1.js\" defer></script>\
                    <script src=\"d2.js\" defer></script>\
                 </body>",
            ),
            ("http://x/d1.js", "console.log('d1');"),
            ("http://x/d2.js", "console.log('d2');"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert_eq!(
            doc.console(),
            vec!["d1".to_string(), "d2".to_string()],
            "defer scripts keep document order",
        );
    }

    /// `async` does not block the parser: an async script positioned before a later
    /// inline script runs after it (the async script is deferred past the blocking pass).
    fn async_runs_after_parser_blocking<E: ScriptEngine>() {
        let files = map_fetcher(&[
            (
                "http://x/index.html",
                "<body>\
                    <script src=\"a.js\" async></script>\
                    <script>console.log('inline');</script>\
                 </body>",
            ),
            ("http://x/a.js", "console.log('async');"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert_eq!(
            doc.console(),
            vec!["inline".to_string(), "async".to_string()],
            "the async script does not block the later inline script",
        );
    }

    /// A non-JavaScript `type` (here `application/json`) is a data block: its content
    /// is never executed, even though it is syntactically runnable JS. A classic
    /// sibling still runs.
    fn script_type_data_block_is_not_executed<E: ScriptEngine>() {
        let html = "<body>\
            <script type=\"application/json\">console.log('json-ran');</script>\
            <script>console.log('classic-ran');</script>\
         </body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("loads");
        assert_eq!(
            doc.console(),
            vec!["classic-ran".to_string()],
            "the application/json data block did not execute",
        );
    }

    /// A `type=module` script never breaks the page: its classic siblings run
    /// regardless of whether this backend supports modules. (Engine-agnostic: a
    /// module-capable backend also runs the module — after the parser-blocking pass —
    /// but that is asserted in the Boa-only module tests below.)
    fn module_keeps_classic_siblings_running<E: ScriptEngine>() {
        let html = "<body>\
            <script type=\"module\">globalThis.__m = 1;</script>\
            <script>console.log('classic-ran');</script>\
         </body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "classic-ran"),
            "the classic sibling runs regardless of module support: {:?}",
            doc.console(),
        );
    }

    /// A `type=module` script executes with **module scope**: its top-level
    /// `var` is module-local and does not leak to `globalThis` (a classic script's
    /// `var` would). Proves modules run with real module semantics, not script eval.
    fn module_executes_with_module_scope<E: ScriptEngine>() {
        let html = "<body><script type=\"module\">\
            var moduleLocal = 7;\
            console.log('module:' + moduleLocal + ',' + (typeof globalThis.moduleLocal));\
            </script></body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "module:7,undefined"),
            "module ran with module scope (local visible, not leaked): {:?}",
            doc.console(),
        );
    }

    /// Modules are deferred: an inline classic script runs before a module that
    /// precedes it in document order.
    fn module_runs_after_parser_blocking<E: ScriptEngine>() {
        let html = "<body>\
            <script type=\"module\">console.log('module');</script>\
            <script>console.log('classic');</script>\
         </body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("loads");
        assert_eq!(
            doc.console(),
            vec!["classic".to_string(), "module".to_string()],
            "the classic script runs before the earlier-positioned module (modules defer)",
        );
    }

    /// A module that `import`s another but cannot fetch it (the fetch-free
    /// `parse` path has no loader) fails gracefully: the import rejects, the module is
    /// reported and skipped, and a classic sibling still runs (the page is not broken).
    fn module_import_fails_gracefully<E: ScriptEngine>() {
        let html = "<body>\
            <script type=\"module\">import x from './dep.js'; console.log('after-import');</script>\
            <script>console.log('sibling');</script>\
         </body>";
        let doc = ScriptedDocument::<E>::parse(html).expect("loads");
        assert!(
            !doc.console().iter().any(|l| l == "after-import"),
            "the import rejected, so the module body past the import did not run: {:?}",
            doc.console(),
        );
        assert!(
            doc.console().iter().any(|l| l == "sibling"),
            "the failed module is not fatal — the classic sibling still runs: {:?}",
            doc.console(),
        );
    }

    /// An external `<script type=module src=…>` is fetched (like a classic
    /// external) and evaluated as a module.
    fn external_module_runs<E: ScriptEngine>() {
        let files = map_fetcher(&[
            ("http://x/index.html", "<body><script type=\"module\" src=\"m.js\"></script></body>"),
            ("http://x/m.js", "console.log('ext-module:' + (typeof globalThis.x));\nvar x = 1;"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "ext-module:undefined"),
            "external module fetched and run with module scope: {:?}",
            doc.console(),
        );
    }

    /// Cross-module `import` works: an entry module imports a named export from
    /// a relative dependency (resolved against the entry's URL and fetched through the
    /// host loader) and uses it.
    fn module_imports_dependency<E: ScriptEngine>() {
        let files = map_fetcher(&[
            ("http://x/index.html", "<body><script type=\"module\" src=\"main.js\"></script></body>"),
            ("http://x/main.js", "import { greet } from './dep.js';\nconsole.log(greet('world'));"),
            ("http://x/dep.js", "export function greet(name) { return 'hello ' + name; }"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "hello world"),
            "the entry module imported and used the dependency's export: {:?}",
            doc.console(),
        );
    }

    /// A diamond import (`main` → `b`, `c` → `shared`) loads `shared` exactly
    /// once: its top-level side effect fires a single time (the loader caches by URL).
    fn module_import_diamond_loads_shared_once<E: ScriptEngine>() {
        let files = map_fetcher(&[
            ("http://x/index.html", "<body><script type=\"module\" src=\"main.js\"></script></body>"),
            (
                "http://x/main.js",
                "import { b } from './b.js';\nimport { c } from './c.js';\nconsole.log('main:' + b + c);",
            ),
            ("http://x/b.js", "import { x } from './shared.js';\nexport var b = x;"),
            ("http://x/c.js", "import { x } from './shared.js';\nexport var c = x;"),
            ("http://x/shared.js", "console.log('shared-init');\nexport var x = 'S';"),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        let console = doc.console();
        assert_eq!(
            console.iter().filter(|l| *l == "shared-init").count(),
            1,
            "the shared module initializes exactly once across the diamond: {console:?}",
        );
        assert!(
            console.iter().any(|l| l == "main:SS"),
            "both branches see the shared export: {console:?}",
        );
    }

    /// Build a fetcher from owned `(url, bytes)` pairs — for fixtures whose script
    /// bytes are not valid UTF-8 (charset) or are hashed (integrity).
    fn map_of(files: Vec<(&str, Vec<u8>)>) -> MapFetcher {
        MapFetcher(files.into_iter().map(|(u, b)| (u.to_string(), b)).collect())
    }

    /// `<script charset>` decodes the fetched bytes with the named encoding, not
    /// UTF-8: an ISO-8859-1 script with a `0xE9` byte ('é') decodes to `café`. As
    /// UTF-8 the lone `0xE9` is invalid and would become a replacement char.
    fn external_script_charset_decodes<E: ScriptEngine>() {
        let mut script = b"console.log('caf".to_vec();
        script.push(0xE9); // 'é' in ISO-8859-1; invalid as UTF-8
        script.extend_from_slice(b"');");
        let files = map_of(vec![
            (
                "http://x/index.html",
                b"<body><script src=\"app.js\" charset=\"iso-8859-1\"></script></body>".to_vec(),
            ),
            ("http://x/app.js", script),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "caf\u{e9}"),
            "iso-8859-1 script decoded to 'café': {:?}",
            doc.console(),
        );
    }

    /// A matching `integrity` (SRI) hash lets the external script run.
    fn integrity_match_runs<E: ScriptEngine>() {
        use base64::Engine as _;
        use sha2::Digest as _;
        let script = b"console.log('sri-ok');";
        let hash = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(script));
        let files = map_of(vec![
            (
                "http://x/index.html",
                format!("<body><script src=\"app.js\" integrity=\"sha256-{hash}\"></script></body>")
                    .into_bytes(),
            ),
            ("http://x/app.js", script.to_vec()),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "sri-ok"),
            "matching integrity runs the script: {:?}",
            doc.console(),
        );
    }

    /// A mismatched `integrity` hash blocks the external script (it never runs), but a
    /// classic sibling still runs — the block is per-script, not fatal.
    fn integrity_mismatch_blocks<E: ScriptEngine>() {
        use base64::Engine as _;
        use sha2::Digest as _;
        // A hash of *different* content: the fetched script will not match it.
        let wrong =
            base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(b"other bytes"));
        let files = map_of(vec![
            (
                "http://x/index.html",
                format!(
                    "<body>\
                        <script src=\"app.js\" integrity=\"sha256-{wrong}\"></script>\
                        <script>console.log('after');</script>\
                     </body>"
                )
                .into_bytes(),
            ),
            ("http://x/app.js", b"console.log('should-not-run');".to_vec()),
        ]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/index.html").expect("loads");
        assert!(
            !doc.console().iter().any(|l| l == "should-not-run"),
            "mismatched integrity blocks the script: {:?}",
            doc.console(),
        );
        assert!(
            doc.console().iter().any(|l| l == "after"),
            "the blocked script is not fatal — the sibling still runs: {:?}",
            doc.console(),
        );
    }

    /// `ScriptedDocument::load` sets the runtime base URL from the page URL, so a
    /// reflected URL attribute (`a.href`) resolves to an absolute URL against it.
    fn url_attributes_resolve_against_page_url<E: ScriptEngine>() {
        let files = map_fetcher(&[(
            "http://x/dir/index.html",
            "<body><a id='a' href='sub/p.html'></a>\
             <script>console.log(document.getElementById('a').href);</script></body>",
        )]);
        let doc = ScriptedDocument::<E>::load(&files, "http://x/dir/index.html").expect("loads");
        assert!(
            doc.console().iter().any(|l| l == "http://x/dir/sub/p.html"),
            "a.href resolved against the page URL: {:?}",
            doc.console(),
        );
    }

    /// End-to-end: `getComputedStyle` reads the rendered frame's cascade through
    /// the `ComputedStyleBridge`. The page schedules the read in a timer; `frame()`
    /// lays out (populating the bridge), then `pump()` fires the timer so the read
    /// sees real computed values.
    fn get_computed_style_reads_cascade<E: ScriptEngine>() {
        let mut doc = ScriptedDocument::<E>::parse(
            "<html><body><div id='d' style='color: red; display: inline'></div>\
             <script>setTimeout(function(){\
               var cs = getComputedStyle(document.getElementById('d'));\
               console.log(cs.color + '|' + cs.display);\
             }, 0);</script></body></html>",
        )
        .expect("doc");
        let _ = doc.frame(400, 300); // lay out -> populate the bridge
        doc.pump(16.0); // fire the timer -> getComputedStyle reads the cascade
        assert!(
            doc.console().iter().any(|l| l == "rgb(255, 0, 0)|inline"),
            "getComputedStyle read the cascade: {:?}",
            doc.console(),
        );
    }

    #[test]
    fn mutation_renders_on_boa() {
        mutation_renders::<BoaEngine>();
    }
    #[test]
    fn get_computed_style_reads_cascade_on_boa() {
        get_computed_style_reads_cascade::<BoaEngine>();
    }
    #[test]
    fn external_script_runs_on_boa() {
        external_script_runs::<BoaEngine>();
    }
    #[test]
    fn scripts_run_in_document_order_on_boa() {
        scripts_run_in_document_order::<BoaEngine>();
    }
    #[test]
    fn relative_src_resolves_against_page_url_on_boa() {
        relative_src_resolves_against_page_url::<BoaEngine>();
    }
    #[test]
    fn missing_external_script_is_skipped_on_boa() {
        missing_external_script_is_skipped::<BoaEngine>();
    }
    #[test]
    fn defer_runs_after_parser_blocking_on_boa() {
        defer_runs_after_parser_blocking::<BoaEngine>();
    }
    #[test]
    fn defer_scripts_run_in_document_order_on_boa() {
        defer_scripts_run_in_document_order::<BoaEngine>();
    }
    #[test]
    fn async_runs_after_parser_blocking_on_boa() {
        async_runs_after_parser_blocking::<BoaEngine>();
    }
    #[test]
    fn script_type_data_block_is_not_executed_on_boa() {
        script_type_data_block_is_not_executed::<BoaEngine>();
    }
    #[test]
    fn module_keeps_classic_siblings_running_on_boa() {
        module_keeps_classic_siblings_running::<BoaEngine>();
    }
    #[test]
    fn module_executes_with_module_scope_on_boa() {
        module_executes_with_module_scope::<BoaEngine>();
    }
    #[test]
    fn module_runs_after_parser_blocking_on_boa() {
        module_runs_after_parser_blocking::<BoaEngine>();
    }
    #[test]
    fn module_import_fails_gracefully_on_boa() {
        module_import_fails_gracefully::<BoaEngine>();
    }
    #[test]
    fn external_module_runs_on_boa() {
        external_module_runs::<BoaEngine>();
    }
    #[test]
    fn module_imports_dependency_on_boa() {
        module_imports_dependency::<BoaEngine>();
    }
    #[test]
    fn module_import_diamond_loads_shared_once_on_boa() {
        module_import_diamond_loads_shared_once::<BoaEngine>();
    }
    #[test]
    fn external_script_charset_decodes_on_boa() {
        external_script_charset_decodes::<BoaEngine>();
    }
    #[test]
    fn integrity_match_runs_on_boa() {
        integrity_match_runs::<BoaEngine>();
    }
    #[test]
    fn integrity_mismatch_blocks_on_boa() {
        integrity_mismatch_blocks::<BoaEngine>();
    }
    #[test]
    fn url_attributes_resolve_against_page_url_on_boa() {
        url_attributes_resolve_against_page_url::<BoaEngine>();
    }
    #[test]
    fn node_identity_is_stable_on_boa() {
        node_identity_is_stable::<BoaEngine>();
    }
    #[test]
    fn gc_soak_bounds_memory_on_boa() {
        gc_soak_bounds_memory::<BoaEngine>();
    }
    #[test]
    fn empty_body_has_no_text_on_boa() {
        empty_body_has_no_text::<BoaEngine>();
    }
    #[test]
    fn scripted_content_scrolls_on_boa() {
        scripted_content_scrolls::<BoaEngine>();
    }
    #[test]
    fn pump_collects_orphans_on_boa() {
        pump_collects_orphans::<BoaEngine>();
    }

    #[cfg(feature = "scripted-nova")]
    mod nova {
        use super::*;
        use script_engine_nova::NovaEngine;

        #[test]
        fn mutation_renders_on_nova() {
            mutation_renders::<NovaEngine>();
        }
        #[test]
        fn get_computed_style_reads_cascade_on_nova() {
            get_computed_style_reads_cascade::<NovaEngine>();
        }
        #[test]
        fn scripted_content_scrolls_on_nova() {
            scripted_content_scrolls::<NovaEngine>();
        }
        // These passed on Boa and failed on Nova until the Nova `Global`-leak fix
        // (the `NovaValue` deferred-release wrapper; reflectors passed as native-fn
        // arguments are now freed at call end instead of pinning every node). See
        // `script-engine-nova`'s `arg_reflector_dies_after_gc` and
        // `docs/2026-06-19_nova_reflector_global_leak.md`.
        #[test]
        fn pump_collects_orphans_on_nova() {
            pump_collects_orphans::<NovaEngine>();
        }
        #[test]
        fn gc_soak_bounds_memory_on_nova() {
            gc_soak_bounds_memory::<NovaEngine>();
        }
        #[test]
        fn node_identity_is_stable_on_nova() {
            node_identity_is_stable::<NovaEngine>();
        }
        #[test]
        fn external_script_runs_on_nova() {
            external_script_runs::<NovaEngine>();
        }
        #[test]
        fn scripts_run_in_document_order_on_nova() {
            scripts_run_in_document_order::<NovaEngine>();
        }
        #[test]
        fn relative_src_resolves_against_page_url_on_nova() {
            relative_src_resolves_against_page_url::<NovaEngine>();
        }
        #[test]
        fn missing_external_script_is_skipped_on_nova() {
            missing_external_script_is_skipped::<NovaEngine>();
        }
        #[test]
        fn defer_runs_after_parser_blocking_on_nova() {
            defer_runs_after_parser_blocking::<NovaEngine>();
        }
        #[test]
        fn defer_scripts_run_in_document_order_on_nova() {
            defer_scripts_run_in_document_order::<NovaEngine>();
        }
        #[test]
        fn async_runs_after_parser_blocking_on_nova() {
            async_runs_after_parser_blocking::<NovaEngine>();
        }
        #[test]
        fn script_type_data_block_is_not_executed_on_nova() {
            script_type_data_block_is_not_executed::<NovaEngine>();
        }
        #[test]
        fn module_keeps_classic_siblings_running_on_nova() {
            module_keeps_classic_siblings_running::<NovaEngine>();
        }
        #[test]
        fn module_executes_with_module_scope_on_nova() {
            module_executes_with_module_scope::<NovaEngine>();
        }
        #[test]
        fn module_runs_after_parser_blocking_on_nova() {
            module_runs_after_parser_blocking::<NovaEngine>();
        }
        #[test]
        fn module_import_fails_gracefully_on_nova() {
            module_import_fails_gracefully::<NovaEngine>();
        }
        #[test]
        fn external_module_runs_on_nova() {
            external_module_runs::<NovaEngine>();
        }
        #[test]
        fn module_imports_dependency_on_nova() {
            module_imports_dependency::<NovaEngine>();
        }
        #[test]
        fn module_import_diamond_loads_shared_once_on_nova() {
            module_import_diamond_loads_shared_once::<NovaEngine>();
        }
        #[test]
        fn external_script_charset_decodes_on_nova() {
            external_script_charset_decodes::<NovaEngine>();
        }
        #[test]
        fn integrity_match_runs_on_nova() {
            integrity_match_runs::<NovaEngine>();
        }
        #[test]
        fn integrity_mismatch_blocks_on_nova() {
            integrity_mismatch_blocks::<NovaEngine>();
        }
        #[test]
        fn url_attributes_resolve_against_page_url_on_nova() {
            url_attributes_resolve_against_page_url::<NovaEngine>();
        }
    }
}
