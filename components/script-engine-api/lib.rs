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

/// JS-opaque native data a reflector carries, bridging a JS object back to the host
/// DOM. Packs a serval `NodeId` (the DOM crate owns the `NodeId` ↔ `u64` mapping;
/// this crate stays DOM-neutral).
pub type ReflectorData = u64;

/// A JavaScript VM instance. Engine-native value/context/callback types deliberately
/// do **not** appear on this trait — they live inside each backend crate.
pub trait ScriptEngine: Sized {
    /// A handle to a JS value. For engines whose values are GC-scoped (Nova), this is
    /// a *rooted* handle so it can be held across calls; for others it is the native
    /// value type (Boa `JsValue`).
    type Value;
    type Error: core::fmt::Debug;

    /// Construct a fresh engine with an empty global scope.
    fn new() -> Result<Self, Self::Error>;

    /// Evaluate `source` in the global scope, returning its completion value.
    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error>;

    /// Coerce a value to a Rust string (`ToString`).
    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error>;
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
