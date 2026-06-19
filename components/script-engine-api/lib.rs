// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Engine-neutral scripting backend contract for serval's scripted tier.
//!
//! Lifted from the Track A (Boa) and Track B (Nova) validation probes — see
//! `docs/2026-05-20_serval_script_engine_plan.md`. The two findings the probes
//! proved are baked into the shape here:
//!
//! - The value surface (`eval`, value→string) and a native-data reflector are
//!   expressible with engine-native types (`JsValue`/`Context`, `Value`/`Agent`,
//!   `EmbedderObject`) fully confined to the backend crates.
//! - Reflector data is recovered by *value extraction* ([`reflector_data`]), not by
//!   invoking a JS method — because Nova's public API can't call a held function, so
//!   the cross-engine bridge has to read native data off a value directly.
//!
//! Backend selection is per-target: **Nova** native (primary), **Boa** on wasm32
//! (Nova is 64-bit-bound, Appendix B). Engines implement these traits; consumers
//! (`serval-scripted-dom`) drive them.

use std::any::Any;
use std::rc::Rc;

/// JS-opaque native data a reflector carries, bridging a JS object back to the host
/// DOM. Packs a serval `NodeId` (the DOM crate owns the `NodeId` ↔ `u64` mapping;
/// this crate stays DOM-neutral).
pub type ReflectorData = u64;

/// Opaque token for a pending host-created ("deferred") promise. Neutral, like
/// [`ReflectorData`]: the engine keeps the real resolve/reject machinery in its own
/// side table keyed by this token, so the neutral host layer can hold and pass the
/// token across the boundary without naming an engine type.
///
/// This is the async-host bridge. A native callback (`fetch`, `callModel`) mints a
/// pending promise with [`CallCx::new_host_promise`], returns it so JS can `await`,
/// and stashes the token in host state; when the backing Rust future completes, the
/// host calls [`ScriptEngine::settle_host_promise`] and then drains the reaction
/// jobs with [`ScriptEngine::pump_microtasks`]. Without it the trait can only *drain*
/// the job queue ([`pump_microtasks`]), never *create* a promise the host resolves.
///
/// [`pump_microtasks`]: ScriptEngine::pump_microtasks
pub type PromiseToken = u64;

/// A bound on how much work one [`ScriptEngine::pump`] call may do, as a runaway
/// guard. The unit is **coarse and backend-defined** (Nova counts reaction *jobs*, a
/// fuel-metered VM counts VM *steps*), so `Steps(n)` is a "stop a script that never
/// settles" cap, not precise accounting. `Unbounded` drains to quiescence (the
/// classic microtask checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    /// Drain the queue fully (a job storm can hang the caller; use only on trusted
    /// scripts or after their work is known-bounded).
    Unbounded,
    /// Run at most this many coarse steps, then return control even if work remains.
    Steps(u64),
}

/// The result of a [`ScriptEngine::pump`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpOutcome {
    /// The queue drained; no pending microtask work remains.
    Quiescent,
    /// The [`Budget`] was exhausted with work still pending. Call `pump` again to make
    /// more progress (a runaway script keeps returning this, which is how the caller
    /// detects and abandons it).
    Pending,
}

/// Host state shared with native callbacks. A refcounted `Any` the host downcasts
/// (typically `Rc<RefCell<…>>` over the live DOM). Engine-neutral: each backend
/// stashes it in its own host-defined-data slot (Nova realm `[[HostDefined]]`, Boa
/// `Context` host data), never a `thread_local`. The host reaches it inside a
/// callback via [`CallCx::host_data`].
pub type HostData = Rc<dyn Any>;

/// A JavaScript VM instance. Engine-native value/context/callback types deliberately
/// do **not** appear on this trait — they live inside each backend crate.
pub trait ScriptEngine: Sized {
    /// A handle to a JS value. For engines whose values are GC-scoped (Nova), this is
    /// a *rooted* handle so it can be held across calls; for others it is the native
    /// value type (Boa `JsValue`).
    type Value;
    type Error: core::fmt::Debug;

    /// The per-call context a native callback receives ([`CallCx`]). It is a
    /// separate surface from `&mut Self` because, inside a callback, the VM is
    /// mid-execution (Nova: holding an `Agent` + `GcScope`), so the full engine
    /// API is not reachable. Carries one lifetime; backends with multi-lifetime
    /// internals (Nova's `GcScope`) collapse them onto it.
    type CallCx<'a>: CallCx<Value = Self::Value, Error = Self::Error>
    where
        Self: 'a;

    /// Construct a fresh engine with an empty global scope.
    fn new() -> Result<Self, Self::Error>;

    /// Evaluate `source` in the global scope, returning its completion value.
    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error>;

    /// Evaluate `source` as an ECMAScript **module** (module scope, strict mode,
    /// `import` / `export`), driving its load → link → evaluate to completion.
    ///
    /// `base_url` is the entry module's own URL (its `import` specifiers resolve
    /// against it). `resolve` is the host module resolver: given an import
    /// `(specifier, referrer_url)` it returns the imported module's
    /// `(resolved_url, source)`, or `None` if it cannot be resolved/fetched — the
    /// seam through which the host (which owns the fetcher) supplies dependency
    /// source on demand. The engine keys its module cache on `resolved_url`, so a
    /// diamond or cycle resolves each module once.
    ///
    /// Returns `Ok(Some(value))` on success, `Err` if the module (or a dependency)
    /// throws or fails to load, and `Ok(None)` when this backend does not support
    /// module evaluation — the default, so a backend without module support (or one
    /// that has not wired it yet) degrades gracefully rather than failing to compile.
    fn eval_module(
        &mut self,
        _source: &str,
        _base_url: &str,
        _resolve: &mut dyn FnMut(&str, &str) -> Option<(String, String)>,
    ) -> Result<Option<Self::Value>, Self::Error> {
        Ok(None)
    }

    /// Like [`eval`](Self::eval), but bounded: a [`Budget::Steps`] cap stops a
    /// runaway script (e.g. `while true do end`) after roughly that many
    /// coarse VM steps and returns an error instead of hanging. The cap is on
    /// the *main* evaluation, complementing [`pump`](Self::pump)'s cap on
    /// microtask jobs.
    ///
    /// The default ignores the budget and runs [`eval`](Self::eval) unbounded
    /// — correct for backends whose VM cannot be step-metered (Boa). A
    /// fuel-metered backend (piccolo) overrides this; an untrusted-script host
    /// should prefer this method with a [`Budget::Steps`] bound on those
    /// backends.
    fn eval_bounded(
        &mut self,
        source: &str,
        _budget: Budget,
    ) -> Result<Self::Value, Self::Error> {
        self.eval(source)
    }

    /// Coerce a value to a Rust string (`ToString`).
    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error>;

    /// Define `name` on the global object with `value`. The primitive the host
    /// (runtime layer) installs globals from: reflectors (`node`), and later the
    /// browser host objects (`self`, `document`, `addEventListener`, …). Native
    /// callbacks ride a separate primitive (see the plan's `new_function`), because
    /// how a callback reaches host state is engine-specific.
    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), Self::Error>;

    /// Stash host state reachable from native callbacks via [`CallCx::host_data`].
    /// Replaces an existing slot of the same shape; the host is expected to set it
    /// once before running script.
    fn set_host_data(&mut self, data: HostData);

    /// Install `name` on the global as a native function backed by `F`, with arity
    /// `length`. `F` is a zero-sized type, not a closure: both backends register a
    /// bare `fn` pointer (Nova `RegularFn`, Boa `NativeFunctionPointer`), so the
    /// callback is monomorphized per `F` (a distinct trampoline) and captures
    /// nothing. State reaches the callback through [`CallCx::host_data`] and the
    /// reflector arguments, not captures.
    fn set_function<F: NativeFn<Self>>(
        &mut self,
        name: &str,
        length: usize,
    ) -> Result<(), Self::Error>;

    /// Run pending microtasks (Promise reaction jobs) up to `budget`, including jobs
    /// enqueued while running. Returns [`PumpOutcome::Quiescent`] if the queue drained
    /// or [`PumpOutcome::Pending`] if `budget` ran out with work remaining. The host
    /// calls this at task boundaries (after the initial script, between timer tasks) so
    /// Promise continuations resolve; passing a [`Budget::Steps`] bound lets it cap a
    /// runaway script instead of hanging. Errors thrown by a job are swallowed (an
    /// unhandled rejection is not the host's failure).
    ///
    /// Backends honor the budget to the degree their job machinery allows: a
    /// fuel-metered VM bounds by VM step, Nova bounds by job count, and Boa drains
    /// fully (its `SimpleJobExecutor` has no sub-drain) and so always returns
    /// `Quiescent`. The contract a caller can rely on everywhere is "`Quiescent` means
    /// done"; only `Steps`-honoring backends return `Pending`.
    fn pump(&mut self, budget: Budget) -> PumpOutcome;

    /// Drain pending microtasks to quiescence: [`pump`] with [`Budget::Unbounded`]. The
    /// common case at a task boundary; callers that must not hang on a runaway script
    /// use [`pump`] with a [`Budget::Steps`] bound and loop on [`PumpOutcome::Pending`].
    ///
    /// [`pump`]: ScriptEngine::pump
    fn pump_microtasks(&mut self) {
        let _ = self.pump(Budget::Unbounded);
    }

    /// Mint a pending ("deferred") promise at the engine level: the between-tasks
    /// mirror of [`CallCx::new_host_promise`], for a promise the host installs (as a
    /// global, say) before running script. Returns the JS promise value and a
    /// [`PromiseToken`] to settle it later with [`settle_host_promise`].
    ///
    /// [`settle_host_promise`]: ScriptEngine::settle_host_promise
    fn new_host_promise(&mut self) -> Result<(Self::Value, PromiseToken), Self::Error>;

    /// Settle a pending host promise: resolve it with `Ok(value)` or reject it with
    /// `Err(error)`. This enqueues the promise's reaction jobs but does not run them;
    /// the host drains them with [`pump_microtasks`]. The token is consumed: a token
    /// not in the table (already settled, or never minted here) is a silent no-op, so
    /// double-settle is safe. The async-host bridge's resolving half — call it when
    /// the Rust future behind a [`new_host_promise`] completes.
    ///
    /// [`pump_microtasks`]: ScriptEngine::pump_microtasks
    /// [`new_host_promise`]: ScriptEngine::new_host_promise
    fn settle_host_promise(
        &mut self,
        token: PromiseToken,
        outcome: Result<&Self::Value, &Self::Value>,
    ) -> Result<(), Self::Error>;

    /// Report the reflectors whose JS objects have been collected since the last
    /// call, returning their [`ReflectorData`] and forgetting them from the
    /// canonical-reflector cache. The host drains this at the same cadence as
    /// [`pump_microtasks`] and unpins each returned id from its reflector-pin
    /// table, so a detached ("orphaned") node whose last JS reference has died
    /// becomes collectable (the prerequisite for the gc-arena refit, G3).
    ///
    /// The **default is the fallback / epoch-pin mode**: it reports nothing, so
    /// reflector-held ids stay pinned until document teardown. That is today's
    /// behavior and the correct mode for a backend whose GC cannot report object
    /// deaths; navigation-bounded documents lose nothing by it. A backend that
    /// *can* observe deaths (a weakly-held canonical cache) overrides this.
    ///
    /// [`pump_microtasks`]: ScriptEngine::pump_microtasks
    fn drain_dead_reflectors(&mut self) -> Vec<ReflectorData> {
        Vec::new()
    }

    /// Force a full collection of the engine heap, so that
    /// [`drain_dead_reflectors`](Self::drain_dead_reflectors) can observe the deaths of
    /// reflector wrappers script no longer references. The runtime calls this at the GC
    /// tick (`Runtime::collect_garbage`) — a deliberate, not-per-microtask cadence —
    /// immediately before draining. The default is a no-op: an engine in the epoch-pin
    /// fallback (whose drain reports nothing), or one whose GC cannot be forced, loses
    /// nothing by it. A backend with real death-reporting overrides this to drive its
    /// collector, so a just-orphaned node is reaped that same tick (the gc-arena soak's
    /// frame-cadence contract).
    fn force_gc(&mut self) {}
}

/// A native (Rust) callback exposed to JS, implemented by a zero-sized type so the
/// backend can monomorphize a captures-free trampoline per callback. Written once
/// against [`CallCx`]; the same `impl` drives every backend.
pub trait NativeFn<E: ScriptEngine> {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error>;
}

/// What a native callback can do while the VM is mid-call: read its arguments,
/// reach host state, convert and build values. Distinct from [`ScriptEngine`]
/// because the full engine is not reachable from inside a call.
pub trait CallCx {
    type Value;
    type Error;

    /// The `i`th argument, or undefined if absent.
    fn arg(&mut self, i: usize) -> Self::Value;

    /// Host state set by [`ScriptEngine::set_host_data`], or `None` if unset.
    fn host_data(&self) -> Option<HostData>;

    /// Coerce a value to a Rust string (`ToString`).
    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error>;

    /// Recover reflector native data (the JS → host bridge), or `None` if `value`
    /// is not a reflector.
    fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData>;

    /// Mint a reflector carrying `data`, the in-callback mirror of
    /// [`ScriptEngineLive::make_reflector`]. A native callback that *returns* a host
    /// object (e.g. `document.createElement` handing JS a new `Node`) needs this:
    /// `reflector_data` recovers an incoming node, this mints an outgoing one. Both
    /// backends can build a reflector mid-call from the context they already hold
    /// (Nova's `Agent`, Boa's `Context`), so it sits on the base context rather than
    /// gating DOM callbacks behind a separate live-context trait.
    fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error>;

    /// The **canonical** reflector for `data`: minted on first call and cached, so
    /// repeated calls for the same node return a reflector backed by the *same* JS
    /// object (`document.body === document.body`). Unlike [`make_reflector`], which
    /// mints a fresh object every time.
    ///
    /// The cache is necessarily **engine-side** (a cached reflector is an
    /// engine-native value — a Nova `Global`, a Boa `JsValue` — so it cannot live in
    /// neutral [`HostData`] without re-coupling the host layer to an engine). It
    /// lives in the same host-defined slot the engine already owns.
    fn reflector_for(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error>;

    /// Mint a JS string value. The read-surface mirror of [`value_to_string`]: a
    /// callback returning text (`getAttribute`, `tagName`, the `textContent` getter)
    /// builds it with this.
    ///
    /// [`value_to_string`]: CallCx::value_to_string
    fn make_string(&mut self, s: &str) -> Result<Self::Value, Self::Error>;

    /// The `null` value, distinct from [`undefined`]. A miss returns `null`
    /// (`getElementById`, `getAttribute` on an absent attribute), per the DOM.
    ///
    /// [`undefined`]: CallCx::undefined
    fn make_null(&mut self) -> Self::Value;

    /// The `undefined` value (the usual callback return).
    fn undefined(&mut self) -> Self::Value;

    /// Mint a pending ("deferred") promise mid-call. Returns the JS promise value
    /// (the native callback returns it so JS can `await`) and a [`PromiseToken`] the
    /// host stashes (in host data) to settle the promise once the backing Rust future
    /// completes, via [`ScriptEngine::settle_host_promise`]. The in-callback mirror of
    /// [`ScriptEngine::new_host_promise`], and the primitive an async host call
    /// (`fetch`, `callModel`) is built from: return a promise now, resolve it later.
    fn new_host_promise(&mut self) -> Result<(Self::Value, PromiseToken), Self::Error>;
}

/// Live-DOM extension: native-data reflectors (plan Part 3 / Appendix A Finding 2).
/// The reflector is the bridge object — a JS-visible value carrying a host
/// [`ReflectorData`] the host can recover later.
pub trait ScriptEngineLive: ScriptEngine {
    /// Create a JS value carrying `data` as JS-opaque native data. The host hands
    /// this to JS (as a global, a property, a callback argument, …).
    fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error>;

    /// Recover the native data from a reflector value (the JS → host bridge).
    /// `None` if `value` is not a reflector created by [`make_reflector`].
    fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData>;
}
