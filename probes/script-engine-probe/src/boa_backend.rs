//! Boa 0.21 backend. Engine-native types (`JsValue`, `Context`, `NativeFunction`,
//! the `Class`) stay confined to this module — the trait surface stays clean.

use std::cell::RefCell;
use std::rc::Rc;

use boa_engine::{
    class::{Class, ClassBuilder},
    js_string,
    property::Attribute,
    Context, JsData, JsError, JsNativeError, JsObject, JsResult, JsValue, NativeFunction, Source,
};
use boa_gc::{Finalize, GcRefCell, Trace};

use crate::dom::{DomStore, NodeId};
use crate::{ScriptEngine, ScriptEngineLive};

type SharedStore = Rc<RefCell<DomStore>>;

/// Host state placed in the context so native methods can reach the DOM store.
#[derive(Trace, Finalize, JsData)]
struct HostStore(#[unsafe_ignore_trace] SharedStore);

/// Capture payload for the explicit-captures native fn (Appendix A, Finding 1).
/// Must be `Trace` — that is the whole point: Boa can't trace closure
/// environments, so the store handle rides in an explicit, traced capture while
/// the closure itself stays `Copy`.
#[derive(Trace, Finalize)]
struct StoreCapture(#[unsafe_ignore_trace] SharedStore);

/// Reflector native data (Finding 2): carries ONLY the `NodeId`. The node's data
/// lives in the host store, never in the JS heap.
#[derive(Debug, Trace, Finalize, JsData)]
struct NodeReflector {
    #[unsafe_ignore_trace]
    id: NodeId,
}

impl Class for NodeReflector {
    const NAME: &'static str = "Node";
    const LENGTH: usize = 0;

    fn data_constructor(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<Self> {
        Err(JsNativeError::typ()
            .with_message("Node reflectors are host-created, not `new`-able")
            .into())
    }

    fn init(builder: &mut ClassBuilder<'_>) -> JsResult<()> {
        builder.method(js_string!("tag"), 0, NativeFunction::from_fn_ptr(node_tag));
        Ok(())
    }
}

/// `node.tag()` — downcast `this` to recover the native `NodeId` (Finding 2), then
/// read the host store via host-defined data.
fn node_tag(this: &JsValue, _args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = this
        .as_object()
        .and_then(|o| o.downcast_ref::<NodeReflector>().map(|n| n.id))
        .ok_or_else(|| JsNativeError::typ().with_message("receiver is not a Node"))?;
    let store = ctx
        .get_data::<HostStore>()
        .ok_or_else(|| JsNativeError::typ().with_message("no host store"))?
        .0
        .clone();
    let tag = store.borrow().tag(id);
    Ok(js_string!(tag).into())
}

/// Cross-heap rooting table (the hard case). The host registers JS handlers keyed
/// by `NodeId`. The key insight Boa forces: a `JsObject` parked in
/// `#[unsafe_ignore_trace]`'d Rust would NOT survive GC. To root it, the handler
/// must live in a **Boa-traced** container reachable from a root — here, a
/// `HostDefined` table whose `handler: JsObject` field is genuinely traced.
/// `NodeId` (u32) and `JsObject` both implement `Trace`, so the whole table is
/// naturally traceable; `GcRefCell` gives interior mutability that the GC can walk.
/// Contrast: rquickjs `Persistent` / Nova `Global` are detached roots you can hold
/// in plain Rust; Boa has no such free-standing root — you keep things reachable.
#[derive(Trace, Finalize, JsData)]
struct HandlerTable {
    entries: GcRefCell<Vec<HandlerEntry>>,
}

#[derive(Trace, Finalize)]
struct HandlerEntry {
    node: NodeId,
    handler: JsObject,
}

/// `on(node, fn)` — host registers a JS handler against a NodeId (the cross-heap
/// reference the host now holds).
fn handler_on(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get(0)
        .and_then(|v| v.as_object())
        .and_then(|o| o.downcast_ref::<NodeReflector>().map(|n| n.id))
        .ok_or_else(|| JsNativeError::typ().with_message("arg 0 must be a Node"))?;
    let handler = args
        .get(1)
        .and_then(|v| v.as_object())
        .map(|o| o.clone())
        .ok_or_else(|| JsNativeError::typ().with_message("arg 1 must be an object"))?;
    let table = ctx
        .get_data::<HandlerTable>()
        .ok_or_else(|| JsNativeError::typ().with_message("no handler table"))?;
    table.entries.borrow_mut().push(HandlerEntry { node: id, handler });
    Ok(JsValue::undefined())
}

/// `fire(node)` — host looks up the handler by NodeId and invokes it. If the
/// handler survived GC, this still runs.
fn handler_fire(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args
        .get(0)
        .and_then(|v| v.as_object())
        .and_then(|o| o.downcast_ref::<NodeReflector>().map(|n| n.id))
        .ok_or_else(|| JsNativeError::typ().with_message("arg 0 must be a Node"))?;
    let handler = {
        let table = ctx
            .get_data::<HandlerTable>()
            .ok_or_else(|| JsNativeError::typ().with_message("no handler table"))?;
        let entries = table.entries.borrow();
        entries.iter().find(|e| e.node == id).map(|e| e.handler.clone())
    };
    if let Some(handler) = handler {
        handler.call(&JsValue::undefined(), &[], ctx)?;
    }
    Ok(JsValue::undefined())
}

pub struct BoaEngine {
    ctx: Context,
}

impl BoaEngine {
    /// Build an engine sharing `store` with the caller (so a test can assert the
    /// mutations JS performs).
    pub fn with_store(store: SharedStore) -> Result<Self, JsError> {
        let mut ctx = Context::default();
        ctx.insert_data(HostStore(store.clone()));
        ctx.register_global_class::<NodeReflector>()?;

        // Finding 1: global `setText(id, text)` whose store handle rides in an
        // explicit traced capture; the closure is `Copy`.
        let capture = StoreCapture(store);
        ctx.register_global_callable(
            js_string!("setText"),
            2,
            NativeFunction::from_copy_closure_with_captures(
                |_this, args, cap: &StoreCapture, ctx| {
                    let id = args.get(0).cloned().unwrap_or(JsValue::undefined()).to_u32(ctx)?;
                    let text = args
                        .get(1)
                        .cloned()
                        .unwrap_or(JsValue::undefined())
                        .to_string(ctx)?
                        .to_std_string_escaped();
                    cap.0.borrow_mut().set_text(id, &text);
                    Ok(JsValue::undefined())
                },
                capture,
            ),
        )?;

        // Finding (cross-heap rooting): a host-held handler table, traced by Boa.
        ctx.insert_data(HandlerTable {
            entries: GcRefCell::new(Vec::new()),
        });
        ctx.register_global_callable(
            js_string!("on"),
            2,
            NativeFunction::from_fn_ptr(handler_on),
        )?;
        ctx.register_global_callable(
            js_string!("fire"),
            1,
            NativeFunction::from_fn_ptr(handler_fire),
        )?;

        Ok(Self { ctx })
    }
}

impl ScriptEngine for BoaEngine {
    type Value = JsValue;
    type Error = JsError;

    fn new() -> Result<Self, Self::Error> {
        Self::with_store(DomStore::shared())
    }

    fn eval(&mut self, source: &str) -> Result<Self::Value, Self::Error> {
        self.ctx.eval(Source::from_bytes(source))
    }

    fn value_to_string(&mut self, value: &Self::Value) -> Result<String, Self::Error> {
        Ok(value.to_string(&mut self.ctx)?.to_std_string_escaped())
    }
}

impl BoaEngine {
    /// Force a full GC pass — used by the probe to check a live reflector and its
    /// JS-attached state survive collection.
    pub fn force_gc(&self) {
        boa_gc::force_collect();
    }
}

impl ScriptEngineLive for BoaEngine {
    fn install_reflector(&mut self, global_name: &str, node: NodeId) -> Result<(), Self::Error> {
        let obj: JsObject = NodeReflector::from_data(NodeReflector { id: node }, &mut self.ctx)?;
        self.ctx
            .register_global_property(js_string!(global_name), obj, Attribute::all())?;
        Ok(())
    }
}
