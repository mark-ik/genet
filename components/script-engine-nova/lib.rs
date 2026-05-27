// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Nova backend for [`script_engine_api`] — the primary backend, **native-only**.
//!
//! Nova is 64-bit-bound (its data-oriented `Value` is `usize`-sized; wasm32 has
//! 32-bit `usize`), so this crate is gated to non-wasm targets and compiles to an
//! empty shell on wasm32. The wasm scripted tier uses `script-engine-boa`. The
//! native-data reflector rides on the patched `EmbedderObject` (serval-embedder
//! branch of the fork). Engine-native types stay confined here.

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use nova_vm::{
        ecmascript::{
            Agent, AgentOptions, ArgumentsList, Behaviour, BuiltinFunctionArgs, DefaultHostHooks,
            EmbedderObject, ExceptionType, GcAgent, InternalMethods, PropertyDescriptor,
            PropertyKey, RealmRoot, String as JsString, Value, create_builtin_function,
            parse_script, script_evaluation,
        },
        engine::{Bindable, GcScope, Global},
    };
    use script_engine_api::{
        CallCx, HostData, NativeFn, ReflectorData, ScriptEngine, ScriptEngineLive,
    };

    /// The call context handed to a native callback. Nova's `RegularFn` gives
    /// `(&mut Agent, this, ArgumentsList, GcScope<'gc, 'b>)`; `GcScope` is invariant
    /// in its first lifetime but covariant in the second, so the trampoline collapses
    /// the two onto one (`GcScope<'a, 'a>`) and this context carries a single
    /// lifetime, satisfying the engine-neutral one-lifetime [`CallCx`] GAT.
    pub struct NovaCallCx<'a> {
        agent: &'a mut Agent,
        gc: GcScope<'a, 'a>,
        args: Vec<Global<Value<'static>>>,
    }

    impl CallCx for NovaCallCx<'_> {
        type Value = Global<Value<'static>>;
        type Error = String;

        fn arg(&mut self, i: usize) -> Self::Value {
            match self.args.get(i) {
                Some(g) => {
                    let v = g.get(self.agent, self.gc.nogc()).unbind();
                    Global::new(self.agent, v)
                },
                None => Global::new(self.agent, Value::Undefined),
            }
        }

        fn host_data(&self) -> Option<HostData> {
            let agent: &Agent = self.agent;
            agent.current_realm(self.gc.nogc()).host_defined(agent)
        }

        fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
            let v = value.get(self.agent, self.gc.nogc()).unbind();
            match v.to_string(self.agent, self.gc.reborrow()) {
                Ok(s) => Ok(s.to_string_lossy(self.agent).into_owned()),
                Err(_) => Err("toString threw".to_string()),
            }
        }

        fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData> {
            match value.get(self.agent, self.gc.nogc()) {
                Value::EmbedderObject(eo) => Some(eo.embedder_data(self.agent)),
                _ => None,
            }
        }

        fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
            // We are mid-call, already inside the realm (the trampoline holds the
            // `Agent`), so build the `EmbedderObject` directly rather than via
            // `run_in_realm` (which can't nest). Mirrors the engine-level
            // `ScriptEngineLive::make_reflector`.
            let eo = EmbedderObject::create_with_data(self.agent, data);
            Ok(Global::new(self.agent, Value::EmbedderObject(eo).unbind()))
        }

        fn make_string(&mut self, s: &str) -> Result<Self::Value, Self::Error> {
            let js = JsString::from_str(self.agent, s, self.gc.nogc());
            Ok(Global::new(self.agent, Value::from(js).unbind()))
        }

        fn make_null(&mut self) -> Self::Value {
            Global::new(self.agent, Value::Null)
        }

        fn undefined(&mut self) -> Self::Value {
            Global::new(self.agent, Value::Undefined)
        }
    }

    /// Bare `fn`-pointer trampoline, monomorphized per `F` (Nova builtins capture
    /// nothing; state arrives via host-defined data + the reflector args). Roots the
    /// arguments, runs `F` against a [`NovaCallCx`], then maps the result back.
    fn nova_trampoline<'gc, F: NativeFn<NovaEngine>>(
        agent: &mut Agent,
        _this: Value,
        args: ArgumentsList,
        mut gc: GcScope<'gc, '_>,
    ) -> nova_vm::ecmascript::JsResult<'gc, Value<'gc>> {
        let rooted: Vec<Global<Value<'static>>> =
            (0..args.len()).map(|i| Global::new(agent, args.get(i).unbind())).collect();
        let result = {
            let mut cx = NovaCallCx { agent: &mut *agent, gc: gc.reborrow(), args: rooted };
            F::call(&mut cx)
        };
        match result {
            Ok(global) => {
                // `into_nogc` carries the full `'gc` lifetime, so the bound value can
                // be returned (unlike a `nogc()` borrow, which is local).
                let nogc = gc.into_nogc();
                Ok(global.get(agent, nogc).bind(nogc))
            },
            Err(msg) => Err(agent.throw_exception(ExceptionType::Error, msg, gc.into_nogc())),
        }
    }

    /// A Nova-backed scripting engine (native targets only).
    pub struct NovaEngine {
        agent: GcAgent,
        realm: RealmRoot,
    }

    impl ScriptEngine for NovaEngine {
        // Nova values are GC-scoped, so the held value type is a rooted `Global`.
        type Value = Global<Value<'static>>;
        type Error = String;
        type CallCx<'a> = NovaCallCx<'a>;

        fn new() -> Result<Self, Self::Error> {
            let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
            let realm = agent.create_default_realm();
            Ok(Self { agent, realm })
        }

        fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
            let src = source.to_string();
            let mut out: Result<Global<Value<'static>>, String> =
                Err("eval did not run".to_string());
            self.agent.run_in_realm(&self.realm, |agent, mut gc| {
                let realm = agent.current_realm(gc.nogc());
                let source_text = JsString::from_string(agent, src, gc.nogc());
                let script = match parse_script(agent, source_text, realm, false, None, gc.nogc()) {
                    Ok(script) => script,
                    Err(_) => {
                        out = Err("parse error".to_string());
                        return;
                    }
                };
                match script_evaluation(agent, script.unbind(), gc.reborrow()) {
                    Ok(value) => out = Ok(Global::new(agent, value.unbind())),
                    Err(_) => out = Err("evaluation threw".to_string()),
                }
            });
            out
        }

        fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
            let mut out = Err("value_to_string did not run".to_string());
            self.agent.run_in_realm(&self.realm, |agent, mut gc| {
                let v = value.get(agent, gc.nogc()).unbind();
                match v.to_string(agent, gc) {
                    Ok(s) => out = Ok(s.to_string_lossy(agent).into_owned()),
                    Err(_) => out = Err("toString threw".to_string()),
                }
            });
            out
        }

        fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), Self::Error> {
            let name = name.to_string();
            let mut out = Err("set_global did not run".to_string());
            self.agent.run_in_realm(&self.realm, |agent, mut gc| {
                let global = agent.current_realm(gc.nogc()).global_object(agent).unbind();
                let key = PropertyKey::from_str(agent, &name, gc.nogc()).unbind();
                let v = value.get(agent, gc.nogc()).unbind();
                let desc = PropertyDescriptor { value: Some(v), ..Default::default() };
                match global.internal_define_own_property(agent, key, desc, gc.reborrow()) {
                    Ok(_) => out = Ok(()),
                    Err(_) => out = Err("define_own_property threw".to_string()),
                }
            });
            out
        }

        fn set_host_data(&mut self, data: HostData) {
            // Nova's realm `[[HostDefined]]` is `Rc<dyn Any>` — the same shape as
            // `HostData`. Set once before running script (Nova panics on replace).
            self.realm.initialize_host_defined(&mut self.agent, data);
        }

        fn set_function<F: NativeFn<Self>>(
            &mut self,
            name: &str,
            length: usize,
        ) -> Result<(), Self::Error> {
            // Nova wants a `&'static str` function name; builtins are registered a
            // bounded number of times at setup, so leaking is acceptable here.
            let name: &'static str = Box::leak(name.to_string().into_boxed_str());
            let mut out = Err("set_function did not run".to_string());
            self.agent.run_in_realm(&self.realm, |agent, mut gc| {
                let func = create_builtin_function(
                    agent,
                    Behaviour::Regular(nova_trampoline::<F>),
                    BuiltinFunctionArgs::new(length as u32, name),
                    gc.nogc(),
                );
                let global = agent.current_realm(gc.nogc()).global_object(agent).unbind();
                let key = PropertyKey::from_str(agent, &name, gc.nogc()).unbind();
                let desc =
                    PropertyDescriptor { value: Some(func.unbind().into()), ..Default::default() };
                match global.internal_define_own_property(agent, key, desc, gc.reborrow()) {
                    Ok(_) => out = Ok(()),
                    Err(_) => out = Err("define_own_property threw".to_string()),
                }
            });
            out
        }
    }

    impl ScriptEngineLive for NovaEngine {
        fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
            let mut out = None;
            self.agent.run_in_realm(&self.realm, |agent, _gc| {
                let eo = EmbedderObject::create_with_data(agent, data);
                out = Some(Global::new(agent, Value::EmbedderObject(eo).unbind()));
            });
            Ok(out.expect("run_in_realm ran"))
        }

        fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData> {
            let mut out = None;
            self.agent.run_in_realm(&self.realm, |agent, gc| {
                if let Value::EmbedderObject(eo) = value.get(agent, gc.nogc()) {
                    out = Some(eo.embedder_data(agent));
                }
            });
            out
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reflector_round_trip_survives_gc() {
            let mut engine = NovaEngine::new().unwrap();
            let v = engine.make_reflector(0xDEAD_BEEF).unwrap();
            // Survives collection while reachable only via the rooted Global.
            engine.agent.gc();
            engine.agent.gc();
            assert_eq!(engine.reflector_data(&v), Some(0xDEAD_BEEF));

            // A non-reflector value yields None, and the value surface works.
            let n = engine.eval("1 + 2").unwrap();
            assert_eq!(engine.reflector_data(&n), None);
            assert_eq!(engine.value_to_string(&n).unwrap(), "3");
        }

        #[test]
        fn global_reflector_is_reachable_from_js() {
            let mut engine = NovaEngine::new().unwrap();
            let reflector = engine.make_reflector(0x1234).unwrap();
            engine.set_global("node", &reflector).unwrap();

            // JS reads the global; the value it yields carries the host data.
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

            let mut engine = NovaEngine::new().unwrap();
            engine.set_host_data(sink.clone());

            // setText(node, text): recover the node id off the reflector arg, read the
            // text, and record both into host data — reached via Nova [[HostDefined]],
            // not a thread_local.
            struct SetText;
            impl NativeFn<NovaEngine> for SetText {
                fn call(cx: &mut NovaCallCx<'_>) -> Result<Global<Value<'static>>, String> {
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
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::NovaEngine;
