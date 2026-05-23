//! Track B2 — the native-data reflector on Nova (Appendix A Finding 2, Nova
//! flavour), enabled by the `serval-embedder` patch to `EmbedderObject`.
//!
//! A JS-visible `EmbedderObject` carries a `NodeId` as NATIVE data, is rooted via
//! a `Global` held in plain Rust, survives forced GCs, and the `NodeId` is read
//! back out — the reflector bridge from JS heap to host DOM arena. Contrast Boa,
//! where the host-held handle had to live in a *traced* container.

use nova_vm::{
    ecmascript::{AgentOptions, DefaultHostHooks, EmbedderObject, GcAgent, Object},
    engine::{Bindable, Global},
};

#[test]
fn embedder_object_native_data_survives_gc() {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();

    const NODE_ID: u64 = 0xDEAD_BEEF;

    // Create a JS-visible reflector carrying a NodeId as native data; root it via a
    // Global stashed in plain Rust.
    let mut handle: Option<Global<Object>> = None;
    agent.run_in_realm(&realm, |agent, _gc| {
        let eo = EmbedderObject::create_with_data(agent, NODE_ID);
        let obj = Object::EmbedderObject(eo);
        handle = Some(Global::new(agent, obj.unbind()));
    });
    let handle = handle.expect("reflector rooted into a Global");

    agent.gc();
    agent.gc();

    // Recover and read the native NodeId back out.
    let mut recovered: u64 = 0;
    agent.run_in_realm(&realm, |agent, gc| {
        let obj = handle.get(agent, gc.nogc());
        let Object::EmbedderObject(eo) = obj else {
            panic!("expected an EmbedderObject, got {obj:?}");
        };
        recovered = eo.embedder_data(agent);
    });
    assert_eq!(recovered, NODE_ID, "native NodeId survived GC and round-tripped");

    agent.run_in_realm(&realm, |agent, _gc| {
        handle.take(agent);
    });
}
