/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The scripted document profile (V4): a live, script-mutated document.
//!
//! The content half of `pelt --engine scripted <url>`: a [`ScriptedDocument`] that
//! parses HTML into a live [`ScriptedDom`], runs its inline `<script>` through
//! [`script_runtime_api::Runtime`] on a chosen JS engine (Boa by default, Nova
//! behind `scripted-nova`), and renders the *mutated* DOM each frame through
//! `serval-render`. GPU-free and testable here; the windowed present loop drives it
//! exactly as it drives the static [`LoadedDocument`](crate::document::LoadedDocument).
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
use script_engine_api::ScriptEngine;
use script_runtime_api::Runtime;
use serval_layout::{inline_stylesheets, IncrementalLayout, ScrollKey, ScrollOffsets};
use serval_render::scene_from_session_dom;
use serval_static_dom::{StaticDocument, StaticNodeId};

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
}

impl<E: ScriptEngine> ScriptedDocument<E> {
    /// Fetch `url` through `fetcher`, parse it, and run its inline scripts. `Err` on a
    /// failed fetch or a runtime that would not initialize.
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
        let mut me = Self::parse(&String::from_utf8_lossy(&bytes))?;
        me.pending_fragment = fragment;
        Ok(me)
    }

    /// Parse already-loaded HTML into a live DOM, then run its inline `<script>`s
    /// against it (settling microtasks). The fetch-free half, for tests and inline
    /// `data:` content. `Err` if the runtime fails to initialize.
    pub fn parse(html: &str) -> Result<Self, String> {
        let doc = StaticDocument::parse(html);
        let mut sheets: Vec<String> =
            crate::STRUCTURAL_SHEET.iter().map(|s| s.to_string()).collect();
        sheets.extend(inline_stylesheets(&doc));

        let mut rt = Runtime::<E>::new().map_err(|e| format!("script runtime init: {e:?}"))?;
        // The parsed body becomes the live DOM, so script querying it (document.body,
        // getElementById, querySelector) sees the page's elements.
        rt.load_dom(&doc);
        // Run the page's inline scripts in document order; a script error is reported
        // but does not abort the load (a browser keeps rendering the document).
        for src in inline_scripts(&doc) {
            if let Err(e) = rt.eval(&src) {
                eprintln!("[pelt-scripted] script error: {e:?}");
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

/// Text of every inline `<script>` (no `src`) in document order. `<script src>` is a
/// V4 follow-up (the done-condition is inline script); skipping it here keeps a page
/// with an external script from silently running its inline siblings out of order.
fn inline_scripts(doc: &StaticDocument) -> Vec<String> {
    let mut out = Vec::new();
    collect_inline_scripts(doc, doc.document(), &mut out);
    out
}

fn collect_inline_scripts(dom: &StaticDocument, node: StaticNodeId, out: &mut Vec<String>) {
    if dom.element_name(node).is_some_and(|q| q.local.as_ref() == "script") {
        let has_src = dom
            .attribute(node, &Namespace::default(), &LocalName::from("src"))
            .is_some();
        if !has_src {
            let mut text = String::new();
            for child in dom.dom_children(node) {
                if let Some(t) = dom.text(child) {
                    text.push_str(t);
                }
            }
            if !text.trim().is_empty() {
                out.push(text);
            }
        }
    }
    for child in dom.dom_children(node) {
        collect_inline_scripts(dom, child, out);
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

    #[test]
    fn mutation_renders_on_boa() {
        mutation_renders::<BoaEngine>();
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
        fn scripted_content_scrolls_on_nova() {
            scripted_content_scrolls::<NovaEngine>();
        }
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
    }
}
