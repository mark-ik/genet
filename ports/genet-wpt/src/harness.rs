/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 3: run a `testharness.js` test and collect its per-subtest results.
//!
//! Extracts the test's own scripts (inline `<script>` + local `<script src>`,
//! skipping `testharness.js` / the report hook, which the host surface supplies),
//! then runs them against `testharness.js` on a fresh [`Runtime`] and reads the
//! results through the bridge ([`Runtime::run_testharness`]).
//!
//! Engine: selectable via `--engine boa|nova` (see [`Engine`]). Boa is the
//! pure-Rust conformance oracle; Nova is the native primary. The harness's
//! regex-incompatible source is shimmed host-side (`harness_regex_compat`), and
//! the WTF-8/UTF-16 string-indexing bugs that once panicked Nova are fixed in the
//! fork (`docs/2026-06-02_nova_wtf8_indexing_fixes.md`); both engines now produce
//! real numbers on the same corpus.
//!
//! Limitation: the test starts with an empty DOM. Tests that build their own DOM
//! (`createElement`) or are pure-JS run; tests that query elements declared in the
//! HTML body do not see them yet (parsing the body into the scripted DOM is a
//! later step).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use std::cell::RefCell;
use std::rc::Rc;

use genet_layout::{Applied, IncrementalLayout, inline_stylesheets};
use genet_livery::Device;
use genet_scripted::LiveryCssom;
use genet_scripted_dom::NodeId as DomNodeId;
use genet_static_dom::StaticDocument;
use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace};
use script_engine_api::ScriptEngine;
use script_runtime_api::{
    ComputedStyleHandler, FetchHandler, FetchOutcome, MediaQueryHandler, Runtime, TestResult,
    WebGlFactory,
};

thread_local! {
    /// One media-query evaluator per thread (the WPT 800x600 viewport, default
    /// media environment), shared across tests so `matchMedia` works without a
    /// per-test stylist build.
    static MEDIA_QUERY_EVAL: std::rc::Rc<genet_layout::MediaQueryEvaluator> =
        std::rc::Rc::new(genet_layout::MediaQueryEvaluator::new(VIEWPORT_W, VIEWPORT_H));
}

/// `matchMedia` seam for the WPT runner: evaluates against a default device.
struct WptMediaQueries(std::rc::Rc<genet_layout::MediaQueryEvaluator>);
impl MediaQueryHandler for WptMediaQueries {
    fn evaluate(&self, query: &str) -> (String, bool) {
        self.0.evaluate(query)
    }
}

/// A deferred `fetch()` completion, applied to the runtime by the drive loop. A
/// response streams as `StartStream` (status + headers) -> `Chunk`* (body) ->
/// `Close`, or `Error` if the body fails partway (e.g. a `Content-Encoding`
/// decode error: the response resolved, but body reads reject). `Fail` is a
/// network error before the headers.
pub enum FetchCompletion {
    StartStream(u64, FetchOutcome),
    Chunk(u64, Vec<u8>),
    Close(u64),
    Error(u64),
    Fail(u64, String),
}

/// A source of deferred fetch completions (the netfetch worker's channel). The
/// drive loop pulls completions and applies each via the callback. Disk / sync
/// runs pass `None` and never touch this.
pub trait CompletionSource {
    /// Apply every currently-ready completion; return how many were applied.
    fn drain(&self, apply: &mut dyn FnMut(FetchCompletion)) -> usize;
    /// Block up to `timeout` for one completion, then apply it; return 0 or 1.
    fn wait(&self, timeout: Duration, apply: &mut dyn FnMut(FetchCompletion)) -> usize;
}

/// Per-test wall-clock ceiling for the deferred drive loop: a test that awaits a
/// never-settling fetch fails (TIMEOUT) instead of hanging the runner.
const DRIVE_DEADLINE: Duration = Duration::from_secs(15);
/// Timers fired per drive turn before re-checking the completion channel.
const TIMER_BUDGET: u32 = 64;
/// The WPT viewport, in CSS px. One constant so the three consumers cannot
/// drift: the layout session, the media-query evaluator (`matchMedia`), and
/// `window.innerWidth`/`innerHeight` (which the wheel cluster computes its hit
/// point from).
const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: f32 = 600.0;

/// The drive loop's rendering cadence (one 60Hz frame, ms). In disk mode this is
/// a *virtual* step — rAF callbacks and the animation clock advance by it with no
/// sleeping, so a 2s animation costs evaluation time, not wall time. In server
/// mode it caps the idle wait while frames are pending.
const FRAME_MS: f64 = 1000.0 / 60.0;

/// Which JS engine the testharness runner drives. Boa is the pure-Rust
/// conformance oracle; Nova is the native primary. Both implement
/// `ScriptEngine`, so the harness path is generic — this only selects the
/// monomorphization (`--engine`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Engine {
    #[default]
    Boa,
    Nova,
}

/// Which cascade backs scripted CSSOM and `getComputedStyle` reads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StyleRoute {
    #[default]
    Stylo,
    Livery,
}

impl Engine {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "boa" => Some(Engine::Boa),
            "nova" => Some(Engine::Nova),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Engine::Boa => "boa",
            Engine::Nova => "nova",
        }
    }
}

/// Outcome of running one testharness test.
pub enum HarnessOutcome {
    /// The harness ran to completion; the per-subtest results (may be empty if the
    /// test reported none, e.g. an async test that never completed).
    Ran(Vec<TestResult>),
    /// The harness or the test threw before reporting — usually an unimplemented
    /// DOM/JS feature. Carries a concise message.
    Threw(String),
}

/// Where a test's `<script src>` resources come from. Disk mode reads files;
/// server mode HTTP-GETs them so `.sub.js` template substitution happens. The
/// harness / report-hook srcs are filtered out by the caller, not the loader.
pub trait ScriptSrcLoader {
    /// The contents of a non-harness `<script src>`, or `None` to skip it
    /// (unresolvable, remote-in-disk-mode, or fetch failed).
    fn load_script(&self, src: &str) -> Option<String>;
}

/// Disk loader: resolve `<script src>` against the test dir / tests root, read the
/// file. The default (no server). Remote and `data:` srcs are skipped.
pub struct DiskLoader<'a> {
    pub base_dir: &'a Path,
    pub tests_root: &'a Path,
}

impl ScriptSrcLoader for DiskLoader<'_> {
    fn load_script(&self, src: &str) -> Option<String> {
        let path = resolve(src, self.base_dir, self.tests_root)?;
        fs::read_to_string(path).ok()
    }
}

/// Run one testharness test HTML and collect its results, using `loader` to fetch
/// `<script src>` resources. `base_url` (when set) becomes the document base for
/// relative `fetch()` / `Request` URLs and populates `location`; `handler` (when
/// set) is the `fetch()` network seam. Disk mode passes `None`/`None`.
pub fn run_test(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    engine: Engine,
) -> HarnessOutcome {
    run_test_with_style(
        testharness_js,
        html,
        loader,
        base_url,
        handler,
        completion,
        engine,
        StyleRoute::Stylo,
    )
}

/// Run one test with an explicit scripted style route. The Livery route owns
/// `document.styleSheets` and `getComputedStyle`; the retained Stylo session
/// remains the geometry and animation driver until Livery replaces that half.
#[allow(clippy::too_many_arguments)]
pub fn run_test_with_style(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    engine: Engine,
    style: StyleRoute,
) -> HarnessOutcome {
    run_test_with_webgl_and_style(
        testharness_js,
        html,
        loader,
        base_url,
        handler,
        completion,
        None,
        engine,
        style,
    )
}

/// Like [`run_test`] but also installs a WebGL context factory, so a test that
/// calls `canvas.getContext('webgl')` (e.g. the Khronos conformance suite via
/// `webgl-test-utils.js`) draws against a real backend. The factory is minted
/// per `getContext`; pass `None` for the graphics-free default.
#[allow(clippy::too_many_arguments)]
pub fn run_test_with_webgl(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    webgl: Option<WebGlFactory>,
    engine: Engine,
) -> HarnessOutcome {
    run_test_with_webgl_and_style(
        testharness_js,
        html,
        loader,
        base_url,
        handler,
        completion,
        webgl,
        engine,
        StyleRoute::Stylo,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_test_with_webgl_and_style(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    webgl: Option<WebGlFactory>,
    engine: Engine,
    style: StyleRoute,
) -> HarnessOutcome {
    let doc = StaticDocument::parse(html);
    let mut scripts = Vec::new();
    collect_scripts(&doc, doc.document(), loader, &mut scripts);
    let test_src = scripts.join("\n;\n");

    match engine {
        Engine::Boa => run_with::<script_engine_boa::BoaEngine>(
            testharness_js,
            &test_src,
            &doc,
            base_url,
            handler,
            completion,
            webgl,
            style,
        ),
        Engine::Nova => run_with::<script_engine_nova::NovaEngine>(
            testharness_js,
            &test_src,
            &doc,
            base_url,
            handler,
            completion,
            webgl,
            style,
        ),
    }
}

/// A Nova runtime snapshotted after the host surface and `testharness.js` are
/// loaded. Each test clones this template, installs fresh host state, loads that
/// test's DOM, and runs only the test body.
pub struct NovaHarnessTemplate {
    rt: Runtime<script_engine_nova::NovaEngine>,
}

impl NovaHarnessTemplate {
    pub fn new(testharness_js: &str) -> Result<Self, String> {
        let mut rt = Runtime::<script_engine_nova::NovaEngine>::new()
            .map_err(|e| format!("runtime init: {e:?}"))?;
        rt.load_testharness(testharness_js)
            .map_err(|e| format!("testharness load: {e:?}"))?;
        Ok(Self { rt })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_test_with_webgl(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
        webgl: Option<WebGlFactory>,
    ) -> HarnessOutcome {
        self.run_test_with_webgl_and_style(
            html,
            loader,
            base_url,
            handler,
            completion,
            webgl,
            StyleRoute::Stylo,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_test_with_webgl_and_style(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
        webgl: Option<WebGlFactory>,
        style: StyleRoute,
    ) -> HarnessOutcome {
        let doc = StaticDocument::parse(html);
        let mut scripts = Vec::new();
        collect_scripts(&doc, doc.document(), loader, &mut scripts);
        let test_src = scripts.join("\n;\n");
        let mut rt = match self.rt.snapshot_clone() {
            Ok(rt) => rt,
            Err(e) => return HarnessOutcome::Threw(format!("runtime snapshot clone: {e:?}")),
        };
        prepare_runtime(&mut rt, &doc, base_url, handler, webgl);
        run_loaded_with(&mut rt, &test_src, completion, style)
    }

    pub fn run_test(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
    ) -> HarnessOutcome {
        self.run_test_with_webgl(html, loader, base_url, handler, completion, None)
    }

    pub fn run_test_with_style(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
        style: StyleRoute,
    ) -> HarnessOutcome {
        self.run_test_with_webgl_and_style(html, loader, base_url, handler, completion, None, style)
    }
}

/// Engine-generic core: build a `Runtime<E>`, load the test's body as the live
/// DOM, set the base URL + fetch handler if given, run the harness, collect
/// results. With a `completion` source (deferred / server mode) it drives the
/// event loop and the fetch-completion channel to quiescence (or a deadline)
/// itself, because deferred replies arrive out of band; without one it uses the
/// synchronous one-shot path.
#[allow(clippy::too_many_arguments)]
fn run_with<E: ScriptEngine>(
    testharness_js: &str,
    test_src: &str,
    doc: &StaticDocument,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    webgl: Option<WebGlFactory>,
    style: StyleRoute,
) -> HarnessOutcome {
    let mut rt = match Runtime::<E>::new() {
        Ok(rt) => rt,
        Err(e) => return HarnessOutcome::Threw(format!("runtime init: {e:?}")),
    };
    prepare_runtime(&mut rt, doc, base_url, handler, webgl);
    if let Err(e) = rt.load_testharness(testharness_js) {
        return HarnessOutcome::Threw(format!("testharness load: {e:?}"));
    }
    run_loaded_with(&mut rt, test_src, completion, style)
}

fn prepare_runtime<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    doc: &StaticDocument,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    webgl: Option<WebGlFactory>,
) {
    // The test's body becomes the live DOM, so scripts querying body elements
    // (getElementById / querySelector / document.body) see them.
    rt.load_dom(doc);
    if let Some(base) = base_url {
        let _ = rt.set_base_url(base);
    }
    if let Some(h) = handler {
        rt.set_fetch_handler(h);
    }
    // `window.matchMedia` over a default device, so css/mediaqueries tests can
    // parse + evaluate queries (they never render).
    rt.set_media_query_handler(Box::new(WptMediaQueries(
        MEDIA_QUERY_EVAL.with(|e| e.clone()),
    )));
    if let Some(factory) = webgl {
        rt.set_webgl_factory(factory);
    }
}

/// H7a (harness-exactness plan): the testharness lane's rendering session.
///
/// An [`IncrementalLayout`] over the runtime's live DOM, doing per drive turn
/// what a host's frame loop does: apply pending DOM mutations, pump rAF
/// callbacks, tick CSS animations/transitions on the drive clock, and dispatch
/// the harvested lifecycle events through the runtime. It also backs
/// `getComputedStyle` (the genet-scripted `ComputedStyleBridge` pattern), so
/// style reads return the session's cascade instead of nothing. Cheap when a
/// test never animates: every hook no-ops on an empty set.
struct RenderSession {
    layout: Rc<RefCell<Option<IncrementalLayout<DomNodeId>>>>,
    sheets: Vec<String>,
}

/// `getComputedStyle` over the session's retained cascade. One restyle stale by
/// construction (script runs before the next turn's apply), the standard
/// tradeoff of the script-before-layout split.
struct WptComputedStyle {
    layout: Rc<RefCell<Option<IncrementalLayout<DomNodeId>>>>,
}

impl ComputedStyleHandler for WptComputedStyle {
    fn computed_value(&self, node: u64, property: &str) -> Option<String> {
        self.layout
            .borrow()
            .as_ref()?
            .computed_value(DomNodeId::from_raw(node as usize), property)
    }
}

impl RenderSession {
    /// Build the session over the runtime's already-loaded DOM (before the test
    /// body runs, so early style reads see the parse-time cascade) and register
    /// the `getComputedStyle` bridge.
    fn new<E: ScriptEngine>(rt: &mut Runtime<E>, style: StyleRoute) -> Self {
        let layout = Rc::new(RefCell::new(None));
        let sheets;
        {
            let mut host = rt.host().borrow_mut();
            sheets = inline_stylesheets(&host.dom);
            // Discard the load-time mutation backlog: `new` cascades the DOM's
            // current state, so replaying it through `apply` would be double work.
            let mut discard = Vec::new();
            host.dom.drain_mutations(&mut discard);
            let refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
            *layout.borrow_mut() = Some(IncrementalLayout::new(
                &host.dom, &refs, VIEWPORT_W, VIEWPORT_H,
            ));
        }
        match style {
            StyleRoute::Stylo => rt.set_computed_style_handler(Box::new(WptComputedStyle {
                layout: layout.clone(),
            })),
            StyleRoute::Livery => {
                let refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
                LiveryCssom::install(rt, &refs, Device::screen(VIEWPORT_W, VIEWPORT_H));
            },
        }
        // `window.innerWidth`/`innerHeight` must agree with the session's
        // viewport: the wheel/scroll cluster computes its hit point from them
        // (`Math.floor(window.innerWidth / 2)`).
        rt.set_viewport_size(VIEWPORT_W, VIEWPORT_H);
        Self { layout, sheets }
    }

    /// Whether a declared CSS animation/transition is still live on the session
    /// clock — the drive loop keeps producing frames while true.
    fn animating(&self) -> bool {
        self.layout
            .borrow()
            .as_ref()
            .is_some_and(|l| l.has_active_animations())
    }

    /// One rendering turn at `now_ms`: rAF callbacks, mutation apply, animation
    /// tick, then transition + animation event dispatch. Returns how much work
    /// happened (0 = the turn was a no-op), which feeds the quiescence check.
    fn turn<E: ScriptEngine>(&self, rt: &mut Runtime<E>, now_ms: f64) -> usize {
        let mut work = rt.run_animation_frame_callbacks(now_ms).unwrap_or(0);
        let (t_events, a_events, restyled) = {
            let mut host = rt.host().borrow_mut();
            let mut layout = self.layout.borrow_mut();
            let Some(layout) = layout.as_mut() else {
                return work;
            };
            let refs: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
            let mut muts = Vec::new();
            host.dom.drain_mutations(&mut muts);
            let mut restyled = 0usize;
            if !muts.is_empty() {
                layout.apply(&host.dom, &refs, &muts);
                restyled += 1;
            }
            if layout.tick_animations(&host.dom, now_ms / 1000.0) != Applied::Unchanged {
                restyled += 1;
            }
            (
                layout.take_transition_events(&host.dom),
                layout.take_animation_events(&host.dom),
                restyled,
            )
        };
        // Dispatch off the borrow: listeners can mutate the DOM; the next turn
        // picks those mutations up.
        work += restyled + t_events.len() + a_events.len();
        for ev in t_events {
            let _ = rt.dispatch_transition_event(
                ev.node.raw(),
                ev.kind.event_type(),
                &ev.property_name,
                ev.elapsed_time,
            );
        }
        for ev in a_events {
            let _ = rt.dispatch_animation_event(
                ev.node.raw(),
                ev.kind.event_type(),
                &ev.animation_name,
                ev.elapsed_time,
            );
        }
        work
    }
}

/// H7b: drain and run one queued `test_driver.action_sequence` transaction, if
/// any. Returns how much work happened (0 = nothing queued).
///
/// The vendor JS queues `{actions, resolve, reject}`; this parses the spec
/// Actions JSON with the pinned protocol types, runs it through the shared tick
/// interpreter (element origins resolved through the rendering session's
/// geometry — the interpreter's caller-owns-the-tree rule), dispatches each
/// tick's events at hit-tested nodes, advances the virtual clock by the tick's
/// reported duration (a wall-clock drive collapses ticks instead: reported,
/// never slept), and settles the Promise.
fn process_testdriver_actions<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    render: &RenderSession,
    now_ms: &mut f64,
    virtual_clock: bool,
) -> usize {
    let json = match rt.eval("String(typeof __tdTake === 'function' ? __tdTake() : '')") {
        Ok(v) => rt.value_to_string(&v).unwrap_or_default(),
        Err(_) => return 0,
    };
    if json.is_empty() {
        return 0;
    }
    let settle = |rt: &mut Runtime<E>, err: Option<String>| {
        let call = match err {
            Some(m) => format!("__tdSettle({:?})", m),
            None => "__tdSettle()".to_string(),
        };
        let _ = rt.eval(&call);
        let _ = rt.run_microtasks();
    };
    let sequences: Vec<webdriver::actions::ActionSequence> = match serde_json::from_str(&json) {
        Ok(s) => s,
        Err(e) => {
            settle(rt, Some(format!("malformed actions payload: {e}")));
            return 1;
        },
    };
    // The resolver holds only Rc clones, so `rt` stays free for dispatch below.
    let host_rc = rt.host().clone();
    let layout_rc = render.layout.clone();
    let resolver = move |reference: &str| -> Option<(f64, f64)> {
        let raw: usize = reference.parse().ok()?;
        let node = DomNodeId::from_raw(raw);
        let host = host_rc.borrow();
        let layout = layout_rc.borrow();
        let (x, y, w, h) = layout.as_ref()?.absolute_rect(&host.dom, node)?;
        Some((f64::from(x + w / 2.0), f64::from(y + h / 2.0)))
    };
    let ticks = match embedder_traits::webdriver_actions::interpret_actions(&sequences, &resolver) {
        Ok(t) => t,
        Err(e) => {
            settle(rt, Some(format!("{e:?}")));
            return 1;
        },
    };
    // Per Touch Events, every event for one touch point goes to the element the
    // touch *started* on, not to whatever is under the finger now. Remember that
    // target per touch id for the life of the transaction.
    let mut touch_targets: std::collections::HashMap<i32, DomNodeId> =
        std::collections::HashMap::new();
    for tick in ticks {
        for event in tick.events {
            dispatch_input_event(rt, render, &event, &mut touch_targets);
        }
        if virtual_clock {
            *now_ms += tick.duration_ms as f64;
        }
        let _ = render.turn(rt, *now_ms);
    }
    settle(rt, None);
    1
}

/// Map one interpreter [`InputEvent`] onto DOM events at the hit-tested node.
///
/// Touch and wheel go through the **typed** bridges
/// ([`Runtime::dispatch_touch_event`] / [`dispatch_wheel_event`]), which build a
/// real `TouchEvent` / `WheelEvent` and — critically — decide `cancelable` from
/// the DOM's own listener set (a UA input event is cancelable only if a
/// non-passive listener for its type is on the path). Mouse/pointer still
/// dispatch by type only; giving them coordinates and a typed `MouseEvent` is a
/// follow-on, not needed by the cluster this serves.
///
/// Keyboard events are not dispatched yet (the interpreter emits them).
fn dispatch_input_event<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    render: &RenderSession,
    event: &embedder_traits::input_events::InputEvent,
    touch_targets: &mut std::collections::HashMap<i32, DomNodeId>,
) {
    use embedder_traits::WebViewPoint;
    use embedder_traits::input_events::{InputEvent, MouseButtonAction, TouchEventType};
    let xy = |p: &WebViewPoint| match p {
        WebViewPoint::Page(pt) => (pt.x, pt.y),
        WebViewPoint::Device(pt) => (pt.x, pt.y),
    };
    let hit = |rt: &Runtime<E>, (x, y): (f32, f32)| -> Option<DomNodeId> {
        let host = rt.host().borrow();
        let layout = render.layout.borrow();
        layout
            .as_ref()
            .and_then(|l| l.hit_test(&host.dom, x, y, &Default::default()))
    };

    match event {
        InputEvent::Touch(t) => {
            let (x, y) = xy(&t.point);
            let id = t.touch_id.0;
            let (kind, target) = match t.event_type {
                TouchEventType::Down => {
                    let Some(target) = hit(rt, (x, y)) else {
                        return;
                    };
                    // Every later event for this touch point goes here.
                    touch_targets.insert(id, target);
                    ("touchstart", target)
                },
                // Move/Up/Cancel go to the touchstart target, not a fresh
                // hit-test: the finger owns the element it landed on.
                TouchEventType::Move => match touch_targets.get(&id) {
                    Some(&target) => ("touchmove", target),
                    None => return,
                },
                TouchEventType::Up => match touch_targets.remove(&id) {
                    Some(target) => ("touchend", target),
                    None => return,
                },
                TouchEventType::Cancel => match touch_targets.remove(&id) {
                    Some(target) => ("touchcancel", target),
                    None => return,
                },
            };
            let _ = rt.dispatch_touch_event(target.raw(), kind, f64::from(x), f64::from(y), id);
        },
        InputEvent::Wheel(w) => {
            let (x, y) = xy(&w.point);
            let Some(target) = hit(rt, (x, y)) else {
                return;
            };
            // The interpreter negates the spec's deltas (its positive WheelDelta
            // scrolls the view up); the DOM event carries the spec sign, so
            // negate back.
            let _ = rt.dispatch_wheel_event(
                target.raw(),
                f64::from(x),
                f64::from(y),
                -w.delta.x,
                -w.delta.y,
                0, // DOM_DELTA_PIXEL
            );
        },
        InputEvent::MouseMove(m) => {
            let p = xy(&m.point);
            let Some(target) = hit(rt, p) else { return };
            for kind in ["pointermove", "mousemove"] {
                let _ = rt.dispatch_event(target.raw(), kind);
            }
        },
        InputEvent::MouseButton(b) => {
            let p = xy(&b.point);
            let Some(target) = hit(rt, p) else { return };
            let kinds: &[&str] = match b.action {
                MouseButtonAction::Down => &["pointerdown", "mousedown"],
                MouseButtonAction::Up => &["pointerup", "mouseup", "click"],
            };
            for kind in kinds {
                let _ = rt.dispatch_event(target.raw(), kind);
            }
        },
        _ => {},
    }
}

fn run_loaded_with<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    test_src: &str,
    completion: Option<&dyn CompletionSource>,
    style: StyleRoute,
) -> HarnessOutcome {
    // H7a: every testharness run gets a rendering session. Built before the test
    // body runs so early `getComputedStyle` reads see the parse-time cascade.
    let render = RenderSession::new(rt, style);
    if let Err(e) = rt.begin_loaded_testharness(test_src) {
        return HarnessOutcome::Threw(truncate(&format!("{e:?}"), 200));
    }
    rt.run_microtasks();
    match completion {
        None => drive_virtual(rt, &render),
        Some(cs) => drive_wall(rt, &render, cs),
    }
}

/// Disk-mode drive: all three clocks (timers, rAF, the animation clock) are
/// **virtual** — each turn advances `now_ms` to the next frame while anything
/// frame-paced pends, else jumps straight to the next timer's due time, and
/// never sleeps. Timer firing order matches the old eager path (due-time order),
/// so pure-timer tests score identically; what's new is that rAF-paced and
/// animating tests make progress at evaluation speed rather than never. The
/// wall-clock deadline still backstops runaway loops (a self-rearming 0ms timer,
/// an rAF loop that never exits).
fn drive_virtual<E: ScriptEngine>(rt: &mut Runtime<E>, render: &RenderSession) -> HarnessOutcome {
    let start = Instant::now();
    let mut now_ms = 0.0f64;
    loop {
        if start.elapsed() >= DRIVE_DEADLINE {
            rt.fail_all_pending("test timed out");
            break;
        }
        rt.run_microtasks();
        let fired = rt.run_timers(TIMER_BUDGET, now_ms);
        let rendered = render.turn(rt, now_ms);
        let acted = process_testdriver_actions(rt, render, &mut now_ms, true);
        let has_raf = rt.has_animation_frame_callbacks();
        let animating = render.animating();
        let next_timer = rt.next_timer_delay();
        if fired == 0
            && rendered == 0
            && acted == 0
            && !has_raf
            && !animating
            && next_timer.is_none()
        {
            break; // quiescent: no timer, no frame consumer, nothing happened
        }
        // Advance the virtual clock: to the sooner of the next frame and the next
        // timer while frames are wanted, else straight to the next timer. A turn
        // that did work but scheduled nothing time-based loops once more at the
        // same instant and then quiesces.
        now_ms += if has_raf || animating {
            match next_timer {
                Some(d) if d < FRAME_MS => d.max(0.0),
                _ => FRAME_MS,
            }
        } else if let Some(d) = next_timer {
            d.max(0.0)
        } else {
            0.0
        };
    }
    HarnessOutcome::Ran(rt.results())
}

/// Server-mode drive: timers fire on a real-time gate (virtual clock = elapsed
/// wall ms), so a short abort timer fires at its delay while the far-future
/// testharness timeout stays pending, and fetch completions arrive out of band
/// on the channel. Each turn additionally runs a rendering turn; idle waits are
/// capped at one frame while rAF callbacks or animations pend.
fn drive_wall<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    render: &RenderSession,
    cs: &dyn CompletionSource,
) -> HarnessOutcome {
    let start = Instant::now();
    let elapsed_ms =
        |start: Instant| (Instant::now().saturating_duration_since(start)).as_millis() as f64;
    loop {
        if elapsed_ms(start) >= DRIVE_DEADLINE.as_millis() as f64 {
            rt.fail_all_pending("test timed out");
            break;
        }
        rt.run_microtasks();
        let applied = cs.drain(&mut |c| match c {
            FetchCompletion::StartStream(id, o) => rt.start_stream(id, o),
            FetchCompletion::Chunk(id, b) => rt.push_chunk(id, &b),
            FetchCompletion::Close(id) => rt.close_stream(id),
            FetchCompletion::Error(id) => rt.error_stream(id),
            FetchCompletion::Fail(id, m) => rt.fail_fetch(id, &m),
        });
        let fired = rt.run_timers(TIMER_BUDGET, elapsed_ms(start));
        let rendered = render.turn(rt, elapsed_ms(start));
        // Wall-clock drive: tick durations are collapsed, not slept (the
        // interpreter reports them; a headed consumer would pace real frames).
        let mut wall_now = elapsed_ms(start);
        let acted = process_testdriver_actions(rt, render, &mut wall_now, false);
        let has_raf = rt.has_animation_frame_callbacks();
        let animating = render.animating();
        let pending = rt.pending_fetches();
        let next_timer = rt.next_timer_delay();
        if pending == 0
            && next_timer.is_none()
            && fired == 0
            && applied == 0
            && rendered == 0
            && acted == 0
            && !has_raf
            && !animating
        {
            break; // quiescent: no fetch, no timer, no frame consumer
        }
        if fired == 0 && applied == 0 && rendered == 0 && acted == 0 {
            // Nothing ran this turn. Sleep until the next event: a fetch
            // completion, the next timer's due time, or — while frames are
            // wanted — at most one frame.
            let remaining = (DRIVE_DEADLINE.as_millis() as f64 - elapsed_ms(start)).max(0.0);
            let mut wait_ms = remaining;
            if let Some(d) = next_timer {
                wait_ms = wait_ms.min(d);
            }
            if has_raf || animating {
                wait_ms = wait_ms.min(FRAME_MS);
            }
            if pending > 0 {
                // Wake on a completion or after the slice (whichever first).
                cs.wait(
                    Duration::from_millis(wait_ms.ceil() as u64),
                    &mut |c| match c {
                        FetchCompletion::StartStream(id, o) => rt.start_stream(id, o),
                        FetchCompletion::Chunk(id, b) => rt.push_chunk(id, &b),
                        FetchCompletion::Error(id) => rt.error_stream(id),
                        FetchCompletion::Close(id) => rt.close_stream(id),
                        FetchCompletion::Fail(id, m) => rt.fail_fetch(id, &m),
                    },
                );
            } else if wait_ms > 0.0 {
                // Only timers/frames are outstanding: sleep until the soonest.
                std::thread::sleep(Duration::from_millis(wait_ms.ceil() as u64));
            }
        }
    }
    HarnessOutcome::Ran(rt.results())
}

/// Walk the document collecting test scripts in document order: inline `<script>`
/// text, and the contents of `<script src>` from `loader` (skipping the harness /
/// report hook, which the host surface supplies).
fn collect_scripts<D: LayoutDom>(
    dom: &D,
    node: D::NodeId,
    loader: &dyn ScriptSrcLoader,
    out: &mut Vec<String>,
) {
    if dom
        .element_name(node)
        .is_some_and(|q| q.local.as_ref() == "script")
    {
        match dom.attribute(node, &Namespace::default(), &LocalName::from("src")) {
            // WPT's deliberately-blank vendor hook: splice genet's automation
            // backend in its place (H7b), in document order.
            Some(src) if is_testdriver_vendor_src(src) => {
                out.push(TESTDRIVER_VENDOR_JS.to_string());
            },
            Some(src) if !is_harness_src(src) => {
                if let Some(text) = loader.load_script(src) {
                    out.push(text);
                }
            },
            Some(_) => {}, // the harness / report hook: the host surface supplies these
            None => {
                let mut text = String::new();
                for child in dom.dom_children(node) {
                    if let Some(t) = dom.text(child) {
                        text.push_str(t);
                    }
                }
                if !text.trim().is_empty() {
                    out.push(text);
                }
            },
        }
    }
    for child in dom.dom_children(node) {
        collect_scripts(dom, child, loader, out);
    }
}

/// `testharness.js` and its report hook are supplied by the host surface (the
/// results bridge replaces the report), so the test's own copies are skipped.
fn is_harness_src(src: &str) -> bool {
    let s = src.split(['#', '?']).next().unwrap_or(src);
    s.ends_with("testharness.js")
        || s.ends_with("testharnessreport.js")
        || s.ends_with("testharnesscss.css")
}

/// Whether a `<script src>` is WPT's `testdriver-vendor.js` — the file WPT ships
/// deliberately blank so the vendor supplies its automation backend. genet's
/// backend ([`TESTDRIVER_VENDOR_JS`]) is spliced in its place, in document order,
/// exactly where the real vendor file would run (after `testdriver.js` defines
/// the throwing defaults, before any test script calls them).
fn is_testdriver_vendor_src(src: &str) -> bool {
    let s = src.split(['#', '?']).next().unwrap_or(src);
    s.ends_with("testdriver-vendor.js")
}

/// genet's `testdriver-vendor.js` (H7b): the in-process WebDriver-Actions
/// backend. `action_sequence` normalizes element origins to the wire
/// element-reference format carrying the node's raw ref (only the host can
/// resolve geometry), queues the transaction, and returns a Promise the drive
/// loop settles after running the ticks through the shared interpreter
/// (`embedder_traits::webdriver_actions::interpret_actions`). Commands beyond
/// `action_sequence` keep their spec-honest throwing defaults;
/// `in_automation = true` makes them throw instead of waiting forever for a
/// human.
const TESTDRIVER_VENDOR_JS: &str = r#"
(function() {
  if (typeof window === 'undefined' || !window.test_driver_internal) { return; }
  globalThis.__tdQueue = [];
  globalThis.__tdActive = null;
  globalThis.__tdTake = function() {
    if (globalThis.__tdActive || globalThis.__tdQueue.length === 0) { return ''; }
    globalThis.__tdActive = globalThis.__tdQueue.shift();
    return JSON.stringify(globalThis.__tdActive.actions);
  };
  globalThis.__tdSettle = function(err) {
    var e = globalThis.__tdActive;
    globalThis.__tdActive = null;
    if (!e) { return; }
    if (err) { e.reject(new Error(String(err))); } else { e.resolve(); }
  };
  window.test_driver_internal.in_automation = true;
  window.test_driver_internal.action_sequence = function(actions) {
    var fixed = (actions || []).map(function(src) {
      var copy = {};
      for (var k in src) { copy[k] = src[k]; }
      if (src.actions) {
        copy.actions = src.actions.map(function(a) {
          var b = {};
          for (var j in a) { b[j] = a[j]; }
          if (b.origin && typeof b.origin === 'object' && b.origin.__ref !== undefined) {
            // The wrapper's __ref is the JS-opaque reflector; __nodeRawId is
            // the native reverse of __reflectNode, yielding the raw node id
            // the host resolves through its layout session.
            b.origin = { 'element-6066-11e4-a52e-4f735466cecf': String(__nodeRawId(b.origin.__ref)) };
          }
          return b;
        });
      }
      return copy;
    });
    return new Promise(function(resolve, reject) {
      globalThis.__tdQueue.push({ actions: fixed, resolve: resolve, reject: reject });
    });
  };
})();
"#;

/// Resolve a local `<script src>` to a path (`/`-absolute against the tests root,
/// else relative to the test dir). Remote / `data:` srcs return `None`.
fn resolve(src: &str, base_dir: &Path, tests_root: &Path) -> Option<PathBuf> {
    let src = src.split(['#', '?']).next().unwrap_or(src).trim();
    if src.is_empty()
        || src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("//")
        || src.starts_with("data:")
    {
        return None;
    }
    Some(match src.strip_prefix('/') {
        Some(rest) => tests_root.join(rest),
        None => base_dir.join(src),
    })
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyLoader;

    impl ScriptSrcLoader for EmptyLoader {
        fn load_script(&self, _src: &str) -> Option<String> {
            None
        }
    }

    const MINI_TESTHARNESS: &str = r#"
var __tests = [];
var __completion_callbacks = [];
function setup() {}
function add_completion_callback(cb) { __completion_callbacks.push(cb); }
function assert_true(value, message) {
  if (!value) throw new Error(message || "assert_true failed");
}
function test(fn, name) {
  try {
    fn();
    __tests.push({ name: name, status: 0, message: null });
  } catch (e) {
    __tests.push({ name: name, status: 1, message: String((e && e.message) || e) });
  }
}
window.addEventListener("load", function() {
  var snapshot = __tests.slice();
  for (var i = 0; i < __completion_callbacks.length; i++) {
    __completion_callbacks[i](snapshot);
  }
});
"#;

    #[test]
    fn livery_style_route_exposes_cssom_to_testharness() {
        let html = r#"<!doctype html>
<style>.card { --accent: #ff0000; color: var(--accent); }</style>
<div id="card" class="card"></div>
<script>
test(function() {
  var card = document.getElementById('card');
  var sheet = document.styleSheets[0];
  assert_true(document.styleSheets.length === 1, 'one author sheet');
  assert_true(sheet.cssRules.length === 1, 'one initial rule');
  assert_true(getComputedStyle(card).color === '#ff0000', 'initial color');
  assert_true(sheet.insertRule('.card { --accent: #0000ff; }', 1) === 1, 'insert index');
  assert_true(getComputedStyle(card).color === '#0000ff', 'mutated color');
  assert_true(getComputedStyle(card).getPropertyValue('--accent') === '#0000ff', 'custom value');
  sheet.deleteRule(1);
  assert_true(getComputedStyle(card).color === '#ff0000', 'deleted rule');
}, 'Livery CSSOM composes through the WPT harness');
</script>"#;
        let results = unwrap_ran(run_test_with_style(
            MINI_TESTHARNESS,
            html,
            &EmptyLoader,
            None,
            None,
            None,
            Engine::Boa,
            StyleRoute::Livery,
        ));
        assert_eq!(results.len(), 1, "one subtest: {results:?}");
        assert!(results[0].passed(), "Livery route should pass: {results:?}");
    }

    /// End to end through the H7a rendering session: the real WPT
    /// `animationevent-types.html` (negative delay, iteration-count 2, animates
    /// `left`) runs to completion without panicking. This test found the stylo
    /// f32 boundary hole (fork fix `56e70cacdb`) — the harness's silent panic
    /// hook had reduced it to an opaque `ERROR panic` in the corpus run, so it
    /// stays here where a panic gets a backtrace.
    #[test]
    fn animationevent_types_survives_the_rendering_session() {
        let wpt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
        let testharness_js =
            fs::read_to_string(wpt.join("tests/resources/testharness.js")).expect("harness");
        let test_path = wpt.join("tests/css/css-animations/animationevent-types.html");
        let html = fs::read_to_string(&test_path).expect("test html");
        let base_dir = test_path.parent().unwrap().to_path_buf();
        let tests_root = wpt.join("tests");
        let loader = DiskLoader {
            base_dir: base_dir.as_path(),
            tests_root: tests_root.as_path(),
        };
        let outcome = run_test(
            &testharness_js,
            &html,
            &loader,
            None,
            None,
            None,
            Engine::Boa,
        );
        match outcome {
            // Subtest passes/failures are the corpus baseline's business; this
            // guard only pins "the session does not take the process down".
            HarnessOutcome::Ran(results) => {
                assert!(
                    !results.is_empty(),
                    "the animation events should reach testharness"
                );
            },
            HarnessOutcome::Threw(m) => panic!("threw instead of reporting: {m}"),
        }
    }

    /// H7b end to end, and the first cross-consumer test of the shared Actions
    /// tick interpreter: a WPT-shaped test loads the real `testdriver.js`, the
    /// vendor seam splices genet's backend, `test_driver.Actions()`-format JSON
    /// queues through `action_sequence`, the interpreter resolves the element
    /// origin through the rendering session's geometry, and the synthesized
    /// pointerdown/up/click land on the hit-tested node — completing the
    /// async_test with zero coordinate literals and zero sleeps.
    #[test]
    fn test_driver_action_sequence_synthesizes_a_click() {
        let wpt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
        let testharness_js =
            fs::read_to_string(wpt.join("tests/resources/testharness.js")).expect("harness");
        let tests_root = wpt.join("tests");
        let base_dir = tests_root.join("dom");
        let loader = DiskLoader {
            base_dir: base_dir.as_path(),
            tests_root: tests_root.as_path(),
        };
        let html = r#"<!DOCTYPE html>
<script src="/resources/testdriver.js"></script>
<script src="/resources/testdriver-vendor.js"></script>
<div id="target" style="position:absolute;left:0;top:0;width:100px;height:100px"></div>
<script>
async_test(function(t) {
  var el = document.getElementById('target');
  el.addEventListener('click', t.step_func_done(function() {}));
  var actions = [{
    type: 'pointer', id: 'p1', parameters: { pointerType: 'mouse' },
    actions: [
      { type: 'pointerMove', duration: 0, origin: el, x: 0, y: 0 },
      { type: 'pointerDown', button: 0 },
      { type: 'pointerUp', button: 0 },
    ],
  }];
  window.test_driver_internal.action_sequence(actions).then(
    function() {},
    function(e) {
      t.step_func(function() {
        assert_unreached('action_sequence rejected: ' + (e && e.message));
      })();
    });
}, 'a synthesized click reaches the element listener');
</script>"#;
        let results = unwrap_ran(run_test(
            &testharness_js,
            html,
            &loader,
            None,
            None,
            None,
            Engine::Boa,
        ));
        assert_eq!(results.len(), 1, "one subtest: {results:?}");
        assert!(
            results[0].passed(),
            "the synthesized click should complete the async_test: {results:?}"
        );
    }

    /// The input path end to end (H9), and the rule the whole
    /// `non-cancelable-when-passive` cluster exists to pin: a **touch** pointer
    /// action produces a real `TouchEvent` at the element, and its `cancelable`
    /// is decided by whether a **non-passive** listener for that type is on the
    /// propagation path. Passive-only => not cancelable; any non-passive =>
    /// cancelable.
    ///
    /// Runs both halves in one file so a regression in either direction (always
    /// cancelable / never cancelable) fails.
    #[test]
    fn a_touch_action_produces_a_touchevent_whose_cancelable_respects_passive() {
        let wpt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
        let testharness_js =
            fs::read_to_string(wpt.join("tests/resources/testharness.js")).expect("harness");
        let tests_root = wpt.join("tests");
        let base_dir = tests_root.join("dom");
        let loader = DiskLoader {
            base_dir: base_dir.as_path(),
            tests_root: tests_root.as_path(),
        };
        let page = |passive: bool| {
            format!(
                r#"<!DOCTYPE html>
<script src="/resources/testdriver.js"></script>
<script src="/resources/testdriver-actions.js"></script>
<script src="/resources/testdriver-vendor.js"></script>
<div id="t" style="position:absolute;left:0;top:0;width:200px;height:200px"></div>
<script>
async_test(function(t) {{
  var el = document.getElementById('t');
  el.addEventListener('touchstart', t.step_func_done(function(e) {{
    assert_true(e instanceof TouchEvent, 'a real TouchEvent');
    assert_equals(e.cancelable, {expected}, 'cancelable follows the non-passive rule');
    assert_equals(e.changedTouches.length, 1, 'carries the touch point');
  }}), {{ passive: {passive} }});
  new test_driver.Actions()
    .addPointer('finger', 'touch')
    .pointerMove(0, 0, {{ origin: el }})
    .pointerDown()
    .pointerUp()
    .send();
}}, 'touchstart cancelable with passive={passive}');
</script>"#,
                passive = passive,
                // A passive-only listener cannot preventDefault, so the UA marks
                // the event non-cancelable.
                expected = !passive,
            )
        };
        for passive in [false, true] {
            let results = unwrap_ran(run_test(
                &testharness_js,
                &page(passive),
                &loader,
                None,
                None,
                None,
                Engine::Boa,
            ));
            assert_eq!(
                results.len(),
                1,
                "passive={passive}: one subtest: {results:?}"
            );
            assert!(results[0].passed(), "passive={passive}: {:?}", results[0]);
        }
    }

    fn unwrap_ran(outcome: HarnessOutcome) -> Vec<TestResult> {
        match outcome {
            HarnessOutcome::Ran(results) => results,
            HarnessOutcome::Threw(message) => panic!("harness threw: {message}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn nova_harness_template_reuses_loaded_harness_without_result_leak() {
        let mut template = NovaHarnessTemplate::new(MINI_TESTHARNESS).expect("template");
        let loader = EmptyLoader;

        let first = unwrap_ran(template.run_test(
            r#"<script>test(function(){ assert_true(true); }, "first");</script>"#,
            &loader,
            Some("file:///first.html"),
            None,
            None,
        ));
        let second = unwrap_ran(template.run_test(
            r#"<script>test(function(){ assert_true(true); }, "second");</script>"#,
            &loader,
            Some("file:///second.html"),
            None,
            None,
        ));

        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].name, "first");
        assert!(first[0].passed(), "{first:?}");
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(second[0].name, "second");
        assert!(second[0].passed(), "{second:?}");
    }
}

/// Microbench: where does per-test time go? Times, over N iterations,
/// (a) `Runtime::new()` (the host bootstrap a pool would amortize), (b) the same
/// plus `eval(testharness.js)` (the harness re-eval a pool would *also* amortize),
/// and (c) a full `run_test` of a small testharness file. The deltas say whether a
/// reuse-pool is worth its isolation cost, and which eval dominates.
pub fn bench(tests_root: &str) {
    use std::time::Instant;
    // bench is a Boa-specific perf probe (Runtime::new / harness-eval / full run
    // timings); it doesn't vary by engine, so it names Boa directly.
    use script_engine_boa::BoaEngine;
    let root = Path::new(tests_root);
    let testharness_js = match fs::read_to_string(root.join("resources/testharness.js")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("bench: testharness.js not found under {tests_root}/resources");
            std::process::exit(2);
        },
    };
    let n = 50;

    // (a) Runtime::new() only.
    let t = Instant::now();
    for _ in 0..n {
        let _rt = Runtime::<BoaEngine>::new().expect("new");
    }
    let new_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (b) new() + eval(testharness.js).
    let t = Instant::now();
    for _ in 0..n {
        let mut rt = Runtime::<BoaEngine>::new().expect("new");
        rt.eval(&testharness_js).expect("harness eval");
    }
    let new_harness_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (c) a full run_test on a trivial inline testharness test.
    let html = "<!doctype html><script src=/resources/testharness.js></script>\
                <script>test(function(){ assert_true(true); }, 'x');</script>";
    let loader = DiskLoader {
        base_dir: root,
        tests_root: root,
    };
    let t = Instant::now();
    for _ in 0..n {
        let _ = run_test(
            &testharness_js,
            html,
            &loader,
            None,
            None,
            None,
            Engine::Boa,
        );
    }
    let run_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (d) Isolation probe: can one Runtime run two harness evals back-to-back
    // without the `tests` singleton leaking results across them? If a re-eval
    // resets cleanly, a pooled-Runtime (re-eval harness per test) is safe.
    let mut rt = Runtime::<BoaEngine>::new().expect("new");
    let r1 = rt.run_testharness(
        &testharness_js,
        "test(function(){ assert_true(true); }, 'a');",
    );
    let r2 = rt.run_testharness(
        &testharness_js,
        "test(function(){ assert_true(true); }, 'b');",
    );
    let leak = match (&r1, &r2) {
        (Ok(a), Ok(b)) => format!(
            "run1={} subtests, run2={} subtests (want 1 and 1; >1 = leak)",
            a.len(),
            b.len()
        ),
        _ => "a run errored".to_string(),
    };

    println!("bench (Boa, {n} iters, ms/iter):");
    println!("  (a) Runtime::new()                  {new_ms:8.2}");
    println!(
        "  (b) new() + eval(testharness.js)    {new_harness_ms:8.2}  (harness eval = {:.2})",
        new_harness_ms - new_ms
    );
    println!("  (c) full run_test (trivial test)    {run_ms:8.2}");
    println!("  (d) reuse isolation: {leak}");
    println!(
        "\nFinding: the dominant per-test cost is the harness eval (~{:.0} ms), not\n\
         Runtime::new() (~{:.0} ms). Reusing a Runtime across tests LEAKS — testharness's\n\
         `tests` singleton accumulates across re-evals (see (d)) — so realm-reuse is\n\
         incorrect without a reset. Correct amortization needs a post-(harness-eval)\n\
         snapshot cloned per test (a fresh `tests` each time): the GcAgent::clone path.",
        new_harness_ms - new_ms,
        new_ms,
    );
}
