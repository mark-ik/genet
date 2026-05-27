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
//! Not yet: the DOM **read** surface (`getAttribute`, `textContent` getter,
//! `tagName`), which needs a string-minting primitive on `CallCx`; Promise
//! microtask draining (needs an engine `pump_microtasks` primitive); and real
//! timer delays (the loop fires in `(delay, insertion)` order, cooperatively).
//! Those are the next rungs toward loading `testharness.js`. See
//! `docs/2026-05-26_pluggable_engines_testharness_plan.md`.

use std::cell::RefCell;
use std::rc::Rc;

use script_engine_api::{CallCx, NativeFn, ScriptEngine};
use serval_scripted_dom::ScriptedDom;

mod dom;

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

    /// Drive the event loop: fire pending timers in `(delay, insertion-order)`
    /// order, up to `budget` firings (the cap bounds `setInterval`, which
    /// re-enqueues itself). Cooperative: delays order tasks, they do not wait.
    /// Returns when the queue drains or the budget is spent.
    pub fn run_event_loop(&mut self, budget: u32) -> Result<(), E::Error> {
        self.engine.eval(&format!("globalThis.__runTimers({budget})"))?;
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

    // `document` + the Node/Element construction surface, bound to the `ScriptedDom`
    // in host state. Native sinks mutate the arena; a JS bootstrap wraps reflectors
    // into ergonomic node objects.
    dom::install_dom_surface(engine)?;
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

    #[test]
    fn host_surface_on_boa() {
        host_surface_works::<script_engine_boa::BoaEngine>();
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
