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
