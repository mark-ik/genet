// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Boa 0.21 backend for [`script_engine_api`]. Pure Rust → the wasm32 scripting
//! backend, and the native conformance oracle. Engine-native types (`JsValue`,
//! `Context`, the reflector `Class`) stay confined to this crate.

use std::cell::RefCell;
use std::collections::HashMap;

use boa_engine::{
    Context, JsData, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    Source,
    class::{Class, ClassBuilder},
};
use boa_gc::{Finalize, GcRefCell, Trace};
use script_engine_api::{CallCx, HostData, NativeFn, ReflectorData, ScriptEngine, ScriptEngineLive};

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

/// Host-data slot stored in Boa's `Context` host-defined data. Holds the
/// engine-neutral [`HostData`] (the `Rc<dyn Any>` is not traced — it holds host
/// state, never JS values) plus the canonical-reflector cache (`NodeId →
/// reflector`), which **is** traced: the cached `JsValue`s are live JS objects and
/// must survive collection. Both engine-side, never in neutral host state.
#[derive(Trace, Finalize, JsData)]
struct HostCell {
    #[unsafe_ignore_trace]
    data: RefCell<Option<HostData>>,
    reflectors: GcRefCell<HashMap<u64, JsValue>>,
}

impl HostCell {
    fn new() -> Self {
        Self { data: RefCell::new(None), reflectors: GcRefCell::new(HashMap::new()) }
    }
}

/// A Boa-backed scripting engine.
pub struct BoaEngine {
    ctx: Context,
}

/// The call context handed to a native callback. Boa's callback gives
/// `(this, &[JsValue], &mut Context)`, so one lifetime suffices.
pub struct BoaCallCx<'a> {
    ctx: &'a mut Context,
    args: &'a [JsValue],
}

impl CallCx for BoaCallCx<'_> {
    type Value = JsValue;
    type Error = JsError;

    fn arg(&mut self, i: usize) -> JsValue {
        self.args.get(i).cloned().unwrap_or_default()
    }

    fn host_data(&self) -> Option<HostData> {
        self.ctx.get_data::<HostCell>().and_then(|c| c.data.borrow().clone())
    }

    fn reflector_for(&mut self, data: ReflectorData) -> Result<JsValue, JsError> {
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            if let Some(v) = cell.reflectors.borrow().get(&data) {
                return Ok(v.clone());
            }
        }
        let v = self.make_reflector(data)?;
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            cell.reflectors.borrow_mut().insert(data, v.clone());
        }
        Ok(v)
    }

    fn value_to_string(&mut self, value: &JsValue) -> Result<String, JsError> {
        Ok(value.to_string(self.ctx)?.to_std_string_escaped())
    }

    fn reflector_data(&mut self, value: &JsValue) -> Option<ReflectorData> {
        value
            .as_object()
            .and_then(|o| o.downcast_ref::<Reflector>().map(|r| r.data))
    }

    fn make_reflector(&mut self, data: ReflectorData) -> Result<JsValue, JsError> {
        // The `Reflector` class is registered at engine construction, so building one
        // from the held `Context` is the in-callback mirror of the engine-level
        // `ScriptEngineLive::make_reflector`.
        let obj: JsObject = Reflector::from_data(Reflector { data }, self.ctx)?;
        Ok(obj.into())
    }

    fn make_string(&mut self, s: &str) -> Result<JsValue, JsError> {
        Ok(JsValue::from(JsString::from(s)))
    }

    fn make_null(&mut self) -> JsValue {
        JsValue::null()
    }

    fn undefined(&mut self) -> JsValue {
        JsValue::undefined()
    }
}

impl ScriptEngine for BoaEngine {
    type Value = JsValue;
    type Error = JsError;
    type CallCx<'a> = BoaCallCx<'a>;

    fn new() -> Result<Self, Self::Error> {
        let mut ctx = Context::default();
        ctx.register_global_class::<Reflector>()?;
        ctx.insert_data(HostCell::new());
        Ok(Self { ctx })
    }

    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
        self.ctx.eval(Source::from_bytes(source))
    }

    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
        Ok(value.to_string(&mut self.ctx)?.to_std_string_escaped())
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), Self::Error> {
        let global = self.ctx.global_object();
        global.set(JsString::from(name), value.clone(), false, &mut self.ctx)?;
        Ok(())
    }

    fn set_host_data(&mut self, data: HostData) {
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            *cell.data.borrow_mut() = Some(data);
        }
    }

    fn set_function<F: NativeFn<Self>>(
        &mut self,
        name: &str,
        length: usize,
    ) -> Result<(), Self::Error> {
        // A captures-free trampoline, monomorphized per `F` to a distinct fn
        // pointer — Boa's cheap native-function path, matching Nova's.
        fn trampoline<F: NativeFn<BoaEngine>>(
            _this: &JsValue,
            args: &[JsValue],
            ctx: &mut Context,
        ) -> JsResult<JsValue> {
            let mut cx = BoaCallCx { ctx, args };
            F::call(&mut cx)
        }
        self.ctx.register_global_callable(
            JsString::from(name),
            length,
            NativeFunction::from_fn_ptr(trampoline::<F>),
        )
    }

    fn pump_microtasks(&mut self) {
        // Boa's default executor (SimpleJobExecutor) runs the promise-job queue to
        // completion, including jobs enqueued while running.
        let _ = self.ctx.run_jobs();
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

    #[test]
    fn global_reflector_is_reachable_from_js() {
        let mut engine = BoaEngine::new().unwrap();
        let reflector = engine.make_reflector(0x1234).unwrap();
        engine.set_global("node", &reflector).unwrap();

        let from_js = engine.eval("node").unwrap();
        assert_eq!(engine.reflector_data(&from_js), Some(0x1234));
    }

    #[test]
    fn native_fn_reaches_host_data_and_reflector_arg() {
        use std::cell::RefCell;
        use std::rc::Rc;

        // The host sink a `setText`-style callback writes to (stands in for the DOM).
        type Sink = RefCell<Vec<(ReflectorData, String)>>;
        let sink: Rc<Sink> = Rc::new(RefCell::new(Vec::new()));

        let mut engine = BoaEngine::new().unwrap();
        engine.set_host_data(sink.clone());

        // setText(node, text): recover the node id off the reflector arg, read the
        // text, and record both into host data — the JS→host write path.
        struct SetText;
        impl NativeFn<BoaEngine> for SetText {
            fn call(cx: &mut BoaCallCx<'_>) -> JsResult<JsValue> {
                let node = cx.arg(0);
                let text = cx.arg(1);
                let id = cx.reflector_data(&node).unwrap_or(0);
                let text = cx.value_to_string(&text)?;
                if let Some(data) = cx.host_data() {
                    if let Some(sink) = data.downcast_ref::<Sink>() {
                        sink.borrow_mut().push((id, text));
                    }
                }
                Ok(cx.undefined())
            }
        }
        engine.set_function::<SetText>("setText", 2).unwrap();

        let node = engine.make_reflector(0x42).unwrap();
        engine.set_global("node", &node).unwrap();
        engine.eval("setText(node, 'hello from JS')").unwrap();

        assert_eq!(*sink.borrow(), vec![(0x42, "hello from JS".to_string())]);
    }
}
