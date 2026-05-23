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
            AgentOptions, DefaultHostHooks, EmbedderObject, GcAgent, RealmRoot,
            String as JsString, Value, parse_script, script_evaluation,
        },
        engine::{Bindable, Global},
    };
    use script_engine_api::{ReflectorData, ScriptEngine, ScriptEngineLive};

    /// A Nova-backed scripting engine (native targets only).
    pub struct NovaEngine {
        agent: GcAgent,
        realm: RealmRoot,
    }

    impl ScriptEngine for NovaEngine {
        // Nova values are GC-scoped, so the held value type is a rooted `Global`.
        type Value = Global<Value<'static>>;
        type Error = String;

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
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::NovaEngine;
