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
    use std::any::Any;
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::rc::Rc;

    use nova_vm::{
        ecmascript::{
            Agent, AgentOptions, ArgumentsList, Behaviour, BuiltinFunctionArgs, EmbedderObject,
            ExceptionType, GcAgent, HostHooks, InternalMethods, Job, PropertyDescriptor,
            PropertyKey, RealmRoot, String as JsString, Value, create_builtin_function,
            parse_script, script_evaluation,
        },
        engine::{Bindable, GcScope, Global},
    };

    /// Host hooks that capture promise/generic/timeout jobs into a shared queue the
    /// engine drains in `pump_microtasks`. Nova hands jobs to the host via these
    /// hooks (which take only `&self`), so the queue lives here and is shared with
    /// the engine by `Rc`. Jobs are `'static` (they own rooted handles), so queuing
    /// them across GC is safe.
    struct ServalHostHooks {
        jobs: Rc<RefCell<VecDeque<Job>>>,
    }

    // `HostHooks: Debug`, but `Job` is not `Debug`, so report the queue length only.
    impl std::fmt::Debug for ServalHostHooks {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ServalHostHooks").field("queued", &self.jobs.borrow().len()).finish()
        }
    }

    impl HostHooks for ServalHostHooks {
        fn enqueue_generic_job(&self, job: Job) {
            self.jobs.borrow_mut().push_back(job);
        }
        fn enqueue_promise_job(&self, job: Job) {
            self.jobs.borrow_mut().push_back(job);
        }
        fn enqueue_timeout_job(&self, job: Job, _milliseconds: u64) {
            self.jobs.borrow_mut().push_back(job);
        }
        fn get_host_data(&self) -> &dyn Any {
            // Unused: serval reaches host state through the realm `[[HostDefined]]`
            // slot, not this hook.
            &()
        }
    }
    use script_engine_api::{
        CallCx, HostData, NativeFn, ReflectorData, ScriptEngine, ScriptEngineLive,
    };

    /// Nova's realm `[[HostDefined]]` slot: the neutral [`HostData`] (the DOM, set by
    /// the host) plus the canonical-reflector cache. The cached `Global`s are
    /// permanent roots in `agent.heap.globals`, so they survive collection without
    /// the slot itself being traced. Both engine-side, off the neutral wall.
    struct NovaHostSlot {
        neutral: RefCell<Option<HostData>>,
        reflectors: RefCell<HashMap<u64, Global<Value<'static>>>>,
    }

    impl NovaHostSlot {
        fn new() -> Self {
            Self { neutral: RefCell::new(None), reflectors: RefCell::new(HashMap::new()) }
        }
    }

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
            let hd = agent.current_realm(self.gc.nogc()).host_defined(agent)?;
            let slot = hd.downcast_ref::<NovaHostSlot>()?;
            let neutral = slot.neutral.borrow().clone();
            neutral
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

        fn reflector_for(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
            // Cache hit: return a fresh `Global` to the *same* heap object, so the
            // returned reflectors compare `===`.
            {
                let agent: &Agent = self.agent;
                if let Some(hd) = agent.current_realm(self.gc.nogc()).host_defined(agent) {
                    if let Some(slot) = hd.downcast_ref::<NovaHostSlot>() {
                        if let Some(g) = slot.reflectors.borrow().get(&data) {
                            let v = g.get(self.agent, self.gc.nogc()).unbind();
                            return Ok(Global::new(self.agent, v));
                        }
                    }
                }
            }
            // Miss: mint once, cache a `Global` to it, return another to the same object.
            let canonical = self.make_reflector(data)?;
            {
                let v = canonical.get(self.agent, self.gc.nogc()).unbind();
                let cached = Global::new(self.agent, v);
                let agent: &Agent = self.agent;
                if let Some(hd) = agent.current_realm(self.gc.nogc()).host_defined(agent) {
                    if let Some(slot) = hd.downcast_ref::<NovaHostSlot>() {
                        slot.reflectors.borrow_mut().insert(data, cached);
                    }
                }
            }
            Ok(canonical)
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
        jobs: Rc<RefCell<VecDeque<Job>>>,
    }

    impl ScriptEngine for NovaEngine {
        // Nova values are GC-scoped, so the held value type is a rooted `Global`.
        type Value = Global<Value<'static>>;
        type Error = String;
        type CallCx<'a> = NovaCallCx<'a>;

        fn new() -> Result<Self, Self::Error> {
            // The hooks must be `&'static`; leak one per engine (a few words +
            // shared queue handle). Acceptable for the engine lifetime; the proper
            // fix is a non-'static hooks API upstream.
            let jobs: Rc<RefCell<VecDeque<Job>>> = Rc::new(RefCell::new(VecDeque::new()));
            let hooks: &'static ServalHostHooks =
                Box::leak(Box::new(ServalHostHooks { jobs: jobs.clone() }));
            let mut agent = GcAgent::new(AgentOptions::default(), hooks);
            let realm = agent.create_default_realm();
            // The realm owns the host slot (neutral DOM + reflector cache) for its
            // whole life; `set_host_data` later fills the neutral half.
            realm.initialize_host_defined(&mut agent, Rc::new(NovaHostSlot::new()));
            Ok(Self { agent, realm, jobs })
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
            self.agent.run_in_realm(&self.realm, |agent, gc| {
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
            // Fill the neutral half of the realm's host slot (initialized in `new`).
            self.agent.run_in_realm(&self.realm, |agent, gc| {
                if let Some(hd) = agent.current_realm(gc.nogc()).host_defined(agent) {
                    if let Some(slot) = hd.downcast_ref::<NovaHostSlot>() {
                        *slot.neutral.borrow_mut() = Some(data);
                    }
                }
            });
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

        fn pump_microtasks(&mut self) {
            // Drain to quiescence: a job may enqueue more (chained `.then`), so loop
            // until the queue empties. Each job consumes itself in `run`.
            loop {
                let Some(job) = self.jobs.borrow_mut().pop_front() else { break };
                self.agent.run_in_realm(&self.realm, |agent, gc| {
                    let _ = job.run(agent, gc);
                });
            }
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
