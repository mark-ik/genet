// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Boa 0.21 backend for [`script_engine_api`]. Pure Rust → the wasm32 scripting
//! backend, and the native conformance oracle. Engine-native types (`JsValue`,
//! `Context`, the reflector `Class`) stay confined to this crate.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::rc::Rc;

use boa_engine::{
    Context, JsData, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    Source,
    builtins::promise::PromiseState,
    class::{Class, ClassBuilder},
    module::{Module, ModuleLoader, ModuleRequest, Referrer},
    object::{
        WeakJsObject,
        builtins::{JsFunction, JsPromise},
    },
};
use boa_gc::{Finalize, GcRefCell, Trace};
use script_engine_api::{
    Budget, CallCx, HostData, NativeFn, PromiseToken, PumpOutcome, ReflectorData, ScriptEngine,
    ScriptEngineLive,
};

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

/// The resolve/reject functions of a pending host promise, held until the host
/// settles it. Traced: the `JsFunction`s are live JS objects that must survive
/// collection while the promise is pending.
#[derive(Trace, Finalize)]
struct PendingPromise {
    resolve: JsFunction,
    reject: JsFunction,
}

/// Host-data slot stored in Boa's `Context` host-defined data. Holds the
/// engine-neutral [`HostData`] (the `Rc<dyn Any>` is not traced — it holds host
/// state, never JS values) plus the canonical-reflector cache (`NodeId →
/// reflector`). The cache holds each reflector **weakly** (a [`WeakJsObject`]):
/// it pins canonical identity (`document.body === document.body`) only while
/// script still references the reflector, and reports the death once script
/// drops it (G1 reflector liveness — see
/// [`drain_dead_reflectors`](ScriptEngine::drain_dead_reflectors)). The `pending`
/// table is traced: live resolving functions for host promises awaiting
/// settlement, keyed by [`PromiseToken`]. All engine-side, never in neutral host
/// state.
#[derive(Trace, Finalize, JsData)]
struct HostCell {
    #[unsafe_ignore_trace]
    data: RefCell<Option<HostData>>,
    reflectors: GcRefCell<HashMap<u64, WeakJsObject>>,
    pending: GcRefCell<HashMap<u64, PendingPromise>>,
    #[unsafe_ignore_trace]
    next_token: Cell<u64>,
}

impl HostCell {
    fn new() -> Self {
        Self {
            data: RefCell::new(None),
            reflectors: GcRefCell::new(HashMap::new()),
            pending: GcRefCell::new(HashMap::new()),
            next_token: Cell::new(0),
        }
    }
}

/// Mint a pending promise and register its resolving functions in the host cell.
/// Shared by the engine-level and in-callback `new_host_promise`, both of which hold
/// a `&mut Context`.
fn make_pending(ctx: &mut Context) -> JsResult<(JsValue, PromiseToken)> {
    let (promise, resolvers) = JsPromise::new_pending(ctx);
    let Some(cell) = ctx.get_data::<HostCell>() else {
        return Err(JsNativeError::typ().with_message("host cell missing").into());
    };
    let token = cell.next_token.get();
    cell.next_token.set(token + 1);
    cell.pending.borrow_mut().insert(
        token,
        PendingPromise { resolve: resolvers.resolve, reject: resolvers.reject },
    );
    Ok((promise.into(), token))
}

/// The host module resolver for one `eval_module` call: maps an import
/// `(specifier, referrer_url)` to the imported module's `(resolved_url, source)`,
/// or `None` when it cannot be resolved/fetched.
type ModuleResolver<'a> = dyn FnMut(&str, &str) -> Option<(String, String)> + 'a;

/// Boa [`ModuleLoader`] backed by a host resolver. The loader lives on the
/// `Context` for the engine's whole life, but the resolver borrows host state (the
/// page fetcher) and lives only for one `eval_module` call — so it is injected as a
/// scoped raw pointer, set for the duration of the call and cleared after.
/// `load_imported_module` reads it to fetch + parse each dependency on demand,
/// caching by resolved URL so a diamond / cycle loads each module once.
#[derive(Default)]
struct HostModuleLoader {
    /// Parsed modules by resolved URL — the per-call cache, cleared each call.
    cache: RefCell<HashMap<String, Module>>,
    /// Raw pointer to the active resolver, set for one `eval_module` call (`None`
    /// otherwise). The pointee outlives the call (it is an `eval_module` argument),
    /// so the deref in `load_imported_module` is sound; single-threaded, no reentrancy.
    resolver: Cell<Option<*mut ModuleResolver<'static>>>,
}

impl HostModuleLoader {
    /// Install `resolver` for the duration of `f`, then clear it and the module
    /// cache. The lifetime is erased to `'static` for storage in the long-lived
    /// loader; it is never observed past `f`, where the real resolver lives.
    fn with_resolver<R>(&self, resolver: &mut ModuleResolver<'_>, f: impl FnOnce() -> R) -> R {
        let raw: *mut ModuleResolver<'_> = resolver;
        // SAFETY: erases only the trait object's captured-data lifetime, not the
        // (HRTB) argument lifetimes; same fat-pointer layout. Cleared below before
        // the real lifetime ends.
        let erased: *mut ModuleResolver<'static> = unsafe { std::mem::transmute(raw) };
        self.resolver.set(Some(erased));
        let out = f();
        self.resolver.set(None);
        self.cache.borrow_mut().clear();
        out
    }
}

impl ModuleLoader for HostModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        let result = (|| {
            let specifier = request.specifier().to_std_string_escaped();
            // The importing module's URL (its `path`, set when we parsed it) is the
            // base its relative imports resolve against; empty for the entry's realm.
            let referrer_url =
                referrer.path().and_then(Path::to_str).unwrap_or("").to_string();

            let Some(ptr) = self.resolver.get() else {
                return Err(JsNativeError::typ()
                    .with_message("no module resolver active")
                    .into());
            };
            // SAFETY: `ptr` was set by `with_resolver` to a resolver that outlives
            // this call (cleared after); single-threaded, non-reentrant access.
            let resolve = unsafe { &mut *ptr };
            let Some((url, source)) = resolve(&specifier, &referrer_url) else {
                return Err(JsNativeError::typ()
                    .with_message(format!("could not resolve module '{specifier}'"))
                    .into());
            };

            if let Some(module) = self.cache.borrow().get(&url).cloned() {
                return Ok(module);
            }
            let module = Module::parse(
                Source::from_bytes(source.as_bytes()).with_path(Path::new(&url)),
                None,
                &mut context.borrow_mut(),
            )?;
            self.cache.borrow_mut().insert(url, module.clone());
            Ok(module)
        })();
        async { result }
    }
}

/// A Boa-backed scripting engine.
pub struct BoaEngine {
    ctx: Context,
    /// The module loader installed on `ctx`; `eval_module` sets its resolver per call.
    loader: Rc<HostModuleLoader>,
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
        // Cache hit *and still alive*: return the same object so reflectors compare
        // `===`. A dead weak (script dropped it) falls through to a fresh mint.
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            if let Some(obj) = cell.reflectors.borrow().get(&data).and_then(WeakJsObject::upgrade)
            {
                return Ok(obj.into());
            }
        }
        let v = self.make_reflector(data)?;
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            if let Some(obj) = v.as_object() {
                cell.reflectors.borrow_mut().insert(data, obj.downgrade());
            }
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

    fn new_host_promise(&mut self) -> Result<(JsValue, PromiseToken), JsError> {
        make_pending(self.ctx)
    }
}

impl ScriptEngine for BoaEngine {
    type Value = JsValue;
    type Error = JsError;
    type CallCx<'a> = BoaCallCx<'a>;

    fn new() -> Result<Self, Self::Error> {
        let loader = Rc::new(HostModuleLoader::default());
        let mut ctx = Context::builder().module_loader(loader.clone()).build()?;
        ctx.register_global_class::<Reflector>()?;
        ctx.insert_data(HostCell::new());
        Ok(Self { ctx, loader })
    }

    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
        self.ctx.eval(Source::from_bytes(source))
    }

    fn eval_module(
        &mut self,
        source: &str,
        base_url: &str,
        resolve: &mut dyn FnMut(&str, &str) -> Option<(String, String)>,
    ) -> Result<Option<Self::Value>, Self::Error> {
        // Install the host resolver for this call so the loader can fetch imports,
        // then parse the entry (its `path` = `base_url`, the base its imports resolve
        // against) and drive load → link → evaluate. `run_jobs` settles the promise
        // (synchronously, since the resolver fetches synchronously).
        let loader = Rc::clone(&self.loader);
        let state = loader.with_resolver(resolve, || {
            let module = Module::parse(
                Source::from_bytes(source.as_bytes()).with_path(Path::new(base_url)),
                None,
                &mut self.ctx,
            )?;
            let promise = module.load_link_evaluate(&mut self.ctx);
            let _ = self.ctx.run_jobs();
            Ok::<_, JsError>(promise.state())
        })?;
        match state {
            PromiseState::Fulfilled(_) => Ok(Some(JsValue::undefined())),
            PromiseState::Rejected(reason) => Err(JsError::from_opaque(reason)),
            PromiseState::Pending => Err(JsNativeError::typ()
                .with_message("module evaluation did not settle synchronously")
                .into()),
        }
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

    fn pump(&mut self, _budget: Budget) -> PumpOutcome {
        // Boa's default executor (SimpleJobExecutor) runs the promise-job queue to
        // completion and exposes no sub-drain, so the budget cannot be honored: drain
        // fully and report `Quiescent`. Step-bounding is a Nova/piccolo capability; on
        // Boa a runaway microtask loop still hangs here (acceptable: Boa is the
        // wasm/oracle backend, the runaway-sensitive actors run on native Nova).
        let _ = self.ctx.run_jobs();
        PumpOutcome::Quiescent
    }

    fn new_host_promise(&mut self) -> Result<(Self::Value, PromiseToken), Self::Error> {
        make_pending(&mut self.ctx)
    }

    fn settle_host_promise(
        &mut self,
        token: PromiseToken,
        outcome: Result<&Self::Value, &Self::Value>,
    ) -> Result<(), Self::Error> {
        // Take the resolving functions out of the table (consume the token), then call
        // the matching one. Calling enqueues the reaction jobs; the host drains them
        // with `pump_microtasks`. An unknown/already-settled token is a no-op.
        let pending = self
            .ctx
            .get_data::<HostCell>()
            .and_then(|cell| cell.pending.borrow_mut().remove(&token));
        let Some(pending) = pending else {
            return Ok(());
        };
        let undefined = JsValue::undefined();
        match outcome {
            Ok(value) => {
                pending.resolve.call(&undefined, &[value.clone()], &mut self.ctx)?;
            },
            Err(error) => {
                pending.reject.call(&undefined, &[error.clone()], &mut self.ctx)?;
            },
        }
        Ok(())
    }

    fn force_gc(&mut self) {
        // Drive Boa's collector so the weak canonical-cache entries for reflectors
        // script no longer references become dead before `drain_dead_reflectors`
        // sweeps them — the engine half of the frame-cadence GC tick.
        boa_gc::force_collect();
    }

    fn drain_dead_reflectors(&mut self) -> Vec<ReflectorData> {
        // Real death-reporting: sweep the weak canonical cache and report (and
        // forget) the reflectors whose JS objects have been collected since the
        // last call. Backed by the vendored boa patch (`JsObject::downgrade` /
        // `WeakJsObject::upgrade`). The host unpins each returned id, freeing the
        // underlying detached node for collection (G3).
        let mut dead = Vec::new();
        if let Some(cell) = self.ctx.get_data::<HostCell>() {
            cell.reflectors.borrow_mut().retain(|&data, weak| {
                if weak.upgrade().is_some() {
                    true
                } else {
                    dead.push(data);
                    false
                }
            });
        }
        dead
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

    #[test]
    fn host_promise_bridges_js_await() {
        let mut engine = BoaEngine::new().unwrap();

        // Resolve path: a parked `await` resumes when the host settles the promise.
        let (promise, token) = engine.new_host_promise().unwrap();
        engine.set_global("p", &promise).unwrap();
        engine
            .eval("globalThis.out = 'pending'; (async () => { globalThis.out = await p; })();")
            .unwrap();
        // Drain the script's own microtasks so the async fn reaches its parked `await`.
        engine.pump_microtasks();
        // The await is parked until the host settles; the post-await line has not run.
        let parked = engine.eval("out").unwrap();
        assert_eq!(engine.value_to_string(&parked).unwrap(), "pending");

        // Parenthesized so the literal is an expression, not a directive prologue
        // (a bare string statement has no completion value, per spec).
        let resolution = engine.eval("('resolved!')").unwrap();
        engine.settle_host_promise(token, Ok(&resolution)).unwrap();
        engine.pump_microtasks();
        let resumed = engine.eval("out").unwrap();
        assert_eq!(engine.value_to_string(&resumed).unwrap(), "resolved!");

        // Reject path: the awaiting `catch` sees the host's error value.
        let (promise2, token2) = engine.new_host_promise().unwrap();
        engine.set_global("q", &promise2).unwrap();
        engine
            .eval(
                "globalThis.err = 'none'; \
                 (async () => { try { await q; } catch (e) { globalThis.err = e; } })();",
            )
            .unwrap();
        let reason = engine.eval("('boom')").unwrap();
        engine.settle_host_promise(token2, Err(&reason)).unwrap();
        engine.pump_microtasks();
        let caught = engine.eval("err").unwrap();
        assert_eq!(engine.value_to_string(&caught).unwrap(), "boom");

        // Double-settle is a silent no-op (the token was consumed), not an error.
        engine.settle_host_promise(token, Ok(&resolution)).unwrap();
    }

    #[test]
    fn reflector_for_reports_death_after_gc() {
        let mut engine = BoaEngine::new().unwrap();

        // A callback handing JS the *canonical* reflector for node 0x42.
        struct Canonical;
        impl NativeFn<BoaEngine> for Canonical {
            fn call(cx: &mut BoaCallCx<'_>) -> JsResult<JsValue> {
                cx.reflector_for(0x42)
            }
        }
        engine.set_function::<Canonical>("canonical", 0).unwrap();

        // Hold the reflector from JS: while reachable, no death is reported, and
        // the canonical identity holds (=== the same object).
        let same = engine
            .eval("globalThis.x = canonical(); globalThis.x === canonical()")
            .unwrap();
        assert_eq!(engine.value_to_string(&same).unwrap(), "true");
        assert!(engine.drain_dead_reflectors().is_empty());

        // Drop the last JS reference and collect: the weak cache reports the death.
        engine.eval("globalThis.x = null;").unwrap();
        boa_gc::force_collect();
        assert_eq!(engine.drain_dead_reflectors(), vec![0x42]);

        // The dead entry was swept, so a second drain is empty.
        assert!(engine.drain_dead_reflectors().is_empty());
    }

    #[test]
    fn pump_drains_fully_regardless_of_budget() {
        let mut engine = BoaEngine::new().unwrap();
        engine
            .eval("globalThis.n = 0; Promise.resolve().then(() => { globalThis.n++; });")
            .unwrap();
        // Boa cannot sub-drain, so even a tight `Steps` budget drains to quiescence.
        assert_eq!(engine.pump(Budget::Steps(1)), PumpOutcome::Quiescent);
        let n = engine.eval("n").unwrap();
        assert_eq!(engine.value_to_string(&n).unwrap(), "1");
    }
}
