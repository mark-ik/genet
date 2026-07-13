// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Browser host surface for genet's scripted tier (the plan's Layer 1).
//!
//! Built ON the engine-neutral [`script_engine_api`] VM primitives
//! (`set_global` / `set_function` / host data), generic over the backend. The
//! "browser-ish" concepts — global scope (`self` / `window`), `console`, and
//! later the event loop, `EventTarget`, and `document` — are assembled here, not
//! in `script-engine-api`. If `document` or a timer ever appears in the VM trait,
//! the layering has failed.
//!
//! Present: aggregated [`HostState`] (the engine's single host-data slot) for
//! native sinks; global aliases (`self` / `window`); `console`; a cooperative
//! **event loop** (`setTimeout` / `setInterval` / `clear*`, drained by
//! [`Runtime::run_event_loop`]); **EventTarget** / `Event` (`addEventListener` /
//! `removeEventListener` / `dispatchEvent`); and the **`document` / `Node`
//! construction surface** (the `dom` module) — `createElement`, `createTextNode`,
//! `appendChild`, `setAttribute`, `textContent` (setter), `getElementById` —
//! bound to a [`genet_scripted_dom::ScriptedDom`] in host state. The event loop,
//! EventTarget, and DOM wrappers are JS bootstraps composed on the engine
//! primitives (the rakers lesson); the DOM mutators are native sinks reached the
//! same way as `console`. The only VM-trait growth needed was
//! `CallCx::make_reflector` (mint an outgoing node), added alongside the existing
//! `reflector_data` (recover an incoming one).
//!
//! Present: global scope (`self` / `window`), `console`, the event loop
//! (`setTimeout` / `setInterval`, drained by `run_event_loop` with microtask
//! checkpoints), global + node-level `EventTarget` with tree propagation,
//! `postMessage`, the `document` / `Node` surface, and a `testharness.js` results
//! bridge ([`Runtime::run_testharness`]). See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::LayoutDom;
use script_engine_api::{CallCx, NativeFn, ScriptEngine, ScriptEngineSnapshot};
use genet_scripted_dom::{NodeId, ScriptedDom};

mod dom;
mod fetch;
mod harness;
mod platform;
mod selector;
mod webgl;

pub use dom::{ComputedStyleHandler, CookieProvider, MediaQueryHandler};
pub use fetch::{FetchHandler, FetchOutcome, FetchRequest};
pub use harness::TestResult;
pub use platform::StorageProvider;
pub use webgl::{WebGlFactory, WebGlHandler};

/// State the runtime's native callbacks share, stored as the engine's single
/// host-data slot (`Rc<dyn Any>`). One aggregate so every host object reaches the
/// same place; grows as host objects are added (the event-loop task queue and
/// `EventTarget` listeners land here as they graduate from JS bootstraps).
#[derive(Default)]
pub struct HostState {
    /// `console.log` / `console.error` output, in call order.
    pub console: Vec<String>,
    /// The document (viewport) scroll offset in CSS px, the script side of
    /// `window.scrollX|Y` / `scrollTo` / `scrollBy`. The host owns the real
    /// viewport (it lays out and knows the scroll range): each frame it syncs the
    /// current clamped scroll *in* before running script, and after the run reads
    /// back the value script set, clamps it against its scroll range, and applies
    /// it (`IncrementalLayout::set_viewport_scroll`). The runtime never clamps (it
    /// does not lay out), so a `scrollX` read within the same run that set an
    /// out-of-range value sees the unclamped value until the host reconciles — the
    /// script/layout split's one fidelity gap.
    pub viewport_scroll: (f32, f32),
    /// The viewport's CSS-pixel size, the script side of `window.innerWidth` /
    /// `innerHeight`. The host owns it (it lays out); set it with
    /// [`Runtime::set_viewport_size`] whenever the surface resizes. Defaults to
    /// the 800x600 the runtime's media-query evaluator already assumes, so a host
    /// that never sets it is at least self-consistent.
    pub viewport_size: (f32, f32),
    /// A pending `Element.scrollIntoView()` target, or `None`. The runtime cannot
    /// compute the element's laid-out position (it does not lay out), so it records
    /// the node; after the run the host resolves it (`IncrementalLayout::scroll_to_element`,
    /// block-start), updating the viewport scroll, and clears this. Set by the latest
    /// `scrollIntoView` in a run and cleared by any `scrollTo` / `scrollBy` (so the
    /// last scroll command wins); the host applies this when set, else
    /// [`viewport_scroll`](Self::viewport_scroll).
    pub scroll_into_view: Option<NodeId>,
    /// The live document the `document`/`Node` surface mutates. Native DOM
    /// callbacks reach it through `CallCx::host_data` (a `RefCell<HostState>`).
    pub dom: ScriptedDom,
    /// Nodes pinned by a live reflector (G1/G3). The DOM surface pins a node
    /// when it hands script a reflector (pin-on-mint); [`Runtime::collect_garbage`]
    /// retires the ids the engine reports dead and passes the survivors to
    /// [`ScriptedDom::collect`] as extra roots, so an orphan script can no longer
    /// reach is reaped.
    pub pins: genet_scripted_dom::Pins,
    /// Per-subtest results collected from `testharness.js` via the completion
    /// callback (the results bridge). Populated by [`Runtime::run_testharness`].
    pub results: Vec<TestResult>,
    /// The host's network seam for `fetch()`. `None` = no network (every fetch is
    /// a network error). Installed by [`Runtime::set_fetch_handler`]; an `Rc` so the
    /// native `__fetch_start` sink clones it out from under the `HostState` borrow
    /// before calling it (the handler must not run with a live borrow). No `Send`
    /// bound, so this crate links no network stack and stays `!Send`.
    pub fetch: Option<std::rc::Rc<dyn FetchHandler>>,
    /// The host's computed-style seam for `getComputedStyle` (e.g. pelt's
    /// `ScriptedDocument` over `IncrementalLayout`). `None` = no layout bound, so
    /// `getComputedStyle(...).<prop>` yields "". Installed by
    /// [`Runtime::set_computed_style_handler`]; an `Rc` so the native sink clones
    /// it out before calling (no live `HostState` borrow during the call).
    pub computed_style: Option<std::rc::Rc<dyn ComputedStyleHandler>>,
    /// The host's media-query seam for `window.matchMedia` (e.g. a
    /// `ScriptedDocument` over `IncrementalLayout`). `None` = no layout bound, so
    /// `matchMedia(q).matches` is `false` and `.media` is the raw query. Installed
    /// by [`Runtime::set_media_query_handler`]; an `Rc` so the native sink clones
    /// it out before calling (no live `HostState` borrow during the call).
    pub media_query: Option<std::rc::Rc<dyn MediaQueryHandler>>,
    /// The host's cookie store for `document.cookie` (e.g. meerkat's view over the
    /// netfetcher session jar). `None` = no store, so `document.cookie` reads `""` and
    /// a write is a no-op. Installed by [`Runtime::set_cookie_provider`]; an `Rc` so
    /// the native sink clones it out before calling (no live `HostState` borrow during
    /// the call).
    pub cookies: Option<std::rc::Rc<dyn CookieProvider>>,
    /// The document base URL, against which relative `fetch()` / `Request` URLs
    /// resolve (the `__resolve_url` sink reads it). `None` = no base (relative URLs
    /// stay relative, so a network fetch of one is an error). Set by
    /// [`Runtime::set_base_url`] for server-mode WPT runs.
    pub base_url: Option<String>,
    /// `window.localStorage` backing: an ordered key→value store (insertion
    /// order, for `key(n)` / `Object.keys`). The in-memory default (tests / WPT /
    /// no-host runs); a host that sets [`local_storage`](Self::local_storage) backs
    /// localStorage durably instead. Read/written by the `platform` surface's
    /// `__storage*` sinks.
    pub storage: Vec<(String, String)>,
    /// The host's durable backing for `localStorage`. `None` = use the in-memory
    /// [`storage`](Self::storage); `Some` routes the `__storage*` sinks through the
    /// host store (e.g. eidetic, persona + origin-partitioned). Installed by
    /// [`Runtime::set_local_storage_provider`]; an `Rc` so the native sink clones it
    /// out before calling (no live `HostState` borrow during the call).
    pub local_storage: Option<std::rc::Rc<dyn StorageProvider>>,
    /// `window.history` entries: `(serialized state JSON, document URL)`, the
    /// session history the `platform` surface's `__history*` sinks drive
    /// (`pushState` / `replaceState` / `state` / `length` / `go`). The current
    /// entry is [`history_index`](Self::history_index). Seeded with one entry on
    /// first use. `popstate` / real navigation are not wired (the scripted tier
    /// has none); the URL + state bookkeeping is correct.
    pub history: Vec<(String, String)>,
    /// Index of the current entry in [`history`](Self::history).
    pub history_index: usize,
    /// The host's WebGL context factory. `None` = no WebGL support (every
    /// `__webgl_*` sink no-ops or yields 0 / NO_ERROR, and contexts mint to
    /// index `-1`). Installed by [`Runtime::set_webgl_factory`]; invoked once
    /// per `getContext('webgl')` with the canvas drawing-buffer size, the
    /// result pushed into [`Self::webgl_contexts`].
    pub webgl_factory: Option<WebGlFactory>,
    /// Live WebGL contexts, indexed by the registry id the JS context object
    /// carries (`_ctx`). Each `__webgl_*` sink routes to one of these by its
    /// leading context-id argument. Grows by one per `getContext('webgl')`.
    pub webgl_contexts: Vec<Box<dyn WebGlHandler>>,
    /// Protocol trace marks emitted through native sinks while JS is still
    /// running. [`Runtime`] drains these after each engine boundary so they land
    /// in the deterministic NDJSON stream with stable sequence numbers.
    pending_trace: Vec<PendingTraceEvent>,
}

/// Shared handle to the runtime's [`HostState`]. The host reads it after running
/// script; native callbacks reach it through [`CallCx::host_data`].
pub type SharedHost = Rc<RefCell<HostState>>;

/// A scripting runtime: an engine plus the browser host surface bootstrapped onto
/// it. Generic over the backend ([`ScriptEngine`]); the WPT runner's per-backend
/// A/B uses the dispatch enum from the engine plan, not this type.
pub struct Runtime<E: ScriptEngine> {
    engine: E,
    host: SharedHost,
    scheduler_trace: Vec<SchedulerTraceEvent>,
    next_trace_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingTraceEvent {
    boundary: String,
    phase: String,
    detail: Option<String>,
}

/// One deterministic scheduler trace event, exportable as NDJSON for E4 trace
/// validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerTraceEvent {
    pub seq: u64,
    pub boundary: String,
    pub phase: String,
    pub detail: Option<String>,
}

impl SchedulerTraceEvent {
    fn push_ndjson_line(&self, out: &mut String) {
        out.push_str("{\"seq\":");
        out.push_str(&self.seq.to_string());
        out.push_str(",\"boundary\":");
        out.push_str(&js_str(&self.boundary));
        out.push_str(",\"phase\":");
        out.push_str(&js_str(&self.phase));
        if let Some(detail) = &self.detail {
            out.push_str(",\"detail\":");
            out.push_str(&js_str(detail));
        }
        out.push('}');
    }
}

impl<E: ScriptEngine> Runtime<E> {
    /// Construct an engine and install the host surface on it.
    pub fn new() -> Result<Self, E::Error> {
        let mut engine = E::new()?;
        let host: SharedHost = Rc::new(RefCell::new(HostState::default()));
        // The default viewport: the same 800x600 the media-query evaluator
        // already assumes, so a host that never calls `set_viewport_size` still
        // has `innerWidth`/`innerHeight` agree with `matchMedia`.
        host.borrow_mut().viewport_size = (800.0, 600.0);
        engine.set_host_data(host.clone());
        install_host_surface(&mut engine)?;
        Ok(Self {
            engine,
            host,
            scheduler_trace: Vec::new(),
            next_trace_seq: 0,
        })
    }

    /// Evaluate `source` in the runtime's global scope.
    pub fn eval(&mut self, source: &str) -> Result<E::Value, E::Error> {
        // Host boundary: evaluating a top-level script is driven by the embedder;
        // callers choose when to perform the next microtask checkpoint.
        self.trace_scheduler("eval", "start", None);
        let result = self.engine.eval(source);
        self.flush_host_trace_events();
        self.trace_scheduler("eval", if result.is_ok() { "end" } else { "error" }, None);
        result
    }

    /// Render an engine error to a diagnostic string (the thrown value's `toString`).
    /// Used by the test262 runner to match a `negative:` test's expected error type.
    pub fn describe_error(&mut self, error: &E::Error) -> String {
        self.engine.describe_error(error)
    }

    /// Stringify a value through the engine (`String(value)` semantics). Used by
    /// callers that read a result back out of script — e.g. the test262 async harness,
    /// which reports completion through a captured `print` buffer.
    pub fn value_to_string(&mut self, value: &E::Value) -> Result<String, E::Error> {
        self.engine.value_to_string(value)
    }

    /// Evaluate `source` as an ECMAScript module (`<script type=module>`), resolving
    /// `import`s against `base_url` through the host `resolve` callback (which fetches
    /// dependency source). Returns `Ok(None)` when the backend does not support
    /// modules, so the host can log and skip rather than fail. See
    /// [`ScriptEngine::eval_module`].
    pub fn eval_module(
        &mut self,
        source: &str,
        base_url: &str,
        resolve: &mut dyn FnMut(&str, &str) -> Option<(String, String)>,
    ) -> Result<Option<E::Value>, E::Error> {
        self.engine.eval_module(source, base_url, resolve)
    }

    /// Populate the live document from a parsed source document (any
    /// [`LayoutDom`] — e.g. a `StaticDocument` of a test's HTML), so script can
    /// query it (`document.body`, `getElementById`, `querySelector`). Clones the
    /// element/text tree under the scripted document root. Call before running
    /// script.
    pub fn load_dom<D: layout_dom_api::LayoutDom>(&mut self, src: &D) {
        let mut host = self.host.borrow_mut();
        let root = host.dom.document();
        dom::clone_into(src, src.document(), &mut host.dom, root);
    }

    /// Drain pending microtasks (Promise reaction jobs) to quiescence — a microtask
    /// checkpoint. Run it after evaluating script so Promise continuations resolve.
    pub fn run_microtasks(&mut self) {
        self.perform_microtask_checkpoint();
    }

    /// https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint
    fn perform_microtask_checkpoint(&mut self) {
        // Step 3: "While the event loop's microtask queue is not empty:"
        // Backend boundary: the VM owns the queue, budget behavior, and kept-object cleanup.
        self.trace_scheduler("pump_microtasks", "start", None);
        self.engine.pump_microtasks();
        self.flush_host_trace_events();
        self.trace_scheduler("pump_microtasks", "end", None);
    }

    /// The **input → event bridge**: dispatch a synthetic DOM event of `event_type`
    /// (`"click"`, `"keydown"`, …) at the node with raw id `raw_node_id`, running its
    /// registered listeners with full capture→target→bubble propagation. The host
    /// supplies the target (e.g. from `genet-layout`'s `hit_test`); the runtime owns
    /// no layout, so it cannot hit-test itself.
    ///
    /// Returns `true` if the default action should proceed, `false` if a listener
    /// called `preventDefault` (so the host can suppress link-follow, form submit,
    /// etc.). A microtask checkpoint runs after dispatch so listener-scheduled
    /// continuations settle. Bubbling and cancelable by default; an unknown id is a
    /// no-op that returns `false`.
    pub fn dispatch_event(
        &mut self,
        raw_node_id: usize,
        event_type: &str,
    ) -> Result<bool, E::Error> {
        self.trace_scheduler(
            "dispatch_event",
            "start",
            Some(format!("type={event_type};node={raw_node_id}")),
        );
        // Pass the node id as a *string* literal, not a bare number: a tagged
        // raw NodeId can exceed 2^53 and lose precision as a JS f64, corrupting
        // its doc-tag high bits and tripping the scripted-DOM fence. The
        // `__dispatchSynthetic` bridge does `String(rawId)` anyway.
        let v = match self.engine.eval(&format!(
            "__dispatchSynthetic(\"{raw_node_id}\", {event_type:?})"
        )) {
            Ok(value) => value,
            Err(error) => {
                self.flush_host_trace_events();
                self.trace_scheduler("dispatch_event", "error", None);
                return Err(error);
            },
        };
        self.flush_host_trace_events();
        let proceed = self
            .engine
            .value_to_string(&v)
            .map(|s| s != "false")
            .unwrap_or(true);
        self.perform_microtask_checkpoint();
        self.trace_scheduler("dispatch_event", "end", Some(format!("proceed={proceed}")));
        Ok(proceed)
    }

    /// Dispatch a CSS transition lifecycle event (`transitionrun` /
    /// `transitionstart` / `transitionend` / `transitioncancel`) at `raw_node_id`,
    /// carrying `property_name` and `elapsed_time` (seconds). The host drives this
    /// from the layout tick's harvested events, off the cascade. Bubbles, not
    /// cancelable; a microtask checkpoint runs after so listener-scheduled
    /// continuations settle. An unknown id is a no-op.
    pub fn dispatch_transition_event(
        &mut self,
        raw_node_id: usize,
        event_type: &str,
        property_name: &str,
        elapsed_time: f64,
    ) -> Result<(), E::Error> {
        // Pass the node id as a *string* literal, not a bare number: a tagged
        // raw NodeId can exceed 2^53 and lose precision as a JS f64 (the
        // `__dispatchTransition` bridge does `String(rawId)` anyway). `{:?}`
        // renders each &str as a quoted, escaped literal (valid JS), so a
        // property name / event type can't break out of the call expression.
        let expr = format!(
            "__dispatchTransition(\"{raw_node_id}\", {event_type:?}, {property_name:?}, {elapsed_time})"
        );
        self.engine.eval(&expr)?;
        self.flush_host_trace_events();
        self.perform_microtask_checkpoint();
        Ok(())
    }

    /// Set the viewport's CSS-px size, backing `window.innerWidth` /
    /// `innerHeight`. The host owns the viewport, so it calls this on resize (and
    /// once at startup); the runtime never computes it.
    pub fn set_viewport_size(&mut self, width: f32, height: f32) {
        self.host.borrow_mut().viewport_size = (width, height);
    }

    /// Dispatch a UA-generated **touch** event (`touchstart` / `touchmove` /
    /// `touchend` / `touchcancel`) at `raw_node_id`, carrying one touch point at
    /// page coordinates `(x, y)` with `identifier`.
    ///
    /// `cancelable` is **not** a parameter: per the passive-listener optimization
    /// (and WPT's `dom/events/non-cancelable-when-passive`), a touch event is
    /// cancelable only if some **non-passive** listener for its type exists on the
    /// propagation path. Only the DOM knows that, so `__dispatchTouch` computes it.
    /// Script-constructed events are unaffected — they keep the `cancelable` their
    /// constructor was given.
    ///
    /// Returns `false` if a listener called `preventDefault` (so the host can
    /// suppress the default gesture). An unknown id is a no-op.
    pub fn dispatch_touch_event(
        &mut self,
        raw_node_id: usize,
        event_type: &str,
        x: f64,
        y: f64,
        identifier: i32,
    ) -> Result<bool, E::Error> {
        // Node id as a *string*: a tagged raw NodeId can exceed 2^53 (the
        // precision bug the transitions plan hit).
        let expr = format!(
            "__dispatchTouch(\"{raw_node_id}\", {event_type:?}, {x}, {y}, {identifier})"
        );
        let v = self.engine.eval(&expr)?;
        self.flush_host_trace_events();
        let proceed = self
            .engine
            .value_to_string(&v)
            .map(|s| s != "false")
            .unwrap_or(true);
        self.perform_microtask_checkpoint();
        Ok(proceed)
    }

    /// Dispatch a UA-generated **wheel** event at `raw_node_id`: both the standard
    /// `wheel` and the legacy `mousewheel` (WPT tests both), at page coordinates
    /// `(x, y)` with the given deltas. `delta_mode` is 0 pixel / 1 line / 2 page.
    ///
    /// `cancelable` is computed per type by the same non-passive-listener rule as
    /// [`dispatch_touch_event`]. Returns `false` if either event was canceled.
    pub fn dispatch_wheel_event(
        &mut self,
        raw_node_id: usize,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        delta_mode: u32,
    ) -> Result<bool, E::Error> {
        let expr = format!(
            "__dispatchWheel(\"{raw_node_id}\", {x}, {y}, {delta_x}, {delta_y}, {delta_mode})"
        );
        let v = self.engine.eval(&expr)?;
        self.flush_host_trace_events();
        let proceed = self
            .engine
            .value_to_string(&v)
            .map(|s| s != "false")
            .unwrap_or(true);
        self.perform_microtask_checkpoint();
        Ok(proceed)
    }

    /// Dispatch a CSS animation lifecycle event (`animationstart` /
    /// `animationiteration` / `animationend` / `animationcancel`) at
    /// `raw_node_id`, carrying `animation_name` (the `@keyframes` rule's name) and
    /// `elapsed_time` (seconds). The `@keyframes` twin of
    /// [`dispatch_transition_event`](Self::dispatch_transition_event): the host
    /// drives it from the layout tick's harvested events, off the cascade. Bubbles,
    /// not cancelable; a microtask checkpoint runs after so listener-scheduled
    /// continuations settle. An unknown id is a no-op.
    pub fn dispatch_animation_event(
        &mut self,
        raw_node_id: usize,
        event_type: &str,
        animation_name: &str,
        elapsed_time: f64,
    ) -> Result<(), E::Error> {
        // Node id as a *string* literal: a tagged raw NodeId can exceed 2^53 and
        // lose precision as a JS f64. `{:?}` renders each &str as a quoted, escaped
        // literal, so a name or event type cannot break out of the call expression.
        let expr = format!(
            "__dispatchAnimation(\"{raw_node_id}\", {event_type:?}, {animation_name:?}, {elapsed_time})"
        );
        self.engine.eval(&expr)?;
        self.flush_host_trace_events();
        self.perform_microtask_checkpoint();
        Ok(())
    }

    /// The scripted-tier GC tick (G3): retire the reflectors the engine reports
    /// dead (unpinning their nodes), then mark-sweep the live document with the
    /// surviving pins as extra roots. An orphan script can no longer reach is
    /// reaped; a pinned one (and its whole component) is spared. Returns
    /// `(reflectors_unpinned, nodes_collected)`.
    ///
    /// **Not** auto-fired by [`run_microtasks`](Self::run_microtasks) yet — the
    /// embedder calls it at a GC cadence (a frame/idle tick). Auto-firing at the
    /// microtask checkpoint is a one-line flip, safe because pin-on-mint is
    /// complete: every node handoff in the `document`/`Node` surface goes through
    /// `dom::reflect_pinned`, and there is no `make_reflector` (unpinned) path.
    pub fn collect_garbage(&mut self) -> (usize, usize) {
        // Force the engine GC first, so reflector wrappers script has dropped are
        // observed dead this tick (the epoch-pin default no-ops, losing nothing).
        self.engine.force_gc();
        let dead = self.engine.drain_dead_reflectors();
        let mut host = self.host.borrow_mut();
        let unpinned = host
            .pins
            .retire_dead(dead.into_iter().map(|d| NodeId::from_raw(d as usize)));
        let HostState { dom, pins, .. } = &mut *host;
        let collected = dom.collect(pins.iter());
        (unpinned, collected)
    }

    /// Drive the event loop: fire pending timers in `(delay, insertion-order)`
    /// order, up to `budget` firings (the cap bounds `setInterval`, which
    /// re-enqueues itself). Cooperative: delays order tasks, they do not wait. A
    /// microtask checkpoint runs before the loop and after each fired timer task.
    /// Returns when the queue drains or the budget is spent.
    pub fn run_event_loop(&mut self, budget: u32) -> Result<(), E::Error> {
        self.trace_scheduler("run_event_loop", "start", Some(format!("budget={budget}")));
        self.perform_microtask_checkpoint();
        let mut fired_total = 0;
        for _ in 0..budget {
            // Step 2.6: "Perform oldestTask's steps."
            // Genet currently has one runnable task source here: timers.
            let fired = match self.run_one_timer_task(None) {
                Ok(fired) => fired,
                Err(error) => {
                    self.trace_scheduler("run_event_loop", "error", None);
                    return Err(error);
                },
            };
            if fired == 0 {
                break;
            }
            fired_total += fired;
            // Step 2.8: "Perform a microtask checkpoint."
            self.perform_microtask_checkpoint();
        }
        self.trace_scheduler(
            "run_event_loop",
            "end",
            Some(format!("fired={fired_total}")),
        );
        Ok(())
    }

    /// Load `harness_src` (`testharness.js`), run `test_src` against it, and return
    /// the per-subtest results. Completion is triggered by dispatching the window
    /// `load` event (when the `WindowTestEnvironment` reports) and draining the
    /// event loop + microtasks. Results also remain in [`HostState::results`].
    pub fn run_testharness(
        &mut self,
        harness_src: &str,
        test_src: &str,
    ) -> Result<Vec<TestResult>, E::Error> {
        self.load_testharness(harness_src)?;
        self.run_loaded_testharness(test_src)
    }

    /// Load `testharness.js` and install the result bridge, but do not run a
    /// test yet. A snapshot-capable engine can clone after this point so each
    /// test gets a fresh harness heap without re-evaluating the harness source.
    pub fn load_testharness(&mut self, harness_src: &str) -> Result<(), E::Error> {
        self.engine.eval(harness_src)?;
        self.flush_host_trace_events();
        harness::install_bridge(&mut self.engine)
    }

    /// Run `test_src` against an already-loaded `testharness.js`, dispatch
    /// `load`, drain the event loop, and return the reported subtests.
    pub fn run_loaded_testharness(&mut self, test_src: &str) -> Result<Vec<TestResult>, E::Error> {
        self.host.borrow_mut().results.clear();
        self.engine.eval(test_src)?;
        self.flush_host_trace_events();
        self.engine
            .eval("window.dispatchEvent(new Event('load'));")?;
        self.flush_host_trace_events();
        self.run_event_loop(1000)?;
        Ok(self.host.borrow().results.clone())
    }

    /// Set up a testharness run (load the harness + bridge, run the test, dispatch
    /// `load`) WITHOUT draining to completion. The caller then drives the event loop
    /// and a deferred-fetch completion source itself ([`run_timers`](Self::run_timers),
    /// [`settle_fetch`](Self::settle_fetch), [`pending_fetches`](Self::pending_fetches)),
    /// which the synchronous [`run_testharness`](Self::run_testharness) cannot do
    /// because deferred replies arrive out of band. Read results with
    /// [`results`](Self::results).
    pub fn begin_testharness(&mut self, harness_src: &str, test_src: &str) -> Result<(), E::Error> {
        self.load_testharness(harness_src)?;
        self.begin_loaded_testharness(test_src)
    }

    /// Begin a testharness run against an already-loaded harness without
    /// draining to completion. The caller drives timers/fetch completions and
    /// reads [`results`](Self::results).
    pub fn begin_loaded_testharness(&mut self, test_src: &str) -> Result<(), E::Error> {
        self.host.borrow_mut().results.clear();
        self.engine.eval(test_src)?;
        self.flush_host_trace_events();
        self.engine
            .eval("window.dispatchEvent(new Event('load'));")?;
        self.flush_host_trace_events();
        Ok(())
    }

    /// Fire up to `budget` due timers (with a microtask checkpoint after each task)
    /// against the virtual clock at `now_ms` (the real elapsed time of the run), and
    /// return how many fired. Real-time gating lets a short abort timer fire at its
    /// delay while the far-future testharness timeout stays pending.
    pub fn run_timers(&mut self, budget: u32, now_ms: f64) -> usize {
        self.trace_scheduler(
            "run_timers",
            "start",
            Some(format!("budget={budget};now_ms={now_ms}")),
        );
        self.perform_microtask_checkpoint();
        let mut fired = 0;
        for _ in 0..budget {
            // Step 2.6: "Perform oldestTask's steps."
            // The host drive loop supplies `now_ms`; the JS timer queue chooses one due task.
            let n = self.run_one_timer_task(Some(now_ms)).unwrap_or(0);
            if n == 0 {
                break;
            }
            fired += n;
            // Step 2.8: "Perform a microtask checkpoint."
            self.perform_microtask_checkpoint();
        }
        self.trace_scheduler("run_timers", "end", Some(format!("fired={fired}")));
        fired
    }

    /// Run the window's animation frame callbacks against the frame timestamp
    /// `now_ms` — the "run the animation frame callbacks" step of the rendering
    /// update; the host tick owns the clock and calls this once per frame.
    /// One callback per engine call, with a microtask checkpoint after each, so
    /// a Promise reaction queued by callback N settles before callback N+1
    /// (the same granularity [`run_timers`](Self::run_timers) has per task).
    /// Callbacks registered during the run land in the next frame; canceled
    /// handles are skipped. Returns how many callbacks ran.
    ///
    /// https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames
    pub fn run_animation_frame_callbacks(&mut self, now_ms: f64) -> Result<usize, E::Error> {
        self.trace_scheduler(
            "run_animation_frame_callbacks",
            "start",
            Some(format!("now_ms={now_ms}")),
        );
        let mut ran = 0usize;
        loop {
            // Step 3: "For each handle in callbackHandles, if handle exists in
            // callbacks: ... Invoke callback with « now » and "report"."
            let v = match self
                .engine
                .eval(&format!("String(globalThis.__runOneAnimationFrameCallback({now_ms}))"))
            {
                Ok(value) => value,
                Err(error) => {
                    self.flush_host_trace_events();
                    self.trace_scheduler("run_animation_frame_callbacks", "error", None);
                    return Err(error);
                },
            };
            self.flush_host_trace_events();
            let n = self
                .engine
                .value_to_string(&v)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            ran += n;
            // The JS stack is empty between callbacks: perform a microtask
            // checkpoint, matching the between-callbacks checkpoint browsers
            // exhibit ("clean up after running script" on an empty stack).
            self.perform_microtask_checkpoint();
        }
        self.trace_scheduler(
            "run_animation_frame_callbacks",
            "end",
            Some(format!("ran={ran}")),
        );
        Ok(ran)
    }

    /// Whether any animation frame callbacks are registered. The host uses this
    /// (alongside its own animation state) to keep requesting frames only while
    /// script is animating, so idle surfaces stop ticking.
    pub fn has_animation_frame_callbacks(&mut self) -> bool {
        self.engine
            .eval("String(globalThis.__hasAnimationFrameCallbacks())")
            .ok()
            .and_then(|v| self.engine.value_to_string(&v).ok())
            .map(|s| s == "true")
            .unwrap_or(false)
    }

    fn run_one_timer_task(&mut self, now_ms: Option<f64>) -> Result<usize, E::Error> {
        let expr = match now_ms {
            Some(now_ms) => format!("String(globalThis.__runTimers(1,{now_ms}))"),
            None => "String(globalThis.__runTimers(1))".to_string(),
        };
        let v = match self.engine.eval(&expr) {
            Ok(value) => value,
            Err(error) => {
                self.flush_host_trace_events();
                self.trace_scheduler("timer_task", "error", None);
                return Err(error);
            },
        };
        self.flush_host_trace_events();
        let s = self.engine.value_to_string(&v)?;
        let fired = s.parse().unwrap_or(0);
        if fired > 0 {
            self.trace_scheduler("timer_task", "performed", Some(format!("fired={fired}")));
        }
        Ok(fired)
    }

    /// Milliseconds until the next timer is due (relative to the last `run_timers`
    /// virtual clock), or `None` if no timer is scheduled. The drive loop sleeps at
    /// most this long so a timer fires near its real deadline.
    pub fn next_timer_delay(&mut self) -> Option<f64> {
        let d: f64 = self
            .engine
            .eval("String(globalThis.__nextTimerDelay())")
            .ok()
            .and_then(|v| self.engine.value_to_string(&v).ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(-1.0);
        if d < 0.0 { None } else { Some(d) }
    }

    /// The per-subtest results collected so far (the driver reads this after the run
    /// quiesces or its deadline elapses).
    pub fn results(&self) -> Vec<TestResult> {
        self.host.borrow().results.clone()
    }

    /// Install the host's `fetch()` network seam (e.g. a netfetcher-backed
    /// handler). Until set, `fetch()` yields a network error. A synchronous handler
    /// (implementing only `fetch`) settles each request in-tick; a deferred handler
    /// (overriding `start`) settles later via [`settle_fetch`](Self::settle_fetch).
    pub fn set_fetch_handler(&mut self, handler: Box<dyn FetchHandler>) {
        self.host.borrow_mut().fetch = Some(std::rc::Rc::from(handler));
    }

    /// Install the host's computed-style seam for `getComputedStyle` (e.g. a
    /// handler over the host's `IncrementalLayout`). Until set, computed-style
    /// reads yield "". The runtime itself links no layout engine — this is the
    /// boundary, mirroring [`set_fetch_handler`](Self::set_fetch_handler).
    pub fn set_computed_style_handler(&mut self, handler: Box<dyn ComputedStyleHandler>) {
        self.host.borrow_mut().computed_style = Some(std::rc::Rc::from(handler));
    }

    /// Install the host's media-query seam for `window.matchMedia` (e.g. a
    /// handler evaluating a query string against the host's `IncrementalLayout`
    /// device). Until set, `matchMedia(q).matches` is `false`. The runtime links
    /// no layout engine — this is the boundary, mirroring
    /// [`set_computed_style_handler`](Self::set_computed_style_handler).
    pub fn set_media_query_handler(&mut self, handler: Box<dyn MediaQueryHandler>) {
        self.host.borrow_mut().media_query = Some(std::rc::Rc::from(handler));
    }

    /// Notify script that the media environment changed (viewport resize,
    /// preference flip, …). Re-evaluates every live `MediaQueryList` and fires a
    /// `change` event on those whose match state flipped. The host calls this
    /// after mutating the device its [`MediaQueryHandler`] evaluates against. A
    /// microtask checkpoint runs after so listener-scheduled work settles.
    pub fn notify_media_features_changed(&mut self) -> Result<(), E::Error> {
        self.engine.eval(
            "globalThis.__reevaluateMediaQueries && globalThis.__reevaluateMediaQueries()",
        )?;
        self.flush_host_trace_events();
        self.perform_microtask_checkpoint();
        Ok(())
    }

    /// Install the host's cookie store for `document.cookie` (e.g. meerkat's view over
    /// the netfetcher session jar). Until set, `document.cookie` reads "" and writes
    /// no-op. The runtime owns no networking — this is the boundary, mirroring
    /// [`set_fetch_handler`](Self::set_fetch_handler).
    pub fn set_cookie_provider(&mut self, provider: Box<dyn CookieProvider>) {
        self.host.borrow_mut().cookies = Some(std::rc::Rc::from(provider));
    }

    /// Install the host's durable backing for `localStorage` (e.g. an eidetic store,
    /// persona + origin-partitioned). Until set, localStorage is the in-memory
    /// [`HostState::storage`] default. The runtime owns no persistence — this is the
    /// boundary, mirroring [`set_cookie_provider`](Self::set_cookie_provider).
    pub fn set_local_storage_provider(&mut self, provider: Box<dyn StorageProvider>) {
        self.host.borrow_mut().local_storage = Some(std::rc::Rc::from(provider));
    }

    /// Install the host's WebGL context factory (e.g. one that mints a
    /// webgl-wgpu context over a real `wgpu::Device` at the requested size).
    /// Until set, the JS `WebGLRenderingContext` methods no-op or return
    /// `gl.NO_ERROR`. The factory is called once per `getContext('webgl')`,
    /// so each `<canvas>` gets its own independent context.
    pub fn set_webgl_factory(&mut self, factory: WebGlFactory) {
        self.host.borrow_mut().webgl_factory = Some(factory);
    }

    /// Resolve the pending `fetch()` Promise `id` with `outcome` (a deferred host
    /// calls this when the reply arrives). Rust cannot call a held JS function, so
    /// this evals the `__fetchSettle` entry point (the same shape as `__runTimers`)
    /// and pumps microtasks. Holds no `HostState` borrow across the eval.
    pub fn settle_fetch(&mut self, id: u64, outcome: FetchOutcome) {
        let json = fetch::encode_outcome(&outcome);
        let js = format!("globalThis.__fetchSettle({},{});", id, js_str(&json));
        // Fetch task source: perform the host completion task, then checkpoint.
        let _ = self.engine.eval(&js);
        self.perform_microtask_checkpoint();
    }

    /// Reject the pending `fetch()` Promise `id` as a network error with `message`
    /// (a `TypeError`, per Fetch). For a deferred host's failed request.
    pub fn fail_fetch(&mut self, id: u64, message: &str) {
        let js = format!("globalThis.__fetchFail({},{});", id, js_str(message));
        // Fetch task source: perform the host completion task, then checkpoint.
        let _ = self.engine.eval(&js);
        self.perform_microtask_checkpoint();
    }

    /// Early-settle the pending `fetch()` Promise `id` with a streaming response:
    /// status + headers from `meta` (its body is ignored), body delivered
    /// incrementally via [`push_chunk`](Self::push_chunk) then
    /// [`close_stream`](Self::close_stream). For a host that streams a response body
    /// as it arrives rather than buffering the whole thing.
    pub fn start_stream(&mut self, id: u64, meta: FetchOutcome) {
        let json = fetch::encode_outcome(&meta);
        let js = format!("globalThis.__fetchStartStream({},{});", id, js_str(&json));
        // Fetch task source: resolve the response task before body chunks arrive.
        let _ = self.engine.eval(&js);
        self.perform_microtask_checkpoint();
    }

    /// Push a body chunk to a streaming response started with
    /// [`start_stream`](Self::start_stream). Bytes cross as a JS array literal (no
    /// string-escape hazard), feeding the response's `ReadableStream` controller.
    pub fn push_chunk(&mut self, id: u64, bytes: &[u8]) {
        let mut lit = String::with_capacity(bytes.len() * 4 + 2);
        lit.push('[');
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 {
                lit.push(',');
            }
            lit.push_str(&b.to_string());
        }
        lit.push(']');
        // Fetch task source: deliver one body-chunk task, then checkpoint.
        let _ = self
            .engine
            .eval(&format!("globalThis.__fetchPushChunk({},{});", id, lit));
        self.perform_microtask_checkpoint();
    }

    /// Close a streaming response started with [`start_stream`](Self::start_stream):
    /// the body's `ReadableStream` ends and pending reads resolve `done`.
    pub fn close_stream(&mut self, id: u64) {
        // Fetch task source: deliver the end-of-body task, then checkpoint.
        let _ = self
            .engine
            .eval(&format!("globalThis.__fetchClose({});", id));
        self.perform_microtask_checkpoint();
    }

    /// Error a streaming response started with [`start_stream`](Self::start_stream):
    /// the body's `ReadableStream` errors so pending/future reads reject with a
    /// `TypeError`. The response itself stays resolved (the failure is mid-body,
    /// e.g. a `Content-Encoding` decode error), so only body consumption rejects.
    pub fn error_stream(&mut self, id: u64) {
        // Fetch task source: deliver the body-error task, then checkpoint.
        let _ = self
            .engine
            .eval(&format!("globalThis.__fetchError({});", id));
        self.perform_microtask_checkpoint();
    }

    /// Reject every still-pending `fetch()` Promise with `message` (a `TypeError`).
    /// The host drive loop calls this at its wall-clock deadline so a test that
    /// awaits a never-settling fetch records a failure rather than hanging.
    pub fn fail_all_pending(&mut self, message: &str) {
        let js = format!(
            "(function(){{var p=globalThis.__pending;for(var k in p){{var e=p[k];delete p[k];if(e){{\
             if(e.controller){{try{{e.controller.error(new TypeError({m}));}}catch(x){{}}}}\
             if(!e.settled){{e.settled=true;e.reject(new TypeError({m}));}}}}}}}})();",
            m = js_str(message)
        );
        // Fetch task source: reject outstanding fetches at the host deadline, then checkpoint.
        let _ = self.engine.eval(&js);
        self.perform_microtask_checkpoint();
    }

    /// How many `fetch()` Promises are still pending. The host drive loop reads this
    /// to know when script has quiesced (no in-flight fetches left to settle).
    pub fn pending_fetches(&mut self) -> usize {
        // Count only fetches still doing work: a Promise not yet settled, or a
        // streaming body actively awaiting a demanded chunk. A settled response
        // whose body the script abandoned (never read) does not count, so the
        // event loop can quiesce instead of waiting on a chunk no one wants.
        self.engine
            .eval(
                "String((function(){var p=globalThis.__pending,n=0;\
                 for(var k in p){var e=p[k];if(e&&(!e.settled||e.awaiting))n++;}return n;})())",
            )
            .ok()
            .and_then(|v| self.engine.value_to_string(&v).ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// Set the document base URL: relative `fetch()` / `Request` URLs resolve
    /// against it (via the `__resolve_url` sink), and `window.location` is
    /// populated from its components so `get-host-info` / `make_absolute_url` read
    /// the real origin. For server-mode WPT runs; disk mode leaves the default
    /// `about:blank` location. A non-absolute `url` is ignored.
    pub fn set_base_url(&mut self, url: &str) -> Result<(), E::Error> {
        let Ok(u) = url::Url::parse(url) else {
            return Ok(());
        };
        self.host.borrow_mut().base_url = Some(u.to_string());
        // `globalThis.location` is a live view of `base_url` (the `platform`
        // surface's getters re-read it), so updating the host base URL is enough;
        // no need to rebuild a location snapshot here.
        Ok(())
    }

    /// The shared host state (e.g. to read `console` output after a run).
    pub fn host(&self) -> &SharedHost {
        &self.host
    }

    /// The underlying engine, for callers needing reflectors or raw globals.
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }

    /// Deterministic scheduler trace events captured since runtime creation or the
    /// last clear.
    pub fn scheduler_trace(&mut self) -> &[SchedulerTraceEvent] {
        self.flush_host_trace_events();
        &self.scheduler_trace
    }

    /// Export the scheduler trace as newline-delimited JSON for E4 trace-data
    /// generation.
    pub fn scheduler_trace_ndjson(&mut self) -> String {
        self.flush_host_trace_events();
        let mut out = String::new();
        for event in &self.scheduler_trace {
            event.push_ndjson_line(&mut out);
            out.push('\n');
        }
        out
    }

    /// Clear the scheduler trace without touching JS or host state.
    pub fn clear_scheduler_trace(&mut self) {
        self.scheduler_trace.clear();
        self.next_trace_seq = 0;
        self.host.borrow_mut().pending_trace.clear();
    }

    fn trace_scheduler(&mut self, boundary: &str, phase: &str, detail: Option<String>) {
        self.next_trace_seq += 1;
        self.scheduler_trace.push(SchedulerTraceEvent {
            seq: self.next_trace_seq,
            boundary: boundary.to_string(),
            phase: phase.to_string(),
            detail,
        });
    }

    fn flush_host_trace_events(&mut self) {
        let pending = {
            let mut host = self.host.borrow_mut();
            std::mem::take(&mut host.pending_trace)
        };
        for event in pending {
            self.trace_scheduler(&event.boundary, &event.phase, event.detail);
        }
    }
}

impl<E: ScriptEngineSnapshot> Runtime<E> {
    /// Clone this runtime's idle engine heap while replacing host-owned state.
    ///
    /// The JS global/bootstrap state is preserved by the engine snapshot. The Rust
    /// host state (`HostState`, DOM arena, pins, fetch/computed-style handlers, etc.)
    /// is fresh, because it is owned outside the VM heap and must not leak between
    /// tests/documents.
    pub fn snapshot_clone(&mut self) -> Result<Self, E::Error> {
        let mut engine = self.engine.snapshot_clone()?;
        let host: SharedHost = Rc::new(RefCell::new(HostState::default()));
        engine.set_host_data(host.clone());
        Ok(Self {
            engine,
            host,
            scheduler_trace: Vec::new(),
            next_trace_seq: 0,
        })
    }
}

/// Install the global host objects from VM primitives.
fn install_host_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    // `self` and `window` alias the global. `globalThis` is provided by the engine
    // (ES2020), so both backends bootstrap the aliases the same way.
    engine.eval("var self = globalThis; var window = globalThis;")?;

    // console.log / console.error: native sinks, exposed as methods on a `console`
    // object. (A future slice formats multiple arguments; this records the first.)
    engine.set_function::<ConsoleLog>("__console_log", 1)?;
    engine.set_function::<ConsoleError>("__console_error", 1)?;
    engine.eval("globalThis.console = { log: __console_log, error: __console_error };")?;

    // window.scrollTo / scrollBy / scrollX|Y over the host's document scroll
    // (V4 of the viewport standards). Numbers cross the native boundary as strings
    // (the CallCx surface has no number marshalling); the JS getters `Number()`
    // them back. The host clamps + applies the value script sets (see
    // `HostState::viewport_scroll`).
    engine.set_function::<ScrollTo>("__scrollTo", 2)?;
    engine.set_function::<ScrollBy>("__scrollBy", 2)?;
    engine.set_function::<ScrollX>("__scrollX", 0)?;
    engine.set_function::<ScrollY>("__scrollY", 0)?;
    // Element.scrollIntoView records a pending target the host resolves; the JS glue
    // lives in the DOM bootstrap (Element.prototype), which runs below.
    engine.set_function::<ScrollIntoView>("__scrollIntoView", 1)?;
    engine.eval(SCROLL_BOOTSTRAP)?;

    // window.innerWidth / innerHeight over the host's viewport size. Tests that
    // compute a hit point from the viewport (WPT's wheel/scroll cluster does:
    // `Math.floor(window.innerWidth / 2)`) get NaN without these.
    engine.set_function::<InnerWidth>("__innerWidth", 0)?;
    engine.set_function::<InnerHeight>("__innerHeight", 0)?;
    engine.eval(VIEWPORT_BOOTSTRAP)?;

    // Event loop and EventTarget are pure-JS bootstraps over the global. Callbacks
    // live JS-side; the only Rust entry is `run_event_loop` (evals `__runTimers`).
    // ES5-style (function constructors, no arrows/classes) for the widest backend
    // coverage.
    engine.eval(EVENT_LOOP_BOOTSTRAP)?;
    engine.eval(EVENT_TARGET_BOOTSTRAP)?;

    // postMessage (async 'message' delivery to the global) + minimal location /
    // navigator stubs the harness reads at load. Depends on the event loop +
    // EventTarget above.
    engine.set_function::<TraceProtocol>("__traceProtocol", 3)?;
    engine.eval(SHELL_GLOBALS_BOOTSTRAP)?;

    // `document` + the Node/Element construction surface, bound to the `ScriptedDom`
    // in host state. Native sinks mutate the arena; a JS bootstrap wraps reflectors
    // into ergonomic node objects.
    dom::install_dom_surface(engine)?;
    // The `fetch()` / `Response` / `Headers` surface over the host fetch seam.
    fetch::install_fetch_surface(engine)?;
    // `WebGLRenderingContext` (Triangle-class subset) over the host webgl seam.
    // Until step 2 wires `HTMLCanvasElement.getContext('webgl')`, JS reaches it
    // through the `__createWebGLContext()` helper.
    webgl::install_webgl_surface(engine)?;

    // Window platform services: `location` (reflecting the document URL), plus
    // `localStorage` / `history` as they land. After `dom` so `document` exists.
    platform::install_platform_surface(engine)?;

    // The `__reportResult` sink for the testharness results bridge. The completion
    // callback that calls it is registered later (after testharness loads).
    harness::install_report_sink(engine)?;
    Ok(())
}

/// `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` over a private
/// queue, drained by `__runTimers(budget)` in `(delay, insertion)` order.
const EVENT_LOOP_BOOTSTRAP: &str = r#"
(function() {
  // HTML timers task source: pending timer tasks wait here until the Rust host
  // asks `__runTimers` to perform one task boundary.
  var timers = [];
  var nextId = 1;
  var vnow = 0; // virtual clock (ms); advanced by __runTimers in real-time mode
  function schedule(cb, delay, repeat) {
    var d = +delay || 0; if (d < 0) d = 0;
    var id = nextId++;
    timers.push({ id: id, cb: cb, delay: d, at: vnow + d, seq: timers.length, repeat: !!repeat });
    return id;
  }
  globalThis.setTimeout = function(cb, delay) { return schedule(cb, delay, false); };
  globalThis.setInterval = function(cb, delay) { return schedule(cb, delay, true); };
  globalThis.clearTimeout = function(id) {
    for (var i = 0; i < timers.length; i++) {
      if (timers[i].id === id) { timers.splice(i, 1); return; }
    }
  };
  globalThis.clearInterval = globalThis.clearTimeout;
  // Legacy escape() / unescape() (ECMAScript Annex B): percent-escape bytes
  // (%XX) and code units > 255 (%uXXXX). Some WPT tests build byte sequences
  // with escape(); provide them if the engine doesn't.
  if (typeof globalThis.escape !== 'function') {
    var ESCAPE_OK = /[A-Za-z0-9@*_+\-.\/]/;
    globalThis.escape = function(s) {
      s = String(s); var out = '';
      for (var i = 0; i < s.length; i++) {
        var ch = s.charAt(i), c = s.charCodeAt(i);
        if (ESCAPE_OK.test(ch)) out += ch;
        else if (c < 256) out += '%' + ('0' + c.toString(16).toUpperCase()).slice(-2);
        else out += '%u' + ('000' + c.toString(16).toUpperCase()).slice(-4);
      }
      return out;
    };
  }
  if (typeof globalThis.unescape !== 'function') {
    globalThis.unescape = function(s) {
      s = String(s); var out = '';
      for (var i = 0; i < s.length; i++) {
        var ch = s.charAt(i);
        if (ch === '%' && s.charAt(i + 1) === 'u') { out += String.fromCharCode(parseInt(s.substr(i + 2, 4), 16)); i += 5; }
        else if (ch === '%') { out += String.fromCharCode(parseInt(s.substr(i + 1, 2), 16)); i += 2; }
        else out += ch;
      }
      return out;
    };
  }
  // btoa() / atob() (base64 of a binary string). Each char is one byte (0-255);
  // btoa throws on a code unit > 255. Provided if the engine lacks them.
  if (typeof globalThis.btoa !== 'function') {
    var B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
    globalThis.btoa = function(s) {
      s = String(s); var out = '';
      for (var i = 0; i < s.length; i += 3) {
        var c0 = s.charCodeAt(i);
        var c1 = i + 1 < s.length ? s.charCodeAt(i + 1) : 0;
        var c2 = i + 2 < s.length ? s.charCodeAt(i + 2) : 0;
        if (c0 > 255 || c1 > 255 || c2 > 255) throw new RangeError("btoa: code unit out of range");
        var n = (c0 << 16) | (c1 << 8) | c2;
        out += B64.charAt((n >> 18) & 63) + B64.charAt((n >> 12) & 63);
        out += (i + 1 < s.length) ? B64.charAt((n >> 6) & 63) : '=';
        out += (i + 2 < s.length) ? B64.charAt(n & 63) : '=';
      }
      return out;
    };
    globalThis.atob = function(s) {
      s = String(s).replace(/[ \t\n\f\r]/g, '');
      if (s.length % 4 === 1) throw new RangeError("atob: invalid length");
      s = s.replace(/=+$/, '');
      var out = '', buf = 0, bits = 0;
      for (var i = 0; i < s.length; i++) {
        var idx = B64.indexOf(s.charAt(i));
        if (idx < 0) throw new RangeError("atob: invalid character");
        buf = (buf << 6) | idx; bits += 6;
        if (bits >= 8) { bits -= 8; out += String.fromCharCode((buf >> bits) & 0xFF); }
      }
      return out;
    };
  }
  // Fire due timers. `nowMs` undefined => cooperative (disk path): fire everything
  // in (delay, seq) order, ignoring real time. `nowMs` given => real-time gate
  // (deferred drive loop): advance the virtual clock and fire only timers whose
  // `at` has elapsed, in (at, seq) order — so a short abort timer fires at its real
  // delay while the far-future testharness timeout stays pending.
  globalThis.__runTimers = function(budget, nowMs) {
    var realtime = (nowMs !== undefined);
    if (realtime && nowMs > vnow) vnow = nowMs;
    var fired = 0;
    while (fired < budget) {
      var pending = globalThis.__pending;
      if (pending && Object.keys(pending).length > 0) break;
      var idx = -1, bestKey = 0, bestSeq = 0;
      for (var i = 0; i < timers.length; i++) {
        var t = timers[i];
        if (realtime && t.at > vnow) continue; // not due yet
        var key = realtime ? t.at : t.delay;
        if (idx < 0 || key < bestKey || (key === bestKey && t.seq < bestSeq)) { idx = i; bestKey = key; bestSeq = t.seq; }
      }
      if (idx < 0) break; // nothing eligible
      var due = timers.splice(idx, 1)[0];
      fired++;
      if (due.repeat) { due.at = vnow + due.delay; due.seq = nextId++; timers.push(due); }
      due.cb();
      // If a timer callback issued a deferred fetch (left a pending entry), stop so
      // the host drive loop can settle it before later timers (e.g. the harness
      // timeout) fire. No-op when nothing is pending (the disk path).
      var p = globalThis.__pending;
      if (p && Object.keys(p).length > 0) break;
    }
    return fired;
  };
  // Milliseconds until the next timer is due (real-time), or -1 if none. The drive
  // loop sleeps at most this long so a timer fires near its real deadline.
  globalThis.__nextTimerDelay = function() {
    var best = -1;
    for (var i = 0; i < timers.length; i++) {
      var d = timers[i].at - vnow; if (d < 0) d = 0;
      if (best < 0 || d < best) best = d;
    }
    return best;
  };
})();
"#;

/// `EventTarget` (with the global as one) + a minimal `Event`. Listeners are kept
/// per target keyed by type; `dispatchEvent` calls them synchronously over a copy.
const EVENT_TARGET_BOOTSTRAP: &str = r#"
(function() {
  // Shared EventTarget. Listeners are stored as `{cb, once, passive}` records,
  // keyed by phase ('c:'/'b:' + type) — the same model Node uses in dom.rs, so
  // window and DOM nodes share one listener shape and one firing helper
  // (`__fire`). window has no DOM children, so its own `dispatchEvent` is a
  // target-only dispatch; Node (dom.rs) overrides dispatchEvent with the full
  // capture → target → bubble walk and reuses `__fire` per node (including
  // window at the top of the propagation path). See
  // docs/2026-06-01_event_model_convergence_plan.md.
  function eventOpts(arg) {
    if (arg && typeof arg === 'object') {
      return { capture: !!arg.capture, once: !!arg.once, passive: !!arg.passive };
    }
    return { capture: !!arg, once: false, passive: false };
  }
  function EventTarget() { this.__listeners = {}; }
  EventTarget.prototype.addEventListener = function(type, cb, opts) {
    if (typeof cb !== 'function') return;
    if (!this.__listeners) this.__listeners = {};
    var o = eventOpts(opts);
    var key = (o.capture ? 'c:' : 'b:') + type;
    var l = this.__listeners[key] || (this.__listeners[key] = []);
    for (var i = 0; i < l.length; i++) { if (l[i].cb === cb) return; }
    l.push({ cb: cb, once: o.once, passive: o.passive });
  };
  EventTarget.prototype.removeEventListener = function(type, cb, opts) {
    if (!this.__listeners) return;
    var o = eventOpts(opts);
    var l = this.__listeners[(o.capture ? 'c:' : 'b:') + type];
    if (!l) return;
    for (var i = 0; i < l.length; i++) {
      if (l[i].cb === cb) { l.splice(i, 1); return; }
    }
  };
  // Fire one node's listeners for `key` ('c:'/'b:' + type). Honors once (remove
  // before call), passive (preventDefault no-op via __inPassive), and
  // stopImmediatePropagation (halts the rest of this node's listeners). Shared
  // by window's dispatchEvent here and Node's propagation walk in dom.rs.
  EventTarget.prototype.__fire = function(event, key) {
    if (!this.__listeners) return;
    var l = this.__listeners[key];
    if (!l) return;
    event.currentTarget = this;
    var copy = l.slice();
    for (var i = 0; i < copy.length && !event.__stopImmediate; i++) {
      var rec = copy[i];
      if (rec.once) { var j = l.indexOf(rec); if (j !== -1) l.splice(j, 1); }
      event.__inPassive = rec.passive;
      // Listener exceptions are reported, not propagated (DOM §dispatch); see
      // the same guard in dom/bootstrap.js's node `fire`.
      try { rec.cb.call(this, event); }
      catch (ex) { globalThis.__reportListenerException(ex); }
      event.__inPassive = false;
    }
  };
  EventTarget.prototype.dispatchEvent = function(event) {
    if (event.__initialized === false || event.__dispatch) {
      throw new DOMException("The event is not initialized or is being dispatched.", "InvalidStateError");
    }
    // window is a leaf target (no DOM tree): target phase only — capture- then
    // bubble-registered listeners on this target, with the dispatch flags set.
    // The stop flags are cleared *after* dispatch, not before (DOM §dispatch):
    // an event already stopped (`cancelBubble` set pre-dispatch) fires nothing.
    event.__dispatch = true;
    event.target = this;
    event.srcElement = this;
    event.eventPhase = 2; // AT_TARGET
    if (!event.__stop) { this.__fire(event, 'c:' + event.type); }
    if (!event.__stop) { this.__fire(event, 'b:' + event.type); }
    event.__dispatch = false;
    event.currentTarget = null;
    event.eventPhase = 0;
    event.__stop = false;
    event.__stopImmediate = false;
    return !event.__canceled;
  };
  globalThis.EventTarget = EventTarget;

  function Event(type, init) {
    this.type = type;
    init = init || {};
    this.bubbles = !!init.bubbles;
    this.cancelable = !!init.cancelable;
    this.defaultPrevented = false;
    this.__canceled = false;
    this.eventPhase = 0; // NONE until dispatch sets the phase
    this.target = null;
    this.currentTarget = null;
    // Initialized flag (DOM §dom-event-initevent). A constructed Event is
    // initialized; an event from document.createEvent() is NOT until initEvent()
    // runs — dispatchEvent throws InvalidStateError before then.
    this.__initialized = true;
  }
  Event.prototype.preventDefault = function() {
    // A passive listener (addEventListener {passive:true}) cannot cancel the
    // default action — preventDefault is a no-op while one is firing (DOM;
    // __inPassive is set around the listener call in dom.rs's `fire`).
    if (this.cancelable && !this.__inPassive) {
      this.__canceled = true;
      this.defaultPrevented = true;
    }
  };
  // Legacy alias: `event.returnValue = false` is preventDefault(); reading it
  // reflects whether the default is still allowed (DOM, Window event handlers).
  Object.defineProperty(Event.prototype, 'returnValue', {
    configurable: true,
    get: function() { return !this.__canceled; },
    set: function(v) { if (!v) { this.preventDefault(); } },
  });
  globalThis.Event = Event;

  function UIEvent(type, init) {
    Event.call(this, type, init);
    init = init || {};
    this.view = init.view || null;
    this.detail = Number(init.detail || 0);
  }
  UIEvent.prototype = Object.create(Event.prototype);
  UIEvent.prototype.constructor = UIEvent;
  globalThis.UIEvent = UIEvent;

  function MouseEvent(type, init) {
    UIEvent.call(this, type, init);
    init = init || {};
    this.screenX = Number(init.screenX || 0);
    this.screenY = Number(init.screenY || 0);
    this.clientX = Number(init.clientX || 0);
    this.clientY = Number(init.clientY || 0);
    this.ctrlKey = !!init.ctrlKey;
    this.shiftKey = !!init.shiftKey;
    this.altKey = !!init.altKey;
    this.metaKey = !!init.metaKey;
    this.button = Number(init.button || 0);
    this.buttons = Number(init.buttons || 0);
    this.relatedTarget = init.relatedTarget || null;
  }
  MouseEvent.prototype = Object.create(UIEvent.prototype);
  MouseEvent.prototype.constructor = MouseEvent;
  globalThis.MouseEvent = MouseEvent;

  // PointerEvent extends MouseEvent (Pointer Events): pointer identity + geometry.
  function PointerEvent(type, init) {
    MouseEvent.call(this, type, init);
    init = init || {};
    this.pointerId = Number(init.pointerId || 0);
    this.width = Number(init.width || 1);
    this.height = Number(init.height || 1);
    this.pressure = Number(init.pressure || 0);
    this.tangentialPressure = Number(init.tangentialPressure || 0);
    this.tiltX = Number(init.tiltX || 0);
    this.tiltY = Number(init.tiltY || 0);
    this.twist = Number(init.twist || 0);
    this.pointerType = init.pointerType !== undefined ? String(init.pointerType) : '';
    this.isPrimary = !!init.isPrimary;
  }
  PointerEvent.prototype = Object.create(MouseEvent.prototype);
  PointerEvent.prototype.constructor = PointerEvent;
  globalThis.PointerEvent = PointerEvent;

  // WheelEvent extends MouseEvent (CSSOM View / UI Events): scroll deltas.
  var WHEEL_MODE = { PIXEL: 0, LINE: 1, PAGE: 2 };
  function WheelEvent(type, init) {
    MouseEvent.call(this, type, init);
    init = init || {};
    this.deltaX = Number(init.deltaX || 0);
    this.deltaY = Number(init.deltaY || 0);
    this.deltaZ = Number(init.deltaZ || 0);
    this.deltaMode = Number(init.deltaMode || 0);
  }
  WheelEvent.prototype = Object.create(MouseEvent.prototype);
  WheelEvent.prototype.constructor = WheelEvent;
  WheelEvent.DOM_DELTA_PIXEL = WHEEL_MODE.PIXEL;
  WheelEvent.DOM_DELTA_LINE = WHEEL_MODE.LINE;
  WheelEvent.DOM_DELTA_PAGE = WHEEL_MODE.PAGE;
  globalThis.WheelEvent = WheelEvent;

  // KeyboardEvent extends UIEvent (UI Events): key identity + modifiers.
  function KeyboardEvent(type, init) {
    UIEvent.call(this, type, init);
    init = init || {};
    this.key = init.key !== undefined ? String(init.key) : '';
    this.code = init.code !== undefined ? String(init.code) : '';
    this.location = Number(init.location || 0);
    this.ctrlKey = !!init.ctrlKey;
    this.shiftKey = !!init.shiftKey;
    this.altKey = !!init.altKey;
    this.metaKey = !!init.metaKey;
    this.repeat = !!init.repeat;
    this.isComposing = !!init.isComposing;
  }
  KeyboardEvent.prototype = Object.create(UIEvent.prototype);
  KeyboardEvent.prototype.constructor = KeyboardEvent;
  KeyboardEvent.prototype.getModifierState = function(k) {
    switch (String(k)) {
      case 'Control': return this.ctrlKey;
      case 'Shift': return this.shiftKey;
      case 'Alt': return this.altKey;
      case 'Meta': return this.metaKey;
      default: return false;
    }
  };
  globalThis.KeyboardEvent = KeyboardEvent;

  // Touch + TouchList + TouchEvent (Touch Events). A Touch is a plain data
  // object (not an Event); TouchList is array-like; TouchEvent extends UIEvent
  // and carries the three touch lists plus the modifier flags.
  function Touch(init) {
    init = init || {};
    this.identifier = Number(init.identifier || 0);
    this.target = init.target || null;
    this.screenX = Number(init.screenX || 0);
    this.screenY = Number(init.screenY || 0);
    this.clientX = Number(init.clientX || 0);
    this.clientY = Number(init.clientY || 0);
    this.pageX = Number(init.pageX || 0);
    this.pageY = Number(init.pageY || 0);
    this.radiusX = Number(init.radiusX || 0);
    this.radiusY = Number(init.radiusY || 0);
    this.rotationAngle = Number(init.rotationAngle || 0);
    this.force = Number(init.force || 0);
  }
  globalThis.Touch = Touch;
  function makeTouchList(touches) {
    var list = { length: touches.length };
    for (var i = 0; i < touches.length; i++) { list[i] = touches[i]; }
    list.item = function(i) { return this[i] || null; };
    return list;
  }
  globalThis.TouchList = function() { return makeTouchList([]); };
  function TouchEvent(type, init) {
    UIEvent.call(this, type, init);
    init = init || {};
    this.touches = makeTouchList(init.touches || []);
    this.targetTouches = makeTouchList(init.targetTouches || []);
    this.changedTouches = makeTouchList(init.changedTouches || []);
    this.ctrlKey = !!init.ctrlKey;
    this.shiftKey = !!init.shiftKey;
    this.altKey = !!init.altKey;
    this.metaKey = !!init.metaKey;
  }
  TouchEvent.prototype = Object.create(UIEvent.prototype);
  TouchEvent.prototype.constructor = TouchEvent;
  globalThis.TouchEvent = TouchEvent;

  // The Window's listeners live on `globalThis` **itself**, not on a private
  // EventTarget instance. This is load-bearing: `Node.prototype.dispatchEvent`
  // (dom/bootstrap.js) walks the tree and, at the top of the path, fires the
  // window by reading `globalThis.__listeners` directly. If the listeners lived
  // anywhere else, `window.addEventListener('click', …)` would never fire for a
  // click on the page — it would only see events dispatched *at* the window.
  //
  // The options argument is forwarded, so `{capture, once, passive}` reaches the
  // window too (dropping it silently made every window listener non-passive).
  globalThis.addEventListener = function(type, cb, opts) {
    EventTarget.prototype.addEventListener.call(globalThis, type, cb, opts);
  };
  globalThis.removeEventListener = function(type, cb, opts) {
    EventTarget.prototype.removeEventListener.call(globalThis, type, cb, opts);
  };
  globalThis.dispatchEvent = function(event) {
    return EventTarget.prototype.dispatchEvent.call(globalThis, event);
  };
  // `dispatchEvent` fires through `this.__fire`, and globalThis does not inherit
  // from EventTarget.prototype — it only borrows its methods — so it needs its
  // own. (Node's tree walk calls the standalone `fire` in dom/bootstrap.js, which
  // reads `__listeners` off the node directly, so it needs nothing here.)
  globalThis.__fire = function(event, key) {
    return EventTarget.prototype.__fire.call(globalThis, event, key);
  };

  // "Report the exception" (HTML): fire an ErrorEvent-shaped `error` event at
  // the global, then fall back to the console. This is what a listener's
  // exception does instead of propagating out of dispatchEvent — and it is how
  // testharness.js notices: it does `addEventListener("error", …)` and reads
  // `message` / `error` / `filename` / `lineno` / `colno` to fail the running
  // test. Guard against recursion: a throwing error-listener must not re-enter.
  var reporting = false;
  globalThis.__reportListenerException = function(ex) {
    var msg = (ex && ex.message !== undefined) ? String(ex.message) : String(ex);
    if (!reporting) {
      reporting = true;
      try {
        var ev = new Event('error', { cancelable: true });
        ev.message = msg;
        ev.error = ex;
        ev.filename = '';
        ev.lineno = 0;
        ev.colno = 0;
        globalThis.dispatchEvent(ev);
      } catch (ignored) {
        // An error handler that itself threw: fall through to the console.
      }
      reporting = false;
    }
    if (globalThis.console && globalThis.console.error) {
      globalThis.console.error('Uncaught (in listener): ' + msg);
    }
  };
})();
"#;

/// A JS string literal (quotes + minimal escaping) for embedding a Rust string in
/// evaluated source. Used to build the `location` object from URL components and to
/// carry a fetch outcome's JSON into `__fetchSettle`. U+2028 / U+2029 are escaped
/// because, unescaped, they are JS line terminators that would break the eval'd
/// source (a header value or URL can contain them).
fn js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Push the stringified first argument into `HostState::console`.
fn record_console<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
    let arg = cx.arg(0);
    let line = cx.value_to_string(&arg)?;
    if let Some(data) = cx.host_data() {
        if let Some(host) = data.downcast_ref::<RefCell<HostState>>() {
            host.borrow_mut().console.push(line);
        }
    }
    Ok(cx.undefined())
}

fn trace_arg<E: ScriptEngine>(cx: &mut E::CallCx<'_>, i: usize) -> Result<String, E::Error> {
    let value = cx.arg(i);
    cx.value_to_string(&value)
}

struct TraceProtocol;
impl<E: ScriptEngine> NativeFn<E> for TraceProtocol {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let boundary = trace_arg::<E>(cx, 0)?;
        let phase = trace_arg::<E>(cx, 1)?;
        let detail = trace_arg::<E>(cx, 2)?;
        if let Some(data) = cx.host_data() {
            if let Some(host) = data.downcast_ref::<RefCell<HostState>>() {
                host.borrow_mut().pending_trace.push(PendingTraceEvent {
                    boundary,
                    phase,
                    detail: if detail == "undefined" {
                        None
                    } else {
                        Some(detail)
                    },
                });
            }
        }
        Ok(cx.undefined())
    }
}

struct ConsoleLog;
impl<E: ScriptEngine> NativeFn<E> for ConsoleLog {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        record_console::<E>(cx)
    }
}

struct ConsoleError;
impl<E: ScriptEngine> NativeFn<E> for ConsoleError {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        record_console::<E>(cx)
    }
}

/// Read argument `i` as an `f64`. JS numbers cross the native boundary as strings
/// (the `CallCx` surface marshals no numbers), so this stringifies then parses;
/// a non-numeric or absent argument is `0.0`.
fn scroll_arg<E: ScriptEngine>(cx: &mut E::CallCx<'_>, i: usize) -> Result<f64, E::Error> {
    let v = cx.arg(i);
    let s = cx.value_to_string(&v)?;
    Ok(s.trim().parse::<f64>().unwrap_or(0.0))
}

/// The document scroll the host last synced into [`HostState`] (`(0, 0)` if host
/// data is unavailable).
fn read_scroll<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> (f32, f32) {
    cx.host_data()
        .and_then(|d| {
            d.downcast_ref::<RefCell<HostState>>()
                .map(|h| h.borrow().viewport_scroll)
        })
        .unwrap_or((0.0, 0.0))
}

/// Store the document scroll script set, for the host to clamp + apply. Clears any
/// pending `scrollIntoView` (an absolute scroll command supersedes it — last wins).
fn write_scroll<E: ScriptEngine>(cx: &mut E::CallCx<'_>, scroll: (f32, f32)) {
    if let Some(data) = cx.host_data() {
        if let Some(host) = data.downcast_ref::<RefCell<HostState>>() {
            let mut host = host.borrow_mut();
            host.viewport_scroll = scroll;
            host.scroll_into_view = None;
        }
    }
}

/// `__scrollIntoView(elementRef)` → record the element as the host's pending
/// scroll-into-view target (`Element.scrollIntoView`); the host resolves it after
/// the run.
struct ScrollIntoView;
impl<E: ScriptEngine> NativeFn<E> for ScrollIntoView {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let el = cx.arg(0);
        if let Some(raw) = cx.reflector_data(&el) {
            let node = NodeId::from_raw(raw as usize);
            if let Some(data) = cx.host_data() {
                if let Some(host) = data.downcast_ref::<RefCell<HostState>>() {
                    host.borrow_mut().scroll_into_view = Some(node);
                }
            }
        }
        Ok(cx.undefined())
    }
}

/// `__scrollTo(x, y)` → set the document scroll absolutely (`window.scrollTo`).
struct ScrollTo;
impl<E: ScriptEngine> NativeFn<E> for ScrollTo {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let x = scroll_arg::<E>(cx, 0)? as f32;
        let y = scroll_arg::<E>(cx, 1)? as f32;
        write_scroll::<E>(cx, (x, y));
        Ok(cx.undefined())
    }
}

/// `__scrollBy(dx, dy)` → offset the document scroll (`window.scrollBy`).
struct ScrollBy;
impl<E: ScriptEngine> NativeFn<E> for ScrollBy {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let dx = scroll_arg::<E>(cx, 0)? as f32;
        let dy = scroll_arg::<E>(cx, 1)? as f32;
        let (x, y) = read_scroll::<E>(cx);
        write_scroll::<E>(cx, (x + dx, y + dy));
        Ok(cx.undefined())
    }
}

/// `__scrollX()` → the document scroll x as a string (`window.scrollX` `Number()`s it).
struct ScrollX;
impl<E: ScriptEngine> NativeFn<E> for ScrollX {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let x = read_scroll::<E>(cx).0;
        cx.make_string(&x.to_string())
    }
}

/// `__scrollY()` → the document scroll y as a string (`window.scrollY` `Number()`s it).
struct ScrollY;
impl<E: ScriptEngine> NativeFn<E> for ScrollY {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let y = read_scroll::<E>(cx).1;
        cx.make_string(&y.to_string())
    }
}

/// The viewport's CSS-px size from host state (`read_scroll`'s sibling).
fn read_viewport_size<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> (f32, f32) {
    cx.host_data()
        .and_then(|d| {
            d.downcast_ref::<RefCell<HostState>>()
                .map(|h| h.borrow().viewport_size)
        })
        .unwrap_or((0.0, 0.0))
}

/// `__innerWidth()` → the viewport width in CSS px as a string.
struct InnerWidth;
impl<E: ScriptEngine> NativeFn<E> for InnerWidth {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let w = read_viewport_size::<E>(cx).0;
        cx.make_string(&w.to_string())
    }
}

/// `__innerHeight()` → the viewport height in CSS px as a string.
struct InnerHeight;
impl<E: ScriptEngine> NativeFn<E> for InnerHeight {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let h = read_viewport_size::<E>(cx).1;
        cx.make_string(&h.to_string())
    }
}

/// `window.innerWidth` / `innerHeight` (plus the `outerWidth` / `outerHeight`
/// aliases and `devicePixelRatio`) over the `__inner*` natives. Numbers cross the
/// native boundary as strings, like `__scroll*`, so the getters `Number()` them.
const VIEWPORT_BOOTSTRAP: &str = r#"
(function() {
  function def(name, fn) {
    Object.defineProperty(globalThis, name, { configurable: true, get: fn });
  }
  def('innerWidth', function() { return Number(__innerWidth()); });
  def('innerHeight', function() { return Number(__innerHeight()); });
  // No window chrome in an embedded surface: outer == inner.
  def('outerWidth', function() { return Number(__innerWidth()); });
  def('outerHeight', function() { return Number(__innerHeight()); });
  def('devicePixelRatio', function() { return 1; });
})();
"#;

/// `window.scrollTo` / `scrollBy` (the `(x, y)` and `{ left, top }` forms) plus the
/// `scrollX` / `scrollY` / `pageXOffset` / `pageYOffset` getters, over the `__scroll*`
/// natives. `window` is the global, so these define on `globalThis`.
const SCROLL_BOOTSTRAP: &str = r#"
(function() {
  function coords(a, b) {
    if (a && typeof a === 'object') { return [a.left || 0, a.top || 0]; }
    return [a || 0, b || 0];
  }
  globalThis.scrollTo = function(a, b) { var c = coords(a, b); __scrollTo(c[0], c[1]); };
  globalThis.scrollBy = function(a, b) { var c = coords(a, b); __scrollBy(c[0], c[1]); };
  function getter(name, fn) {
    Object.defineProperty(globalThis, name, { configurable: true, get: fn });
  }
  getter('scrollX', function() { return Number(__scrollX()); });
  getter('scrollY', function() { return Number(__scrollY()); });
  getter('pageXOffset', function() { return Number(__scrollX()); });
  getter('pageYOffset', function() { return Number(__scrollY()); });
})();
"#;

/// `postMessage` (async `message` delivery to the global) plus minimal `location`
/// and `navigator` stubs the harness touches at load. Async delivery rides the
/// event loop, so a `postMessage` only arrives after `run_event_loop`.
const SHELL_GLOBALS_BOOTSTRAP: &str = r#"
(function() {
  var nextMessageId = 1;
  globalThis.postMessage = function(data) {
    var event = new Event('message');
    var id = String(nextMessageId++);
    event.data = data;
    __traceProtocol('post_message', 'enqueue', id);
    setTimeout(function() {
      __traceProtocol('post_message', 'deliver', id);
      dispatchEvent(event);
    }, 0);
  };
  // A top-level window: parent/top are itself, no opener. testharness walks
  // `while (w != w.parent)`, so a self-referential parent terminates the walk.
  globalThis.parent = globalThis;
  globalThis.top = globalThis;
  globalThis.opener = null;
  // `globalThis.location` is installed by the `platform` surface (a live view of
  // the document URL, defaulting to about:blank); not snapshotted here.
  globalThis.navigator = { userAgent: 'genet', platform: '', language: 'en-US' };

  // AnimationFrameProvider (window). The window's associated document is this
  // runtime's one document, so window-global state realizes the document's
  // "map of animation frame callbacks" and identifier counter. The host drives
  // passes via Runtime::run_animation_frame_callbacks (its tick owns the clock).
  // https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames
  var rafCallbacks = {};   // "map of animation frame callbacks" (handle -> callback)
  var rafId = 0;           // "animation frame callback identifier"
  var rafPass = null;      // key snapshot of an in-progress "run the animation frame callbacks"
  globalThis.requestAnimationFrame = function(callback) {
    if (typeof callback !== 'function') {
      throw new TypeError('requestAnimationFrame: callback is not callable');
    }
    // Step 3: "Increment target's animation frame callback identifier by one, and let handle be the result."
    rafId += 1;
    // Step 5: "Set callbacks[handle] to callback."
    rafCallbacks[rafId] = callback;
    // Step 6: "Return handle."
    return rafId;
  };
  globalThis.cancelAnimationFrame = function(handle) {
    // Step 3: "Remove callbacks[handle]."
    delete rafCallbacks[handle];
  };
  // "Run the animation frame callbacks", one callback per host call, so the Rust
  // drive loop can run a microtask checkpoint between callbacks (the JS stack is
  // empty there, matching the checkpoint browsers exhibit between rAF callbacks;
  // same granularity as the per-timer-task checkpoint). Returns 1 if a callback
  // ran, 0 when the pass is exhausted (which also ends the pass).
  globalThis.__runOneAnimationFrameCallback = function(now) {
    if (rafPass === null) {
      // Step 2: "Let callbackHandles be the result of getting the keys of callbacks."
      rafPass = Object.keys(rafCallbacks);
    }
    while (rafPass.length > 0) {
      var handle = rafPass.shift();
      // Step 3: "For each handle in callbackHandles, if handle exists in callbacks:"
      if (!(handle in rafCallbacks)) { continue; }
      var callback = rafCallbacks[handle];
      // Step 3: "Remove callbacks[handle]." Before invoking, so a
      // requestAnimationFrame from inside the callback lands in the next pass.
      delete rafCallbacks[handle];
      // Step 3: "Invoke callback with « now » and "report"."
      // Note: an exception propagates to the host (matching the timer path in
      // __runTimers) instead of the spec's report-and-continue.
      callback(now);
      return 1;
    }
    rafPass = null;
    return 0;
  };
  // Whether any frame callbacks are registered; the host keeps requesting
  // frames only while script is animating.
  globalThis.__hasAnimationFrameCallbacks = function() {
    for (var k in rafCallbacks) { return true; }
    return false;
  };

  // DOMException with the legacy name→code table, so tests that construct it,
  // check `instanceof DOMException`, or read `.code`/`.name` work. (DOM methods
  // throwing the *right* exception for bad input is later, deeper work.)
  var CODES = {
    IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4,
    InvalidCharacterError: 5, NoModificationAllowedError: 7, NotFoundError: 8,
    NotSupportedError: 9, InUseAttributeError: 10, InvalidStateError: 11,
    SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14,
    InvalidAccessError: 15, TypeMismatchError: 17, SecurityError: 18,
    NetworkError: 19, AbortError: 20, URLMismatchError: 21,
    QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24,
    DataCloneError: 25
  };
  function DOMException(message, name) {
    this.message = message === undefined ? '' : String(message);
    this.name = name === undefined ? 'Error' : String(name);
    this.code = CODES[this.name] || 0;
  }
  DOMException.prototype = Object.create(Error.prototype);
  DOMException.prototype.constructor = DOMException;
  DOMException.prototype.toString = function() { return this.name + ': ' + this.message; };
  // Legacy ALLCAPS code constants on both constructor and prototype.
  var LEGACY = {
    INDEX_SIZE_ERR: 1, HIERARCHY_REQUEST_ERR: 3, WRONG_DOCUMENT_ERR: 4,
    INVALID_CHARACTER_ERR: 5, NO_MODIFICATION_ALLOWED_ERR: 7, NOT_FOUND_ERR: 8,
    NOT_SUPPORTED_ERR: 9, INUSE_ATTRIBUTE_ERR: 10, INVALID_STATE_ERR: 11,
    SYNTAX_ERR: 12, INVALID_MODIFICATION_ERR: 13, NAMESPACE_ERR: 14,
    INVALID_ACCESS_ERR: 15, TYPE_MISMATCH_ERR: 17, SECURITY_ERR: 18,
    NETWORK_ERR: 19, ABORT_ERR: 20, URL_MISMATCH_ERR: 21,
    QUOTA_EXCEEDED_ERR: 22, TIMEOUT_ERR: 23, INVALID_NODE_TYPE_ERR: 24,
    DATA_CLONE_ERR: 25
  };
  for (var k in LEGACY) { DOMException[k] = DOMException.prototype[k] = LEGACY[k]; }
  globalThis.DOMException = DOMException;
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// The host surface, exercised against any backend: global aliases resolve and
    /// `console.log` reaches host state through real JS execution.
    fn host_surface_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        // `self` and `window` are the global.
        rt.eval("if (self !== globalThis || window !== globalThis) throw new Error('alias')")
            .expect("aliases");

        // console.log records into host state, in order.
        rt.eval("console.log('one'); console.log('two'); console.error('three');")
            .expect("console");

        assert_eq!(rt.host().borrow().console, vec!["one", "two", "three"]);
    }

    /// The event loop, exercised against any backend: timers fire in `(delay,
    /// insertion)` order when drained, `clearTimeout` cancels, the budget bounds
    /// intervals.
    fn event_loop_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        // Ordering: a later-scheduled shorter delay fires first; equal delays keep
        // insertion order. A cleared timer never fires.
        rt.eval(
            "setTimeout(function(){ console.log('a'); }, 10);\
             setTimeout(function(){ console.log('b'); }, 0);\
             setTimeout(function(){ console.log('c'); }, 0);\
             var id = setTimeout(function(){ console.log('canceled'); }, 0);\
             clearTimeout(id);",
        )
        .expect("schedule");

        // Nothing fires until the loop runs.
        assert!(rt.host().borrow().console.is_empty());

        rt.run_event_loop(100).expect("drain");
        assert_eq!(rt.host().borrow().console, vec!["b", "c", "a"]);

        // setInterval re-enqueues; the budget caps total firings.
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval("var n = 0; setInterval(function(){ n++; console.log('tick'); }, 0);")
            .expect("interval");
        rt.run_event_loop(3).expect("drain capped");
        assert_eq!(rt.host().borrow().console, vec!["tick", "tick", "tick"]);
    }

    /// EventTarget, exercised against any backend: listeners fire on dispatch,
    /// `removeEventListener` detaches, `preventDefault` flips the dispatch result.
    fn event_target_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "function onX(e){ console.log('x:' + e.type); }\
             self.addEventListener('x', onX);\
             self.addEventListener('x', function(){ console.log('x2'); });\
             window.dispatchEvent(new Event('x'));\
             self.removeEventListener('x', onX);\
             self.dispatchEvent(new Event('x'));",
        )
        .expect("events");

        // First dispatch hits both listeners; after removing onX, only x2 fires.
        assert_eq!(rt.host().borrow().console, vec!["x:x", "x2", "x2"]);

        // preventDefault on a cancelable event makes dispatchEvent return false.
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            "self.addEventListener('y', function(e){ e.preventDefault(); });\
             var e = new Event('y', { cancelable: true });\
             console.log(String(self.dispatchEvent(e)));\
             console.log(String(self.dispatchEvent(new Event('z'))));",
        )
        .expect("cancel");
        assert_eq!(rt.host().borrow().console, vec!["false", "true"]);
    }

    /// Microtasks, against any backend: Promise continuations do not run until a
    /// checkpoint, then drain to quiescence (chained `.then` runs fully), and a
    /// promise resolved inside a timer callback drains during the event loop.
    fn microtasks_work<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "Promise.resolve('v').then(function(v){ console.log('then:' + v); });\
             Promise.resolve().then(function(){ console.log('a'); }).then(function(){ console.log('b'); });",
        )
        .expect("promise script");

        // Nothing runs until the checkpoint.
        assert!(rt.host().borrow().console.is_empty());

        rt.run_microtasks();
        // Both chains drained, in enqueue order (a's second .then is queued after the
        // first then: and a callbacks run).
        assert_eq!(rt.host().borrow().console, vec!["then:v", "a", "b"]);

        // A promise resolved inside a timer callback drains during run_event_loop.
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval("setTimeout(function(){ Promise.resolve().then(function(){ console.log('timer-microtask'); }); }, 0);")
            .expect("timer script");
        rt.run_event_loop(10).expect("loop");
        assert_eq!(rt.host().borrow().console, vec!["timer-microtask"]);
    }

    /// A microtask queued by timer task N must run before timer task N+1. This is
    /// the fine-grained checkpoint shape the batch checkpoint missed.
    fn per_timer_microtask_checkpoint_works<E: ScriptEngine>() {
        let script = "setTimeout(function(){ \
                          console.log('first'); \
                          Promise.resolve().then(function(){ console.log('micro'); }); \
                      }, 0); \
                      setTimeout(function(){ console.log('second'); }, 0);";

        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(script).expect("schedule");
        rt.run_event_loop(10).expect("loop");
        assert_eq!(rt.host().borrow().console, vec!["first", "micro", "second"]);

        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(script).expect("schedule");
        assert_eq!(rt.run_timers(10, 0.0), 2);
        assert_eq!(rt.host().borrow().console, vec!["first", "micro", "second"]);
    }

    /// A `MediaQueryList`'s `.matches` is live, and `change` fires (via
    /// `addEventListener('change')`) when the host re-evaluates after the device
    /// flips — once per genuine flip, not on a no-op re-evaluation, and both
    /// directions.
    fn match_media_change_events_work<E: ScriptEngine>() {
        use std::cell::Cell;
        use std::rc::Rc;

        struct FlipHandler {
            on: Rc<Cell<bool>>,
        }
        impl MediaQueryHandler for FlipHandler {
            fn evaluate(&self, query: &str) -> (String, bool) {
                (query.to_string(), self.on.get())
            }
        }

        let flag = Rc::new(Cell::new(false));
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.set_media_query_handler(Box::new(FlipHandler { on: flag.clone() }));
        rt.eval(
            "globalThis.__seen = []; \
             var mql = matchMedia('(min-width: 500px)'); \
             mql.addEventListener('change', function(e){ __seen.push('c:' + e.matches); }); \
             console.log('init:' + mql.matches);",
        )
        .expect("setup");
        let last = |rt: &Runtime<E>| rt.host().borrow().console.last().cloned().unwrap_or_default();
        assert_eq!(last(&rt), "init:false");

        // Device flips to matching + notify -> one change; `.matches` now true.
        flag.set(true);
        rt.notify_media_features_changed().expect("notify");
        rt.eval("console.log('after:' + mql.matches + '|' + __seen.join(','));")
            .expect("read");
        assert_eq!(last(&rt), "after:true|c:true");

        // Re-notify with no state change -> no new event.
        rt.notify_media_features_changed().expect("notify2");
        rt.eval("console.log('again:' + __seen.join(','));").expect("read2");
        assert_eq!(last(&rt), "again:c:true");

        // Flip back -> another change (both directions fire).
        flag.set(false);
        rt.notify_media_features_changed().expect("notify3");
        rt.eval("console.log('final:' + mql.matches + '|' + __seen.join(','));")
            .expect("read3");
        assert_eq!(last(&rt), "final:false|c:true,c:false");
    }

    /// rAF callbacks run once per pass in registration order with the frame
    /// timestamp; a microtask queued by callback N runs before callback N+1
    /// (same pass); a handle canceled mid-pass is skipped; a callback
    /// registered mid-pass lands in the next frame; an idle runtime reports no
    /// registered callbacks and a zero-count pass.
    fn animation_frame_callbacks_work<E: ScriptEngine>() {
        let script = "var c3; \
                      requestAnimationFrame(function(now){ \
                          console.log('one@' + now); \
                          Promise.resolve().then(function(){ console.log('micro'); }); \
                          cancelAnimationFrame(c3); \
                          requestAnimationFrame(function(){ console.log('next'); }); \
                      }); \
                      requestAnimationFrame(function(){ console.log('two'); }); \
                      c3 = requestAnimationFrame(function(){ console.log('canceled'); });";

        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(script).expect("schedule");
        assert!(rt.has_animation_frame_callbacks());

        // Pass 1: snapshot is the three registered handles; the mid-pass
        // registration ('next') must not run, the canceled one must be skipped,
        // and 'micro' must land between the two callbacks that do run.
        assert_eq!(rt.run_animation_frame_callbacks(16.0).expect("frame 1"), 2);
        assert_eq!(rt.host().borrow().console, vec!["one@16", "micro", "two"]);
        assert!(rt.has_animation_frame_callbacks());

        // Pass 2: only the mid-pass registration remains.
        assert_eq!(rt.run_animation_frame_callbacks(32.0).expect("frame 2"), 1);
        assert_eq!(
            rt.host().borrow().console,
            vec!["one@16", "micro", "two", "next"]
        );

        // Idle: nothing registered, nothing runs.
        assert!(!rt.has_animation_frame_callbacks());
        assert_eq!(rt.run_animation_frame_callbacks(48.0).expect("frame 3"), 0);
    }

    /// E4 trace seed: the runtime emits deterministic NDJSON over the named
    /// scheduler boundaries so a follow-on generator can lower it into TLA+
    /// constants.
    fn scheduler_trace_ndjson_works<E: ScriptEngine>() {
        let script = "setTimeout(function(){ \
                          Promise.resolve().then(function(){ console.log('micro'); }); \
                      }, 0);";
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(script).expect("schedule");
        rt.run_event_loop(10).expect("loop");
        let document = rt.host().borrow().dom.document().raw();
        let _ = rt.dispatch_event(document, "click").expect("dispatch");

        rt.eval(script).expect("reschedule");
        assert_eq!(rt.run_timers(10, 0.0), 1);

        let trace = rt.scheduler_trace();
        assert!(
            trace.windows(2).all(|w| w[0].seq + 1 == w[1].seq),
            "trace seqs must be contiguous: {trace:?}"
        );
        for (boundary, phase) in [
            ("eval", "start"),
            ("eval", "end"),
            ("dispatch_event", "start"),
            ("dispatch_event", "end"),
            ("run_event_loop", "start"),
            ("run_event_loop", "end"),
            ("run_timers", "start"),
            ("run_timers", "end"),
            ("timer_task", "performed"),
            ("pump_microtasks", "start"),
            ("pump_microtasks", "end"),
        ] {
            assert!(
                trace
                    .iter()
                    .any(|event| event.boundary == boundary && event.phase == phase),
                "missing {boundary}/{phase} in {trace:?}"
            );
        }

        let ndjson = rt.scheduler_trace_ndjson();
        assert!(
            ndjson
                .lines()
                .any(|line| line.contains(r#""boundary":"run_timers""#)),
            "run_timers boundary missing from NDJSON: {ndjson}"
        );
        assert!(
            ndjson.lines().all(|line| line.starts_with("{\"seq\":")),
            "each line is one trace object: {ndjson}"
        );
    }

    /// E4 protocol witness: `postMessage` records one enqueue mark during the
    /// calling script, one delivery mark during the later event-loop task, and
    /// delivery stays async.
    fn post_message_trace_ndjson_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "self.addEventListener('message', function(e){ console.log('msg:' + e.data); });\
             self.postMessage('hi');",
        )
        .expect("postMessage script");

        assert!(rt.host().borrow().console.is_empty(), "delivery is async");
        rt.run_event_loop(10).expect("loop");
        assert_eq!(rt.host().borrow().console, vec!["msg:hi"]);

        let trace = rt.scheduler_trace();
        let enqueue_idx = trace
            .iter()
            .position(|event| event.boundary == "post_message" && event.phase == "enqueue")
            .expect("postMessage enqueue trace");
        let deliver_idx = trace
            .iter()
            .position(|event| event.boundary == "post_message" && event.phase == "deliver")
            .expect("postMessage deliver trace");
        let loop_start_idx = trace
            .iter()
            .position(|event| event.boundary == "run_event_loop" && event.phase == "start")
            .expect("run_event_loop start");
        let loop_end_idx = trace
            .iter()
            .position(|event| event.boundary == "run_event_loop" && event.phase == "end")
            .expect("run_event_loop end");
        let timer_idx = trace
            .iter()
            .position(|event| event.boundary == "timer_task" && event.phase == "performed")
            .expect("timer task");

        assert!(
            enqueue_idx < deliver_idx,
            "delivery must follow enqueue: {trace:?}"
        );
        assert!(
            loop_start_idx < deliver_idx && deliver_idx < timer_idx && timer_idx < loop_end_idx,
            "delivery must happen during the later event-loop task: {trace:?}"
        );

        let enqueue = &trace[enqueue_idx];
        let deliver = &trace[deliver_idx];
        assert_eq!(
            enqueue.detail, deliver.detail,
            "enqueue and deliver must carry the same message id: {trace:?}"
        );

        let ndjson = rt.scheduler_trace_ndjson();
        assert!(
            ndjson
                .lines()
                .any(|line| line.contains(r#""boundary":"post_message""#)
                    && line.contains(r#""phase":"deliver""#)),
            "postMessage delivery missing from NDJSON: {ndjson}"
        );
    }

    #[test]
    fn host_surface_on_boa() {
        host_surface_works::<script_engine_boa::BoaEngine>();
    }

    /// The window scroll API (`scrollTo` / `scrollBy` / `scrollX|Y`), against any
    /// backend: the host syncs the current document scroll into `HostState`, script
    /// reads it as numbers and sets it (absolute + relative + the `{left, top}`
    /// form), and the host reads back the value to clamp + apply.
    fn window_scroll_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        // The host syncs the current document scroll in before running script.
        rt.host().borrow_mut().viewport_scroll = (0.0, 120.0);
        // Script reads it through window.scrollX / scrollY as numbers (not strings).
        rt.eval("console.log((window.scrollX === 0) + ',' + (window.scrollY === 120));")
            .expect("read scroll");
        assert_eq!(rt.host().borrow().console, vec!["true,true"]);

        // scrollTo sets absolutely, scrollBy offsets; the host reads the value back.
        rt.eval("window.scrollTo(0, 500); window.scrollBy(0, 50);")
            .expect("set scroll");
        assert_eq!(rt.host().borrow().viewport_scroll, (0.0, 550.0));

        // The options form scrollTo({ left, top }) works too.
        rt.eval("window.scrollTo({ left: 10, top: 700 });")
            .expect("scroll options");
        assert_eq!(rt.host().borrow().viewport_scroll, (10.0, 700.0));
    }

    #[test]
    fn window_scroll_on_boa() {
        window_scroll_works::<script_engine_boa::BoaEngine>();
    }

    /// `Element.scrollIntoView`, against any backend: it records the element as the
    /// host's pending scroll-into-view target (the host resolves it to a viewport
    /// scroll after the run, since the runtime cannot lay out). The latest target
    /// wins, and an absolute `scrollTo` supersedes a pending one.
    fn scroll_into_view_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        let src = genet_static_dom::StaticDocument::parse(
            "<html><body><div id=\"a\"></div><div id=\"b\"></div></body></html>",
        );
        rt.load_dom(&src);

        // Nothing pending initially.
        assert!(rt.host().borrow().scroll_into_view.is_none());

        // scrollIntoView records the element as the host's pending target.
        rt.eval("document.getElementById('a').scrollIntoView();")
            .expect("scrollIntoView a");
        let a = rt.host().borrow().scroll_into_view.expect("a recorded");

        // A later call records a different element (element-specific, last wins).
        rt.eval("document.getElementById('b').scrollIntoView();")
            .expect("scrollIntoView b");
        let b = rt.host().borrow().scroll_into_view.expect("b recorded");
        assert_ne!(a, b, "scrollIntoView records the specific element");

        // An absolute scroll supersedes a pending scrollIntoView (last command wins).
        rt.eval("window.scrollTo(0, 0);").expect("scrollTo");
        assert!(
            rt.host().borrow().scroll_into_view.is_none(),
            "scrollTo clears the pending target"
        );
    }

    #[test]
    fn scroll_into_view_on_boa() {
        scroll_into_view_works::<script_engine_boa::BoaEngine>();
    }

    /// postMessage, against any backend: delivery is async (nothing until the loop
    /// runs), then the `message` event carries `data` to a global listener.
    fn post_message_works<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");

        rt.eval(
            "self.addEventListener('message', function(e){ console.log('msg:' + e.data); });\
             self.postMessage('hi');",
        )
        .expect("postMessage script");

        assert!(rt.host().borrow().console.is_empty(), "delivery is async");
        rt.run_event_loop(10).expect("loop");
        assert_eq!(rt.host().borrow().console, vec!["msg:hi"]);
    }

    #[test]
    fn microtasks_on_boa() {
        microtasks_work::<script_engine_boa::BoaEngine>();
    }

    /// Event-handler IDL attributes (`el.onclick = fn`), against any backend.
    /// Three things the WPT `dom/events` cluster depends on: an element handler
    /// fires on dispatch; `document.body.onload` reflects to the Window (the
    /// `load` event is dispatched at the window, and this is how the cluster's
    /// `body.onload = () => runTest()` ever runs); and reassigning a handler to
    /// null removes it while a coexisting addEventListener listener survives.
    fn event_handler_idl_attributes_work<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            r#"
            var h = document.createElement('html');
            var b = document.createElement('body');
            var d = document.createElement('div');
            b.appendChild(d); h.appendChild(b); document.appendChild(h);
            globalThis.__d = d;
            // Element handler + a coexisting addEventListener listener.
            d.onclick = function(){ console.log('onclick'); };
            d.addEventListener('click', function(){ console.log('listener'); });
            // body.onload must reflect to the window.
            document.body.onload = function(){ console.log('body.onload'); };
            // the presence + type checks the WPT tests do.
            console.log('has:' + ('onclick' in d) + ',' + (typeof window.onload));
            "#,
        )
        .expect("setup");

        // The load event is dispatched at the window; body.onload must catch it.
        rt.eval("window.dispatchEvent(new Event('load'));")
            .expect("dispatch load");
        // Click the div: onclick then the addEventListener listener, in set order.
        rt.eval("__d.dispatchEvent(new Event('click', { bubbles: true }));")
            .expect("dispatch click");
        // Null out onclick: the handler goes, the addEventListener listener stays.
        rt.eval(
            "__d.onclick = null;\
             __d.dispatchEvent(new Event('click', { bubbles: true }));",
        )
        .expect("null-out");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                // `'onclick' in d` is true; `typeof window.onload` is 'function'
                // *because* the `document.body.onload =` above reflected onto the
                // window — the load handler is the same slot. That reflection is
                // the whole point.
                "has:true,function".to_string(),
                "body.onload".to_string(),
                "onclick".to_string(),
                "listener".to_string(),
                "listener".to_string(), // after onclick=null, only the listener fires
            ],
        );
    }

    #[test]
    fn event_handler_idl_attributes_on_boa() {
        event_handler_idl_attributes_work::<script_engine_boa::BoaEngine>();
    }

    /// The TouchEvent / WheelEvent / PointerEvent / KeyboardEvent interfaces
    /// construct, carry their fields, honor `cancelable`, and chain `instanceof`
    /// correctly. The `dom/events` passive/cancelable cluster reads
    /// `event.cancelable` off a `touchstart`, so the type and its cancelable flag
    /// must both be real.
    fn typed_event_interfaces_work<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            r#"
            function chain(e, names) {
              // e.g. names = ['TouchEvent','UIEvent','Event']
              return names.map(function(n){ return e instanceof globalThis[n]; }).join(',');
            }
            var t = new TouchEvent('touchstart', {
              cancelable: true, bubbles: true,
              changedTouches: [ new Touch({ identifier: 7, clientX: 30, clientY: 30 }) ],
            });
            console.log('touch:' + t.type + ',' + t.cancelable + ',' + t.bubbles
              + ',' + t.changedTouches.length + ',' + t.changedTouches[0].clientX
              + ',' + chain(t, ['TouchEvent','UIEvent','Event']));
            var w = new WheelEvent('wheel', { deltaX: 1, deltaY: -3, deltaMode: 1 });
            console.log('wheel:' + w.deltaX + ',' + w.deltaY + ',' + w.deltaMode
              + ',' + WheelEvent.DOM_DELTA_LINE
              + ',' + chain(w, ['WheelEvent','MouseEvent','UIEvent','Event']));
            var p = new PointerEvent('pointerdown', { pointerType: 'touch', pointerId: 5, isPrimary: true });
            console.log('pointer:' + p.pointerType + ',' + p.pointerId + ',' + p.isPrimary
              + ',' + chain(p, ['PointerEvent','MouseEvent','Event']));
            var k = new KeyboardEvent('keydown', { key: 'a', code: 'KeyA', ctrlKey: true });
            console.log('key:' + k.key + ',' + k.code + ',' + k.getModifierState('Control')
              + ',' + chain(k, ['KeyboardEvent','UIEvent','Event']));
            "#,
        )
        .expect("event interfaces");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                "touch:touchstart,true,true,1,30,true,true,true".to_string(),
                "wheel:1,-3,1,1,true,true,true,true".to_string(),
                "pointer:touch,5,true,true,true,true".to_string(),
                "key:a,KeyA,true,true,true,true".to_string(),
            ],
        );
    }

    #[test]
    fn typed_event_interfaces_on_boa() {
        typed_event_interfaces_work::<script_engine_boa::BoaEngine>();
    }

    /// A listener's exception is **reported, not propagated** (DOM §dispatch):
    /// dispatch continues to the remaining listeners, `dispatchEvent` returns
    /// normally, and the exception surfaces as an `error` event at the global —
    /// which is exactly how `testharness.js` notices (it does
    /// `addEventListener("error", …)` and fails the running test). Before this,
    /// one throwing `onload` handler propagated out of the harness's load
    /// dispatch and errored out the entire test file.
    fn listener_exceptions_are_reported_not_propagated<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            r#"
            var h = document.createElement('html');
            var b = document.createElement('body');
            b.appendChild(document.createElement('div'));
            h.appendChild(b); document.appendChild(h);
            // testharness-shaped global error handler.
            addEventListener('error', function(e){ console.log('reported:' + e.message); });
            var d = b.firstChild;
            d.addEventListener('click', function(){ throw new Error('boom'); });
            d.addEventListener('click', function(){ console.log('second listener still ran'); });
            "#,
        )
        .expect("setup");

        // The throwing listener must not make dispatchEvent throw.
        rt.eval(
            "document.body.firstChild.dispatchEvent(new Event('click', { bubbles: true }));",
        )
        .expect("dispatch must not propagate the listener's exception");

        let console = rt.host().borrow().console.clone();
        assert!(
            console.iter().any(|l| l == "second listener still ran"),
            "dispatch continues past a throwing listener: {console:?}"
        );
        assert!(
            console.iter().any(|l| l == "reported:boom"),
            "the exception is reported as an `error` event at the global: {console:?}"
        );
    }

    #[test]
    fn listener_exceptions_reported_on_boa() {
        listener_exceptions_are_reported_not_propagated::<script_engine_boa::BoaEngine>();
    }

    /// A `window` listener fires for an event **bubbling up from a node**, and the
    /// listener options reach it.
    ///
    /// Both halves were broken: `globalThis.addEventListener` delegated to a
    /// private `EventTarget` instance, so the window's listeners lived somewhere
    /// `Node.prototype.dispatchEvent` never looked (it fires the top of the
    /// propagation path by reading `globalThis.__listeners`) — meaning
    /// `window.addEventListener('click', …)` never saw a click on the page. And
    /// the wrapper dropped its third argument, so `{passive}` / `{capture}` /
    /// `{once}` silently never applied to a window listener.
    fn window_listeners_receive_bubbled_events<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            r#"
            var h = document.createElement('html');
            var b = document.createElement('body');
            var d = document.createElement('div');
            b.appendChild(d); h.appendChild(b); document.appendChild(h);
            window.addEventListener('click', function(e){
              console.log('window saw click, phase=' + e.eventPhase);
            });
            // Capture on the window runs before the target's own listeners.
            window.addEventListener('click', function(){ console.log('window capture'); }, { capture: true });
            // `once` must reach the window too.
            window.addEventListener('ping', function(){ console.log('once'); }, { once: true });
            d.addEventListener('click', function(){ console.log('div'); });
            globalThis.__d = d;
            "#,
        )
        .expect("setup");

        rt.eval("__d.dispatchEvent(new Event('click', { bubbles: true }));")
            .expect("dispatch");
        // `once` fired from the window: a second dispatch must not re-run it.
        rt.eval("__d.dispatchEvent(new Event('ping', { bubbles: true }));")
            .expect("ping 1");
        rt.eval("__d.dispatchEvent(new Event('ping', { bubbles: true }));")
            .expect("ping 2");

        assert_eq!(
            rt.host().borrow().console,
            vec![
                "window capture".to_string(),           // capture, root -> target
                "div".to_string(),                      // target
                "window saw click, phase=3".to_string(), // bubble (3 = BUBBLING_PHASE)
                "once".to_string(),                     // fires exactly once
            ],
        );
    }

    #[test]
    fn window_listeners_receive_bubbled_events_on_boa() {
        window_listeners_receive_bubbled_events::<script_engine_boa::BoaEngine>();
    }


    /// The G3 GC tick, against any backend: a detached node script holds is
    /// pinned on mint and spared by `collect_garbage`; once its reflector is
    /// reported dead it is reaped. The drain→retire path is covered in the engine
    /// crates (`reflector_for_reports_death_after_gc`); here we simulate the death
    /// by clearing the pin set, to test pin-on-mint + collect's pin-aware reaping.
    fn gc_tick_collects_unpinned_nodes<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        let base = rt.host().borrow().dom.live_node_count();

        // `createElement` hands script a reflector → pin-on-mint pins the node.
        rt.eval("globalThis.d = document.createElement('div');")
            .expect("create");
        rt.run_microtasks();
        assert_eq!(rt.host().borrow().dom.live_node_count(), base + 1);

        // Pinned + detached → collect_garbage spares it.
        let (_, collected) = rt.collect_garbage();
        assert_eq!(collected, 0, "a pinned detached node is spared");
        assert_eq!(rt.host().borrow().dom.live_node_count(), base + 1);

        // Reflector reported dead (simulated): unpin, then collect reaps the orphan.
        rt.host().borrow_mut().pins.clear();
        let (_, collected) = rt.collect_garbage();
        assert_eq!(collected, 1, "an unpinned detached node is reaped");
        assert_eq!(rt.host().borrow().dom.live_node_count(), base);
    }

    #[test]
    fn gc_tick_on_boa() {
        gc_tick_collects_unpinned_nodes::<script_engine_boa::BoaEngine>();
    }

    /// Milestone: the real WPT `testharness.js` loads on the host surface and
    /// defines its API (`test` / `async_test` / `promise_test`). Skips gracefully if
    /// the corpus is absent. The first end-to-end signal that the shell is harness-
    /// ready.
    fn testharness_loads<E: ScriptEngine>() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/wpt/tests/resources/testharness.js"
        );
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("skipping testharness_loads: not found at {path}");
            return;
        };

        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(&src).expect("testharness.js evaluates");
        rt.eval(
            "if (typeof test !== 'function') throw new Error('test missing');\
             if (typeof async_test !== 'function') throw new Error('async_test missing');\
             if (typeof promise_test !== 'function') throw new Error('promise_test missing');\
             if (typeof done !== 'function') throw new Error('done missing');",
        )
        .expect("testharness API present after load");
    }

    #[test]
    fn post_message_on_boa() {
        post_message_works::<script_engine_boa::BoaEngine>();
    }

    /// The end-to-end milestone: a real `testharness.js` test runs and its per-
    /// subtest results come back through the bridge (a passing and a failing
    /// `assert_true`). Skips if the corpus is absent.
    fn testharness_results<E: ScriptEngine>() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/wpt/tests/resources/testharness.js"
        );
        let Ok(harness) = std::fs::read_to_string(path) else {
            eprintln!("skipping testharness_results: not found at {path}");
            return;
        };

        let mut rt = Runtime::<E>::new().expect("runtime");
        let results = rt
            .run_testharness(
                &harness,
                "test(function(){ assert_true(true); }, 'pass-test');\
                 test(function(){ assert_true(false, 'boom'); }, 'fail-test');",
            )
            .expect("run_testharness");

        assert_eq!(results.len(), 2, "two subtests reported: {results:?}");
        let pass = results
            .iter()
            .find(|r| r.name == "pass-test")
            .expect("pass-test present");
        let fail = results
            .iter()
            .find(|r| r.name == "fail-test")
            .expect("fail-test present");
        assert!(pass.passed(), "pass-test should PASS: {pass:?}");
        assert_eq!(fail.status, 1, "fail-test should FAIL: {fail:?}");
    }

    #[test]
    fn testharness_loads_on_boa() {
        testharness_loads::<script_engine_boa::BoaEngine>();
    }

    #[test]
    fn testharness_results_on_boa() {
        testharness_results::<script_engine_boa::BoaEngine>();
    }

    // Nova's regex engine (`regress`) rejects testharness's lone-surrogate scrub
    // regex (`[\ud800-\udbff]`, "not a Unicode scalar value"). `run_testharness`
    // neutralizes that one cosmetic regex (see `neutralize_surrogate_regex`), so
    // Nova now runs the harness to completion and returns results — the same
    // cross-backend path as Boa. (The scrub only ever touched lone-surrogate test
    // names, which the headless bridge does not serialize, so results are
    // unaffected.)
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn testharness_results_on_nova() {
        testharness_results::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn event_handler_idl_attributes_on_nova() {
        event_handler_idl_attributes_work::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn typed_event_interfaces_on_nova() {
        typed_event_interfaces_work::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn runtime_snapshot_clone_on_nova_preserves_js_and_resets_host_state() {
        let mut template = Runtime::<script_engine_nova::NovaEngine>::new().expect("runtime");
        template
            .eval(
                "var counter = 1; \
                 function bump() { counter += 1; return counter; } \
                 console.log('template');",
            )
            .expect("template script");

        let mut clone = template.snapshot_clone().expect("snapshot clone");
        clone
            .eval("console.log(String(bump()));")
            .expect("clone run");
        template
            .eval("console.log(String(counter));")
            .expect("template run");

        assert_eq!(clone.host().borrow().console, vec!["2"]);
        assert_eq!(template.host().borrow().console, vec!["template", "1"]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn testharness_loads_on_nova() {
        testharness_loads::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn post_message_on_nova() {
        post_message_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn microtasks_on_nova() {
        microtasks_work::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn per_timer_microtask_checkpoint_on_boa() {
        per_timer_microtask_checkpoint_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn per_timer_microtask_checkpoint_on_nova() {
        per_timer_microtask_checkpoint_works::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn animation_frame_callbacks_on_boa() {
        animation_frame_callbacks_work::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn animation_frame_callbacks_on_nova() {
        animation_frame_callbacks_work::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn match_media_change_events_on_boa() {
        match_media_change_events_work::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn match_media_change_events_on_nova() {
        match_media_change_events_work::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn scheduler_trace_ndjson_on_boa() {
        scheduler_trace_ndjson_works::<script_engine_boa::BoaEngine>();
    }

    #[test]
    fn post_message_trace_ndjson_on_boa() {
        post_message_trace_ndjson_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn scheduler_trace_ndjson_on_nova() {
        scheduler_trace_ndjson_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn post_message_trace_ndjson_on_nova() {
        post_message_trace_ndjson_works::<script_engine_nova::NovaEngine>();
    }

    #[test]
    fn event_loop_on_boa() {
        event_loop_works::<script_engine_boa::BoaEngine>();
    }

    #[test]
    fn event_target_on_boa() {
        event_target_works::<script_engine_boa::BoaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn host_surface_on_nova() {
        host_surface_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn event_loop_on_nova() {
        event_loop_works::<script_engine_nova::NovaEngine>();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn event_target_on_nova() {
        event_target_works::<script_engine_nova::NovaEngine>();
    }
}
