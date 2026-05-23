// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Boa 0.21 backend for [`script_engine_api`]. Pure Rust → the wasm32 scripting
//! backend, and the native conformance oracle. Engine-native types (`JsValue`,
//! `Context`, the reflector `Class`) stay confined to this crate.

use boa_engine::{
    Context, JsData, JsError, JsNativeError, JsObject, JsResult, JsValue, Source,
    class::{Class, ClassBuilder},
};
use boa_gc::{Finalize, Trace};
use script_engine_api::{ReflectorData, ScriptEngine, ScriptEngineLive};

/// Native-data reflector (Appendix A Finding 2): a JS object carrying only the host
/// [`ReflectorData`]. The DOM node's data lives in the host arena, never the JS heap.
#[derive(Debug, Trace, Finalize, JsData)]
struct Reflector {
    #[unsafe_ignore_trace]
    data: ReflectorData,
}

impl Class for Reflector {
    const NAME: &'static str = "Reflector";
    const LENGTH: usize = 0;

    fn data_constructor(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("Reflectors are host-created, not `new`-able")
            .into())
    }

    fn init(_builder: &mut ClassBuilder<'_>) -> JsResult<()> {
        Ok(())
    }
}

/// A Boa-backed scripting engine.
pub struct BoaEngine {
    ctx: Context,
}

impl ScriptEngine for BoaEngine {
    type Value = JsValue;
    type Error = JsError;

    fn new() -> Result<Self, Self::Error> {
        let mut ctx = Context::default();
        ctx.register_global_class::<Reflector>()?;
        Ok(Self { ctx })
    }

    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
        self.ctx.eval(Source::from_bytes(source))
    }

    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
        Ok(value.to_string(&mut self.ctx)?.to_std_string_escaped())
    }
}

impl ScriptEngineLive for BoaEngine {
    fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
        let obj: JsObject = Reflector::from_data(Reflector { data }, &mut self.ctx)?;
        Ok(obj.into())
    }

    fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData> {
        value
            .as_object()
            .and_then(|o| o.downcast_ref::<Reflector>().map(|r| r.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflector_round_trip() {
        let mut engine = BoaEngine::new().unwrap();
        let v = engine.make_reflector(0xDEAD_BEEF).unwrap();
        assert_eq!(engine.reflector_data(&v), Some(0xDEAD_BEEF));
        // A non-reflector value yields None.
        let other = engine.eval("({})").unwrap();
        assert_eq!(engine.reflector_data(&other), None);
    }

    #[test]
    fn value_surface() {
        let mut engine = BoaEngine::new().unwrap();
        let v = engine.eval("'a' + (1 + 2)").unwrap();
        assert_eq!(engine.value_to_string(&v).unwrap(), "a3");
    }
}
