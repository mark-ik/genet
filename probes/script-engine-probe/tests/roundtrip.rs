//! The proof: a NodeId reflector + a host-mutating native fn, round-tripped
//! through Boa, validating both Appendix A findings against a real engine.

use script_engine_probe::boa_backend::BoaEngine;
use script_engine_probe::dom::DomStore;
use script_engine_probe::{ScriptEngine, ScriptEngineLive};

#[test]
fn reflector_round_trip() {
    let store = DomStore::shared();
    let id = store.borrow_mut().push("DIV");

    let mut engine = BoaEngine::with_store(store.clone()).expect("engine builds");
    engine.install_reflector("node", id).expect("reflector installs");

    // JS mutates Rust-owned host state via the captured-store global (Finding 1),
    // then reads the native NodeId back out via the reflector method (Finding 2).
    let result = engine
        .eval("setText(0, 'hello world'); node.tag()")
        .expect("eval succeeds");

    // Finding 2: the reflector carried the NodeId into native code, which read the
    // tag back out of the host store.
    assert_eq!(engine.value_to_string(&result).unwrap(), "DIV");
    // Finding 1: JS mutated Rust-owned host state through the traced capture.
    assert_eq!(store.borrow().text(id), "hello world");
}

#[test]
fn reflector_survives_gc() {
    let store = DomStore::shared();
    let id = store.borrow_mut().push("SPAN");

    let mut engine = BoaEngine::with_store(store.clone()).expect("engine builds");
    engine.install_reflector("node", id).expect("reflector installs");

    // Attach a JS handler as a property on the reflector (Boa traces object props).
    engine
        .eval("node.handler = function () { setText(0, 'after-gc'); };")
        .expect("install handler");

    // Force a full collection. The reflector is reachable via the `node` global, so
    // it and its handler property must survive — proving the reflector + its
    // JS-attached state aren't collected out from under the host.
    engine.force_gc();

    engine.eval("node.handler()").expect("handler still callable");
    assert_eq!(store.borrow().text(id), "after-gc");
}

#[test]
fn host_held_handler_survives_gc() {
    let store = DomStore::shared();
    let id = store.borrow_mut().push("BUTTON");

    let mut engine = BoaEngine::with_store(store.clone()).expect("engine builds");
    engine.install_reflector("node", id).expect("reflector installs");

    // The host registers a JS handler against the NodeId — a cross-heap reference
    // the host now holds (in the Boa-traced HandlerTable).
    engine
        .eval("on(node, function () { setText(0, 'clicked'); });")
        .expect("register handler");

    // Force GC. The handler is reachable only through the host's table; were that
    // table not traced, the handler would be collected and `fire` would no-op.
    engine.force_gc();

    // Host fires the handler by NodeId; it survived and still mutates host state.
    engine.eval("fire(node)").expect("fire");
    assert_eq!(store.borrow().text(id), "clicked");
}

#[test]
fn value_surface_round_trips() {
    let mut engine = BoaEngine::new().expect("engine builds");
    let v = engine.eval("'a' + (1 + 2)").expect("eval");
    assert_eq!(engine.value_to_string(&v).unwrap(), "a3");
}
