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
use serval_scripted_dom::ScriptedDom;

mod dom;
mod harness;
mod selector;

pub use harness::TestResult;

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
    /// Per-subtest results collected from `testharness.js` via the completion
    /// callback (the results bridge). Populated by [`Runtime::run_testharness`].
    pub results: Vec<TestResult>,
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
  function schedule(cb, delay, repeat) {
    var id = nextId++;
    timers.push({ id: id, cb: cb, delay: +delay || 0, seq: timers.length, repeat: !!repeat });
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
  globalThis.__runTimers = function(budget) {
    var fired = 0;
    while (timers.length > 0 && fired < budget) {
      timers.sort(function(a, b) { return (a.delay - b.delay) || (a.seq - b.seq); });
      var t = timers.shift();
      fired++;
      if (t.repeat) { t.seq = nextId++; timers.push(t); }
      t.cb();
    }
    return fired;
  };
})();
"#;

/// `EventTarget` (with the global as one) + a minimal `Event`. Listeners are kept
/// per target keyed by type; `dispatchEvent` calls them synchronously over a copy.
const EVENT_TARGET_BOOTSTRAP: &str = r#"
(function() {
  function EventTarget() { this.__listeners = {}; }
  EventTarget.prototype.addEventListener = function(type, cb) {
    if (typeof cb !== 'function') return;
    if (!this.__listeners[type]) this.__listeners[type] = [];
    this.__listeners[type].push(cb);
  };
  EventTarget.prototype.removeEventListener = function(type, cb) {
    var l = this.__listeners[type];
    if (!l) return;
    var i = l.indexOf(cb);
    if (i !== -1) l.splice(i, 1);
  };
  EventTarget.prototype.dispatchEvent = function(event) {
    var l = this.__listeners[event.type];
    if (l) {
      var copy = l.slice();
      for (var i = 0; i < copy.length; i++) { copy[i].call(this, event); }
    }
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
  }
  Event.prototype.preventDefault = function() {
    if (this.cancelable) { this.__canceled = true; this.defaultPrevented = true; }
  };
  globalThis.Event = Event;

  var target = new EventTarget();
  globalThis.addEventListener = function(type, cb) { target.addEventListener(type, cb); };
  globalThis.removeEventListener = function(type, cb) { target.removeEventListener(type, cb); };
  globalThis.dispatchEvent = function(event) { return target.dispatchEvent(event); };
})();
"#;

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
    port: '', pathname: '', search: '', hash: '', origin: 'null'
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

    // Nova's regex engine rejects lone-surrogate ranges (`[\ud800-\udbff]`), which
    // testharness compiles in `sanitize_all_unpaired_surrogates` during completion
    // ("regex parse error: hexadecimal literal is not a Unicode scalar value"). This
    // is an upstream Nova conformance gap (JS regex is UTF-16; the engine wants
    // scalar values), not a binding-layer issue — the exact cross-backend
    // engine-axis delta the plan expects, with Boa (the oracle) passing. Un-ignore
    // when Nova handles surrogate escapes. Loading + DOM/event/microtask paths all
    // pass on Nova (the other tests); only the harness's completion sanitizer trips.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "Nova regex engine rejects surrogate ranges in testharness sanitize step"]
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
