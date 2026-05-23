//! Track A probe — validate the `ScriptEngine` / `ScriptEngineLive` design shape
//! (plan Parts 1 & 3 / Appendix A) against Boa 0.21, a real complete engine API.
//!
//! The whole point is that this **compiles and round-trips**. It proves:
//!   - the value-level VM surface (`eval`, value→string) abstracts cleanly;
//!   - a `NodeId`-carrying reflector with native data round-trips through the JS
//!     heap and is readable back in a native callback (Appendix A, Finding 2);
//!   - Boa's GC-safe explicit-captures pattern (`from_copy_closure_with_captures`
//!     with a `Trace` payload) lets JS mutate host Rust state (Finding 1);
//!   - the engine-native types (`JsValue`, `Context`, `NativeFunction`) stay
//!     *inside* the backend impl and never surface on the trait.

pub mod boa_backend;
pub mod dom;

pub use dom::{DomStore, NodeId};

/// Minimal VM surface — the shareable subset of the plan's `script-engine-api`.
/// Engine-native value/context/callback types deliberately do **not** appear here.
pub trait ScriptEngine: Sized {
    type Value;
    type Error: std::fmt::Debug;

    fn new() -> Result<Self, Self::Error>;

    /// Evaluate source in the global scope.
    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error>;

    /// Coerce a value to a Rust string (the one conversion the round-trip needs).
    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error>;
}

/// Live-DOM extension (plan Part 3 / Appendix A Finding 2): install a reflector
/// carrying a `NodeId` as native data, reachable from JS under `global_name`.
/// How the reflector is built (Boa `Class`, rquickjs `Class`, Nova `EmbedderObject`)
/// is the backend's concern; the trait only names the capability.
pub trait ScriptEngineLive: ScriptEngine {
    fn install_reflector(&mut self, global_name: &str, node: NodeId) -> Result<(), Self::Error>;
}
