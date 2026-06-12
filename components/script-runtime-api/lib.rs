// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Browser host surface for serval's scripted tier (the plan's Layer 1).
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
//! bound to a [`serval_scripted_dom::ScriptedDom`] in host state. The event loop,
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
use script_engine_api::{CallCx, NativeFn, ScriptEngine};
use serval_scripted_dom::{NodeId, ScriptedDom};

mod dom;
mod fetch;
mod harness;
mod selector;
mod webgl;

pub use fetch::{FetchHandler, FetchOutcome, FetchRequest};
pub use harness::TestResult;
pub use webgl::{WebGlFactory, WebGlHandler};

/// State the runtime's native callbacks share, stored as the engine's single
/// host-data slot (`Rc<dyn Any>`). One aggregate so every host object reaches the
/// same place; grows as host objects are added (the event-loop task queue and
/// `EventTarget` listeners land here as they graduate from JS bootstraps).
#[derive(Default)]
pub struct HostState {
    /// `console.log` / `console.error` output, in call order.
    pub console: Vec<String>,
    /// The live document the `document`/`Node` surface mutates. Native DOM
    /// callbacks reach it through `CallCx::host_data` (a `RefCell<HostState>`).
    pub dom: ScriptedDom,
    /// Nodes pinned by a live reflector (G1/G3). The DOM surface pins a node
    /// when it hands script a reflector (pin-on-mint); [`Runtime::collect_garbage`]
    /// retires the ids the engine reports dead and passes the survivors to
    /// [`ScriptedDom::collect`] as extra roots, so an orphan script can no longer
    /// reach is reaped.
    pub pins: serval_scripted_dom::Pins,
    /// Per-subtest results collected from `testharness.js` via the completion
    /// callback (the results bridge). Populated by [`Runtime::run_testharness`].
    pub results: Vec<TestResult>,
    /// The host's network seam for `fetch()`. `None` = no network (every fetch is
    /// a network error). Installed by [`Runtime::set_fetch_handler`]; an `Rc` so the
    /// native `__fetch_start` sink clones it out from under the `HostState` borrow
    /// before calling it (the handler must not run with a live borrow). No `Send`
    /// bound, so this crate links no network stack and stays `!Send`.
    pub fetch: Option<std::rc::Rc<dyn FetchHandler>>,
    /// The document base URL, against which relative `fetch()` / `Request` URLs
    /// resolve (the `__resolve_url` sink reads it). `None` = no base (relative URLs
    /// stay relative, so a network fetch of one is an error). Set by
    /// [`Runtime::set_base_url`] for server-mode WPT runs.
    pub base_url: Option<String>,
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
}

impl<E: ScriptEngine> Runtime<E> {
    /// Construct an engine and install the host surface on it.
    pub fn new() -> Result<Self, E::Error> {
        let mut engine = E::new()?;
        let host: SharedHost = Rc::new(RefCell::new(HostState::default()));
        engine.set_host_data(host.clone());
        install_host_surface(&mut engine)?;
        Ok(Self { engine, host })
    }

    /// Evaluate `source` in the runtime's global scope.
    pub fn eval(&mut self, source: &str) -> Result<E::Value, E::Error> {
        self.engine.eval(source)
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
        self.engine.pump_microtasks();
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
    /// microtask checkpoint runs before and after the timer batch (coarse
    /// interleaving; per-task checkpoints are a later refinement). Returns when the
    /// queue drains or the budget is spent.
    pub fn run_event_loop(&mut self, budget: u32) -> Result<(), E::Error> {
        self.engine.pump_microtasks();
        self.engine.eval(&format!("globalThis.__runTimers({budget})"))?;
        self.engine.pump_microtasks();
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
        self.engine.eval(harness_src)?;
        harness::install_bridge(&mut self.engine)?;
        self.engine.eval(test_src)?;
        self.engine.eval("window.dispatchEvent(new Event('load'));")?;
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
        self.engine.eval(harness_src)?;
        harness::install_bridge(&mut self.engine)?;
        self.engine.eval(test_src)?;
        self.engine.eval("window.dispatchEvent(new Event('load'));")?;
        Ok(())
    }

    /// Fire up to `budget` due timers (with microtask checkpoints around the batch)
    /// against the virtual clock at `now_ms` (the real elapsed time of the run), and
    /// return how many fired. Real-time gating lets a short abort timer fire at its
    /// delay while the far-future testharness timeout stays pending.
    pub fn run_timers(&mut self, budget: u32, now_ms: f64) -> usize {
        self.engine.pump_microtasks();
        let fired = self
            .engine
            .eval(&format!("String(globalThis.__runTimers({budget},{now_ms}))"))
            .ok()
            .and_then(|v| self.engine.value_to_string(&v).ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        self.engine.pump_microtasks();
        fired
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
        if d < 0.0 {
            None
        } else {
            Some(d)
        }
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
        let _ = self.engine.eval(&js);
        self.engine.pump_microtasks();
    }

    /// Reject the pending `fetch()` Promise `id` as a network error with `message`
    /// (a `TypeError`, per Fetch). For a deferred host's failed request.
    pub fn fail_fetch(&mut self, id: u64, message: &str) {
        let js = format!("globalThis.__fetchFail({},{});", id, js_str(message));
        let _ = self.engine.eval(&js);
        self.engine.pump_microtasks();
    }

    /// Early-settle the pending `fetch()` Promise `id` with a streaming response:
    /// status + headers from `meta` (its body is ignored), body delivered
    /// incrementally via [`push_chunk`](Self::push_chunk) then
    /// [`close_stream`](Self::close_stream). For a host that streams a response body
    /// as it arrives rather than buffering the whole thing.
    pub fn start_stream(&mut self, id: u64, meta: FetchOutcome) {
        let json = fetch::encode_outcome(&meta);
        let js = format!("globalThis.__fetchStartStream({},{});", id, js_str(&json));
        let _ = self.engine.eval(&js);
        self.engine.pump_microtasks();
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
        let _ = self.engine.eval(&format!("globalThis.__fetchPushChunk({},{});", id, lit));
        self.engine.pump_microtasks();
    }

    /// Close a streaming response started with [`start_stream`](Self::start_stream):
    /// the body's `ReadableStream` ends and pending reads resolve `done`.
    pub fn close_stream(&mut self, id: u64) {
        let _ = self.engine.eval(&format!("globalThis.__fetchClose({});", id));
        self.engine.pump_microtasks();
    }

    /// Error a streaming response started with [`start_stream`](Self::start_stream):
    /// the body's `ReadableStream` errors so pending/future reads reject with a
    /// `TypeError`. The response itself stays resolved (the failure is mid-body,
    /// e.g. a `Content-Encoding` decode error), so only body consumption rejects.
    pub fn error_stream(&mut self, id: u64) {
        let _ = self.engine.eval(&format!("globalThis.__fetchError({});", id));
        self.engine.pump_microtasks();
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
        let _ = self.engine.eval(&js);
        self.engine.pump_microtasks();
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
        let Ok(u) = url::Url::parse(url) else { return Ok(()) };
        self.host.borrow_mut().base_url = Some(u.to_string());
        let host = match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => String::new(),
        };
        let search = u.query().map(|q| format!("?{q}")).unwrap_or_default();
        let hash = u.fragment().map(|f| format!("#{f}")).unwrap_or_default();
        let js = format!(
            "globalThis.location = {{ href:{}, protocol:{}, host:{}, hostname:{}, \
             port:{}, pathname:{}, search:{}, hash:{}, origin:{} }};",
            js_str(u.as_str()),
            js_str(&format!("{}:", u.scheme())),
            js_str(&host),
            js_str(u.host_str().unwrap_or("")),
            js_str(&u.port().map(|p| p.to_string()).unwrap_or_default()),
            js_str(u.path()),
            js_str(&search),
            js_str(&hash),
            js_str(&u.origin().ascii_serialization()),
        );
        self.engine.eval(&js)?;
        self.engine.eval("globalThis.location.toString = function() { return this.href; };")?;
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

    // Event loop and EventTarget are pure-JS bootstraps over the global. Callbacks
    // live JS-side; the only Rust entry is `run_event_loop` (evals `__runTimers`).
    // ES5-style (function constructors, no arrows/classes) for the widest backend
    // coverage.
    engine.eval(EVENT_LOOP_BOOTSTRAP)?;
    engine.eval(EVENT_TARGET_BOOTSTRAP)?;

    // postMessage (async 'message' delivery to the global) + minimal location /
    // navigator stubs the harness reads at load. Depends on the event loop +
    // EventTarget above.
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

    // The `__reportResult` sink for the testharness results bridge. The completion
    // callback that calls it is registered later (after testharness loads).
    harness::install_report_sink(engine)?;
    Ok(())
}

/// `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` over a private
/// queue, drained by `__runTimers(budget)` in `(delay, insertion)` order.
const EVENT_LOOP_BOOTSTRAP: &str = r#"
(function() {
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
      rec.cb.call(this, event);
      event.__inPassive = false;
    }
  };
  EventTarget.prototype.dispatchEvent = function(event) {
    if (event.__initialized === false || event.__dispatch) {
      throw new DOMException("The event is not initialized or is being dispatched.", "InvalidStateError");
    }
    // window is a leaf target (no DOM tree): target phase only — capture- then
    // bubble-registered listeners on this target, with the dispatch flags set.
    event.__dispatch = true;
    event.target = this;
    event.srcElement = this;
    event.__stop = false;
    event.__stopImmediate = false;
    event.eventPhase = 2; // AT_TARGET
    this.__fire(event, 'c:' + event.type);
    if (!event.__stop) { this.__fire(event, 'b:' + event.type); }
    event.__dispatch = false;
    event.currentTarget = null;
    event.eventPhase = 0;
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

  var target = new EventTarget();
  globalThis.addEventListener = function(type, cb) { target.addEventListener(type, cb); };
  globalThis.removeEventListener = function(type, cb) { target.removeEventListener(type, cb); };
  globalThis.dispatchEvent = function(event) { return target.dispatchEvent(event); };
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

/// `postMessage` (async `message` delivery to the global) plus minimal `location`
/// and `navigator` stubs the harness touches at load. Async delivery rides the
/// event loop, so a `postMessage` only arrives after `run_event_loop`.
const SHELL_GLOBALS_BOOTSTRAP: &str = r#"
(function() {
  globalThis.postMessage = function(data) {
    var event = new Event('message');
    event.data = data;
    setTimeout(function() { dispatchEvent(event); }, 0);
  };
  // A top-level window: parent/top are itself, no opener. testharness walks
  // `while (w != w.parent)`, so a self-referential parent terminates the walk.
  globalThis.parent = globalThis;
  globalThis.top = globalThis;
  globalThis.opener = null;
  globalThis.location = {
    href: 'about:blank', protocol: 'about:', host: '', hostname: '',
    port: '', pathname: '', search: '', hash: '', origin: 'null',
    toString: function() { return this.href; }
  };
  globalThis.navigator = { userAgent: 'serval', platform: '', language: 'en-US' };

  // requestAnimationFrame as a 0-delay timer (no real frame clock yet).
  globalThis.requestAnimationFrame = function(cb) { return setTimeout(cb, 0); };
  globalThis.cancelAnimationFrame = function(id) { clearTimeout(id); };

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

    #[test]
    fn host_surface_on_boa() {
        host_surface_works::<script_engine_boa::BoaEngine>();
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

    /// The G3 GC tick, against any backend: a detached node script holds is
    /// pinned on mint and spared by `collect_garbage`; once its reflector is
    /// reported dead it is reaped. The drain→retire path is covered in the engine
    /// crates (`reflector_for_reports_death_after_gc`); here we simulate the death
    /// by clearing the pin set, to test pin-on-mint + collect's pin-aware reaping.
    fn gc_tick_collects_unpinned_nodes<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        let base = rt.host().borrow().dom.live_node_count();

        // `createElement` hands script a reflector → pin-on-mint pins the node.
        rt.eval("globalThis.d = document.createElement('div');").expect("create");
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
        let pass = results.iter().find(|r| r.name == "pass-test").expect("pass-test present");
        let fail = results.iter().find(|r| r.name == "fail-test").expect("fail-test present");
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
