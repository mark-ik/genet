// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Piccolo (stackless Lua) backend for [`script_engine_api`] — the seam's third
//! backend and the piccolo fork's first in-tree consumer (G4 in
//! `docs/2026-06-11_gc_arena_dom_plan.md`).
//!
//! An **option module**: a pluggable Lua backend for mod-scripting, not a third
//! first-party substrate (the Rust+JS decision stands). It exercises the fork
//! and makes the cross-backend conformance suite real (the seam stops being
//! JS-only).
//!
//! **Scope (clean surface).** `eval`, value→string, globals, native host-fns,
//! and native-data reflectors (Lua `userdata` carrying a `NodeId`, with a
//! canonical-identity cache). Engine-native types (`Lua`, `Context`, `Value`,
//! `UserData`) stay confined here, mirroring the Nova and Boa backends.
//!
//! The host-promise bridge is built on piccolo's executor-level yield/resume:
//! `new_host_promise` mints a promise userdata; the global `await(p)` suspends
//! the running executor on it; `settle_host_promise` resumes that executor
//! (resolve → value, reject → raised Lua error); `pump` drives resumed
//! executors to completion.
//!
//! **Documented deviations** (Lua is not JS):
//! - No `null`/`undefined` distinction — both [`CallCx::make_null`] and
//!   [`CallCx::undefined`] yield Lua `nil`.
//! - `await` is an explicit global function (`await(p)`), not syntax, and there
//!   are no Promise combinators (`.then`, `Promise.all`). `eval` of a chunk
//!   that awaits *yields* rather than returning its final value (the host sees
//!   completion via `pump` after `settle`, like top-level await).
//! - `pump` drains the runnable set fully (no `Budget::Steps` honoring yet);
//!   Lua has no microtask queue, so it returns `Quiescent`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gc_arena::lock::RefLock;
use gc_arena::{Collect, Gc, GcWeak, Rootable};
use piccolo::userdata::UserDataInner;
use piccolo::{
    Callback, CallbackReturn, Closure, Context, Error, Executor, Fuel, IntoValue, Lua, Singleton,
    StashedExecutor, StashedValue, UserData, Value, Variadic,
};
use script_engine_api::{
    Budget, CallCx, HostData, NativeFn, PromiseToken, PumpOutcome, ReflectorData, ScriptEngine,
    ScriptEngineLive,
};

/// The canonical-reflector cache, an **in-arena** table mapping a `NodeId` to a
/// *weak* reference to its reflector userdata. Weak-held (`GcWeak`) so it pins
/// canonical identity (`node == node`) only while script still references the
/// reflector, and reports the death once script drops it and the userdata is
/// collected (G1 reflector liveness). It lives in the arena as a piccolo
/// [`Singleton`] — reachable from both the in-callback `reflector_for` and the
/// engine-level `drain_dead_reflectors` through `ctx` — because a `GcWeak` is
/// `'gc`-branded and cannot be stashed `'static` in the Rust-side `HostSlot`.
#[derive(Collect)]
#[collect(no_drop)]
struct ReflectorCache<'gc> {
    map: Gc<'gc, RefLock<HashMap<u64, GcWeak<'gc, UserDataInner<'gc>>>>>,
}

impl<'gc> Singleton<'gc> for ReflectorCache<'gc> {
    fn create(ctx: Context<'gc>) -> Self {
        ReflectorCache {
            map: Gc::new(&ctx, RefLock::new(HashMap::new())),
        }
    }
}

/// The native data a host-promise userdata carries: its [`PromiseToken`]. A
/// distinct type from a reflector's `u64`, so `downcast_static` tells a promise
/// from a reflector.
struct PromiseTokenData(PromiseToken);

/// Engine-side state reachable from native callbacks. Fully `'static` — it holds
/// the neutral [`HostData`] (the `Rc<dyn Any>`) plus the host-promise bridge
/// tables (registry handles, themselves `'static`) — so it needs no `Collect`
/// impl and rides into callbacks as a captured `Rc`, the piccolo analogue of
/// Nova's realm `[[HostDefined]]` slot. The canonical-reflector cache does *not*
/// live here (a `GcWeak` is `'gc`-branded); it is the in-arena
/// [`ReflectorCache`] singleton. Engine-side, off the neutral wall (rule 1).
struct HostSlot {
    /// The neutral host state (the DOM), set once via [`ScriptEngine::set_host_data`].
    data: RefCell<Option<HostData>>,
    /// Monotonic [`PromiseToken`] source.
    next_token: Cell<u64>,
    /// Executors parked on `await(p)`, keyed by the promise's token. Resumed by
    /// [`settle_host_promise`](ScriptEngine::settle_host_promise).
    waiters: RefCell<HashMap<PromiseToken, StashedExecutor>>,
    /// Outcomes settled *before* anyone awaited (the settle-before-await race):
    /// `Ok` resolves, `Err` rejects. `await` drains this before parking.
    settled: RefCell<HashMap<PromiseToken, Result<StashedValue, StashedValue>>>,
    /// Executors resumed by a settle and awaiting a [`pump`](ScriptEngine::pump)
    /// to drive them to completion (or their next `await`).
    runnable: RefCell<Vec<StashedExecutor>>,
}

impl HostSlot {
    fn new() -> Self {
        Self {
            data: RefCell::new(None),
            next_token: Cell::new(0),
            waiters: RefCell::new(HashMap::new()),
            settled: RefCell::new(HashMap::new()),
            runnable: RefCell::new(Vec::new()),
        }
    }

    fn mint_token(&self) -> PromiseToken {
        let token = self.next_token.get();
        self.next_token.set(token + 1);
        token
    }
}

/// Lua text for a value: Lua `tostring` coercion where it applies (strings,
/// numbers), else the type's display. The seam's `value_to_string` is a
/// `ToString`-style coercion, and this matches it for the value surface.
fn value_to_lua_string<'gc>(ctx: Context<'gc>, value: Value<'gc>) -> String {
    match value.into_string(ctx) {
        Some(s) => std::string::String::from_utf8_lossy(s.as_bytes()).into_owned(),
        None => value.display().to_string(),
    }
}

/// A Piccolo-backed scripting engine.
pub struct PiccoloEngine {
    lua: Lua,
    slot: Rc<HostSlot>,
}

/// The call context handed to a native callback. Piccolo's callback gives
/// `(Context<'gc>, Execution<'gc, '_>, Stack<'gc, '_>)`; the trampoline roots
/// the arguments into `StashedValue`s and collapses the rest onto the single
/// `'gc` lifetime the engine-neutral [`CallCx`] GAT carries.
pub struct PiccoloCallCx<'gc> {
    ctx: Context<'gc>,
    slot: Rc<HostSlot>,
    args: Vec<StashedValue>,
}

impl PiccoloCallCx<'_> {
    fn make_reflector_value(&self, data: ReflectorData) -> StashedValue {
        let ud = UserData::new_static(&self.ctx, data);
        self.ctx.stash(Value::UserData(ud))
    }
}

impl CallCx for PiccoloCallCx<'_> {
    type Value = StashedValue;
    type Error = String;

    fn arg(&mut self, i: usize) -> Self::Value {
        match self.args.get(i) {
            Some(v) => v.clone(),
            None => self.ctx.stash(Value::Nil),
        }
    }

    fn host_data(&self) -> Option<HostData> {
        self.slot.data.borrow().clone()
    }

    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
        let v = self.ctx.fetch(value);
        Ok(value_to_lua_string(self.ctx, v))
    }

    fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData> {
        match self.ctx.fetch(value) {
            Value::UserData(ud) => ud.downcast_static::<u64>().ok().copied(),
            _ => None,
        }
    }

    fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
        Ok(self.make_reflector_value(data))
    }

    fn reflector_for(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
        let ctx = self.ctx;
        let cache = ctx.singleton::<Rootable![ReflectorCache<'_>]>();
        // Cache hit *and still alive*: return the same userdata so the reflectors
        // compare equal in Lua. A dead weak (script dropped it) falls through.
        if let Some(inner) = cache
            .map
            .borrow()
            .get(&data)
            .and_then(|weak| weak.upgrade(&ctx))
        {
            let ud = UserData::from_inner(inner);
            return Ok(ctx.stash(Value::UserData(ud)));
        }
        // Miss/dead: mint once, cache a *weak* handle, return the userdata.
        let ud = UserData::new_static(&ctx, data);
        cache
            .map
            .borrow_mut(&ctx)
            .insert(data, Gc::downgrade(ud.into_inner()));
        Ok(ctx.stash(Value::UserData(ud)))
    }

    fn make_string(&mut self, s: &str) -> Result<Self::Value, Self::Error> {
        let js = piccolo::String::from_slice(&self.ctx, s);
        Ok(self.ctx.stash(Value::String(js)))
    }

    fn make_null(&mut self) -> Self::Value {
        // Deviation: Lua has no null; both null and undefined are nil.
        self.ctx.stash(Value::Nil)
    }

    fn undefined(&mut self) -> Self::Value {
        self.ctx.stash(Value::Nil)
    }

    fn new_host_promise(&mut self) -> Result<(Self::Value, PromiseToken), Self::Error> {
        // In-callback mint: an async host fn returns this so Lua can `await` it.
        let token = self.slot.mint_token();
        let ud = UserData::new_static(&self.ctx, PromiseTokenData(token));
        Ok((self.ctx.stash(Value::UserData(ud)), token))
    }
}

/// Install the global `await(p)`: suspend the current executor on a host promise
/// `p` until the host settles it. A resolve resumes `await` with the value; a
/// reject resumes it with a Lua error (catchable by `pcall`); a promise already
/// settled before the `await` returns/raises immediately.
fn install_await(ctx: Context<'_>, slot: Rc<HostSlot>) {
    let await_cb = Callback::from_fn(&ctx, move |ctx, exec, mut stack| {
        let arg0 = if stack.is_empty() {
            Value::Nil
        } else {
            stack[0]
        };
        let token = match arg0 {
            Value::UserData(ud) => match ud.downcast_static::<PromiseTokenData>() {
                Ok(p) => p.0,
                Err(_) => {
                    return Err("await: argument is not a host promise"
                        .into_value(ctx)
                        .into());
                },
            },
            _ => {
                return Err("await: argument is not a host promise"
                    .into_value(ctx)
                    .into());
            },
        };
        stack.clear();
        // Settle-before-await: hand back (or raise) the stored outcome now.
        if let Some(outcome) = slot.settled.borrow_mut().remove(&token) {
            return match outcome {
                Ok(v) => {
                    stack.push_back(ctx.fetch(&v));
                    Ok(CallbackReturn::Return)
                },
                Err(e) => Err(ctx.fetch(&e).into()),
            };
        }
        // Park the current executor under this token, then yield it out to the
        // host (which resumes it on settle, driven by `pump`).
        slot.waiters
            .borrow_mut()
            .insert(token, ctx.stash(exec.executor()));
        Ok(CallbackReturn::Yield {
            to_thread: None,
            then: None,
        })
    });
    ctx.set_global("await", await_cb);
}

impl PiccoloEngine {
    /// Take the first completion value off a finished executor (Lua returns a
    /// value list; the seam wants one). Shared by `eval` and `eval_bounded`.
    fn take_first(&mut self, executor: &StashedExecutor) -> Result<StashedValue, String> {
        self.lua
            .try_enter(|ctx| {
                let ex = ctx.fetch(executor);
                let vals = match ex.take_result::<Variadic<Vec<Value>>>(ctx) {
                    Ok(inner) => inner?,
                    Err(_bad_mode) => return Ok(ctx.stash(Value::Nil)),
                };
                let first = vals.0.into_iter().next().unwrap_or(Value::Nil);
                Ok(ctx.stash(first))
            })
            .map_err(|e| e.to_string())
    }
}

impl ScriptEngine for PiccoloEngine {
    type Value = StashedValue;
    type Error = String;
    type CallCx<'a> = PiccoloCallCx<'a>;

    fn new() -> Result<Self, Self::Error> {
        let mut engine = Self {
            lua: Lua::full(),
            slot: Rc::new(HostSlot::new()),
        };
        let slot = engine.slot.clone();
        engine.lua.enter(|ctx| install_await(ctx, slot));
        Ok(engine)
    }

    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
        let src = source.as_bytes().to_vec();
        let executor: StashedExecutor = self
            .lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, None, &src[..])?;
                Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
            })
            .map_err(|e| e.to_string())?;
        self.lua.finish(&executor).map_err(|e| format!("{e:?}"))?;
        self.take_first(&executor)
    }

    fn eval_bounded(&mut self, source: &str, budget: Budget) -> Result<Self::Value, Self::Error> {
        let src = source.as_bytes().to_vec();
        let executor: StashedExecutor = self
            .lua
            .try_enter(|ctx| {
                let closure = Closure::load(ctx, None, &src[..])?;
                Ok(ctx.stash(Executor::start(ctx, closure.into(), ())))
            })
            .map_err(|e| e.to_string())?;

        // Step the executor with metered fuel, the bounded mirror of
        // `Lua::finish`. A `Steps(n)` budget caps the number of fuel
        // intervals, so a runaway script returns an error instead of looping
        // forever.
        const FUEL_PER_STEP: i32 = 4096;
        let max_steps = match budget {
            Budget::Unbounded => None,
            Budget::Steps(n) => Some(n),
        };
        let mut steps: u64 = 0;
        loop {
            let mut fuel = Fuel::with(FUEL_PER_STEP);
            let done = self
                .lua
                .enter(|ctx| ctx.fetch(&executor).step(ctx, &mut fuel))
                .map_err(|e| format!("{e:?}"))?;
            if done {
                break;
            }
            steps += 1;
            if let Some(max) = max_steps {
                if steps >= max {
                    return Err(format!(
                        "script budget exhausted after {steps} steps (~{} fuel)",
                        steps as i64 * FUEL_PER_STEP as i64
                    ));
                }
            }
        }
        self.take_first(&executor)
    }

    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
        Ok(self.lua.enter(|ctx| {
            let v = ctx.fetch(value);
            value_to_lua_string(ctx, v)
        }))
    }

    fn set_global(&mut self, name: &str, value: &Self::Value) -> Result<(), Self::Error> {
        // Piccolo's `set_global` wants a `&'static str`; globals are installed a
        // bounded number of times at setup, so leaking is acceptable (matches Nova).
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        self.lua.enter(|ctx| {
            let v = ctx.fetch(value);
            ctx.set_global(name, v);
        });
        Ok(())
    }

    fn set_host_data(&mut self, data: HostData) {
        *self.slot.data.borrow_mut() = Some(data);
    }

    fn set_function<F: NativeFn<Self>>(
        &mut self,
        name: &str,
        _length: usize,
    ) -> Result<(), Self::Error> {
        let name: &'static str = Box::leak(name.to_string().into_boxed_str());
        let slot = self.slot.clone();
        self.lua.enter(|ctx| {
            // The trampoline captures the host slot (a `'static` `Rc`) and is
            // monomorphized per `F` (which it names but does not capture by
            // value). State reaches `F::call` through `host_data` and the
            // reflector args, mirroring the other backends.
            let callback = Callback::from_fn(&ctx, move |ctx, _exec, mut stack| {
                let args: Vec<StashedValue> =
                    (0..stack.len()).map(|i| ctx.stash(stack[i])).collect();
                stack.clear();
                let mut cx = PiccoloCallCx {
                    ctx,
                    slot: slot.clone(),
                    args,
                };
                match F::call(&mut cx) {
                    Ok(ret) => {
                        let v = ctx.fetch(&ret);
                        stack.push_back(v);
                        Ok(CallbackReturn::Return)
                    },
                    Err(msg) => {
                        let s = piccolo::String::from_slice(&ctx, msg.as_bytes());
                        Err(Value::String(s).into())
                    },
                }
            });
            ctx.set_global(name, callback);
        });
        Ok(())
    }

    fn pump(&mut self, _budget: Budget) -> PumpOutcome {
        // Drive every executor a settle made runnable to completion (or its next
        // `await`, which re-parks it in `waiters`). Lua has no microtask queue,
        // so once the runnable set drains the engine is quiescent. A fresh
        // `eval` chunk runs synchronously and needs no pumping.
        loop {
            let next = self.slot.runnable.borrow_mut().pop();
            let Some(executor) = next else { break };
            let _ = self.lua.finish(&executor);
        }
        PumpOutcome::Quiescent
    }

    fn new_host_promise(&mut self) -> Result<(Self::Value, PromiseToken), Self::Error> {
        let token = self.slot.mint_token();
        let promise = self.lua.enter(|ctx| {
            let ud = UserData::new_static(&ctx, PromiseTokenData(token));
            ctx.stash(Value::UserData(ud))
        });
        Ok((promise, token))
    }

    fn settle_host_promise(
        &mut self,
        token: PromiseToken,
        outcome: Result<&Self::Value, &Self::Value>,
    ) -> Result<(), Self::Error> {
        // A parked executor resumes with the value (resolve) or a raised error
        // (reject), then becomes runnable for the next `pump`. With no waiter yet
        // (settle-before-await), stash the outcome for `await` to drain. An
        // unknown/already-settled token leaves no waiter and stashes a settled
        // value nobody reads — a harmless no-op, so double-settle is safe.
        let waiter = self.slot.waiters.borrow_mut().remove(&token);
        if let Some(executor) = waiter {
            self.lua.enter(|ctx| {
                let ex = ctx.fetch(&executor);
                match outcome {
                    Ok(v) => {
                        let value = ctx.fetch(v);
                        let _ = ex.resume(ctx, value);
                    },
                    Err(e) => {
                        let err: Error = ctx.fetch(e).into();
                        let _ = ex.resume_err(&ctx, err);
                    },
                }
            });
            self.slot.runnable.borrow_mut().push(executor);
        } else {
            let stored = match outcome {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(e.clone()),
            };
            self.slot.settled.borrow_mut().insert(token, stored);
        }
        Ok(())
    }

    fn drain_dead_reflectors(&mut self) -> Vec<ReflectorData> {
        // Real death-reporting: sweep the in-arena weak cache and report (and
        // forget) the reflectors whose userdata has been collected. piccolo *is*
        // gc-arena, so this needs no fork patch (unlike Boa/Nova). The host
        // unpins each returned id, freeing the underlying detached node for
        // collection (G3).
        self.lua.enter(|ctx| {
            let cache = ctx.singleton::<Rootable![ReflectorCache<'_>]>();
            let mut map = cache.map.borrow_mut(&ctx);
            let mut dead = Vec::new();
            map.retain(|&data, weak| {
                if weak.upgrade(&ctx).is_some() {
                    true
                } else {
                    dead.push(data);
                    false
                }
            });
            dead
        })
    }
}

impl ScriptEngineLive for PiccoloEngine {
    fn make_reflector(&mut self, data: ReflectorData) -> Result<Self::Value, Self::Error> {
        Ok(self.lua.enter(|ctx| {
            let ud = UserData::new_static(&ctx, data);
            ctx.stash(Value::UserData(ud))
        }))
    }

    fn reflector_data(&mut self, value: &Self::Value) -> Option<ReflectorData> {
        self.lua.enter(|ctx| match ctx.fetch(value) {
            Value::UserData(ud) => ud.downcast_static::<u64>().ok().copied(),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflector_round_trip() {
        let mut engine = PiccoloEngine::new().unwrap();
        let v = engine.make_reflector(0xDEAD_BEEF).unwrap();
        assert_eq!(engine.reflector_data(&v), Some(0xDEAD_BEEF));
        // A non-reflector value (a Lua table) yields None.
        let other = engine.eval("return {}").unwrap();
        assert_eq!(engine.reflector_data(&other), None);
    }

    #[test]
    fn value_surface() {
        let mut engine = PiccoloEngine::new().unwrap();
        let v = engine.eval("return 'a' .. (1 + 2)").unwrap();
        assert_eq!(engine.value_to_string(&v).unwrap(), "a3");
    }

    #[test]
    fn eval_bounded_runs_a_normal_script_to_completion() {
        let mut engine = PiccoloEngine::new().unwrap();
        // A terminating loop well under the cap completes normally.
        let v = engine
            .eval_bounded(
                "local s = 0 for i = 1, 100 do s = s + i end return s",
                Budget::Steps(1000),
            )
            .unwrap();
        assert_eq!(engine.value_to_string(&v).unwrap(), "5050");
    }

    #[test]
    fn eval_bounded_stops_a_runaway_instead_of_hanging() {
        let mut engine = PiccoloEngine::new().unwrap();
        let err = engine
            .eval_bounded("while true do end", Budget::Steps(50))
            .unwrap_err();
        assert!(
            err.contains("budget exhausted"),
            "a runaway script must error, got: {err}"
        );
        // The engine is still usable after stopping a runaway.
        let v = engine
            .eval_bounded("return 1 + 1", Budget::Steps(100))
            .unwrap();
        assert_eq!(engine.value_to_string(&v).unwrap(), "2");
    }

    #[test]
    fn global_reflector_is_reachable_from_lua() {
        let mut engine = PiccoloEngine::new().unwrap();
        let reflector = engine.make_reflector(0x1234).unwrap();
        engine.set_global("node", &reflector).unwrap();

        let from_lua = engine.eval("return node").unwrap();
        assert_eq!(engine.reflector_data(&from_lua), Some(0x1234));
    }

    #[test]
    fn native_fn_reaches_host_data_and_reflector_arg() {
        use std::cell::RefCell;
        use std::rc::Rc;

        // The host sink a `setText`-style callback writes to (stands in for the DOM).
        type Sink = RefCell<Vec<(ReflectorData, String)>>;
        let sink: Rc<Sink> = Rc::new(RefCell::new(Vec::new()));

        let mut engine = PiccoloEngine::new().unwrap();
        engine.set_host_data(sink.clone());

        // setText(node, text): recover the node id off the reflector arg, read the
        // text, and record both into host data — the Lua→host write path, reached
        // via the captured host slot, not a thread_local.
        struct SetText;
        impl NativeFn<PiccoloEngine> for SetText {
            fn call(cx: &mut PiccoloCallCx<'_>) -> Result<StashedValue, String> {
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
        engine.eval("setText(node, 'hello from Lua')").unwrap();

        assert_eq!(*sink.borrow(), vec![(0x42, "hello from Lua".to_string())]);
    }

    #[test]
    fn reflector_for_is_canonical() {
        // `reflector_for` returns the same userdata for the same id, so the two
        // reflectors compare equal in Lua (`==` on userdata is identity).
        struct Canonical;
        impl NativeFn<PiccoloEngine> for Canonical {
            fn call(cx: &mut PiccoloCallCx<'_>) -> Result<StashedValue, String> {
                cx.reflector_for(0x99)
            }
        }
        let mut engine = PiccoloEngine::new().unwrap();
        engine.set_function::<Canonical>("canonical", 0).unwrap();

        let same = engine.eval("return canonical() == canonical()").unwrap();
        assert_eq!(engine.value_to_string(&same).unwrap(), "true");
    }

    #[test]
    fn reflector_for_reports_death_after_gc() {
        struct Canonical;
        impl NativeFn<PiccoloEngine> for Canonical {
            fn call(cx: &mut PiccoloCallCx<'_>) -> Result<StashedValue, String> {
                cx.reflector_for(0x42)
            }
        }
        let mut engine = PiccoloEngine::new().unwrap();
        engine.set_function::<Canonical>("canonical", 0).unwrap();

        // Hold the reflector in a global: while referenced, no death is reported.
        engine.eval("x = canonical()").unwrap();
        assert!(engine.drain_dead_reflectors().is_empty());

        // Drop the last reference and collect: the weak cache reports the death.
        engine.eval("x = nil").unwrap();
        engine.lua.gc_collect();
        engine.lua.gc_collect();
        assert_eq!(engine.drain_dead_reflectors(), vec![0x42]);

        // The dead entry was swept, so a second drain is empty.
        assert!(engine.drain_dead_reflectors().is_empty());
    }

    #[test]
    fn null_and_undefined_are_nil() {
        let mut engine = PiccoloEngine::new().unwrap();
        let nil = engine.eval("return nil").unwrap();
        assert_eq!(engine.value_to_string(&nil).unwrap(), "nil");
    }

    #[test]
    fn host_promise_bridges_lua_await() {
        let mut engine = PiccoloEngine::new().unwrap();

        // Resolve path: a parked `await(p)` resumes when the host settles it.
        let (promise, token) = engine.new_host_promise().unwrap();
        engine.set_global("p", &promise).unwrap();
        // The chunk awaits at top level, so `eval` yields the executor; `out`
        // stays at its pre-await value until the settle+pump resumes it.
        engine.eval("out = 'pending'; out = await(p)").unwrap();
        let parked = engine.eval("return out").unwrap();
        assert_eq!(engine.value_to_string(&parked).unwrap(), "pending");

        let resolution = engine.eval("return 'resolved!'").unwrap();
        engine.settle_host_promise(token, Ok(&resolution)).unwrap();
        engine.pump_microtasks();
        let resumed = engine.eval("return out").unwrap();
        assert_eq!(engine.value_to_string(&resumed).unwrap(), "resolved!");

        // Reject path: the awaiting `pcall` sees the host's error value.
        let (promise2, token2) = engine.new_host_promise().unwrap();
        engine.set_global("q", &promise2).unwrap();
        engine
            .eval("err = 'none'; local ok, e = pcall(function() return await(q) end); if not ok then err = e end")
            .unwrap();
        let reason = engine.eval("return 'boom'").unwrap();
        engine.settle_host_promise(token2, Err(&reason)).unwrap();
        engine.pump_microtasks();
        let caught = engine.eval("return err").unwrap();
        assert_eq!(engine.value_to_string(&caught).unwrap(), "boom");

        // Settle-before-await: `await` returns the stored value immediately,
        // without yielding (so no pump is needed).
        let (promise3, token3) = engine.new_host_promise().unwrap();
        engine.set_global("r", &promise3).unwrap();
        let early = engine.eval("return 'early'").unwrap();
        engine.settle_host_promise(token3, Ok(&early)).unwrap();
        engine.eval("got = await(r)").unwrap();
        let got = engine.eval("return got").unwrap();
        assert_eq!(engine.value_to_string(&got).unwrap(), "early");

        // Double-settle of an already-resumed token is a silent no-op.
        engine.settle_host_promise(token, Ok(&resolution)).unwrap();
    }
}
