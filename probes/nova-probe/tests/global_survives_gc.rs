//! The Track B hypothesis (plan Part 6): a `Global` handle held in **plain Rust**
//! keeps a JS value alive across GC — no Boa-style traced-container dance.
//!
//! Contrast with Track A: in Boa, a host-held `JsObject` must live in a *traced*
//! `HostDefined` table or it's collected. Here the value is reachable ONLY through
//! a `Global` we stash in an ordinary Rust `Option`, and it survives forced GCs.

use nova_vm::{
    ecmascript::{
        AgentOptions, DefaultHostHooks, GcAgent, String as JsString, Value, parse_script,
        script_evaluation,
    },
    engine::{Bindable, Global},
};

/// The core hypothesis: a JS string reachable ONLY through a Rust-held `Global`
/// survives forced GCs unchanged. (Uses a Rust-created value, so it does not
/// depend on script completion-value semantics.)
#[test]
fn global_string_survives_gc() {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();

    const MARKER: &str = "heap-marker-kept-alive-only-by-the-rust-held-global-handle-0123456789";

    let mut handle: Option<Global<JsString>> = None;
    agent.run_in_realm(&realm, |agent, gc| {
        let s = JsString::from_string(agent, MARKER.to_string(), gc.nogc());
        handle = Some(Global::new(agent, s.unbind()));
    });
    let handle = handle.expect("value rooted into a Global");

    // Force several collections. Reachable only via the Rust-held Global.
    agent.gc();
    agent.gc();

    let mut recovered = String::new();
    agent.run_in_realm(&realm, |agent, gc| {
        let s = handle.get(agent, gc.nogc());
        recovered = s.to_string_lossy(agent).into_owned();
    });
    assert_eq!(recovered, MARKER, "Global-rooted value survived GC unchanged");

    // Nova requires explicit release (no Drop-based unrooting).
    agent.run_in_realm(&realm, |agent, _gc| {
        handle.take(agent);
    });
}

/// Informational: does Nova surface a script's top-level completion value, or
/// `undefined`? (Shapes how the binding layer reads results — return value vs
/// reading globals/host state.)
#[test]
fn eval_completion_value() {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();

    let mut out = String::new();
    agent.run_in_realm(&realm, |agent, mut gc| {
        let realm_id = agent.current_realm(gc.nogc());
        let src = JsString::from_string(agent, "1 + 2".to_string(), gc.nogc());
        let script = parse_script(agent, src, realm_id, false, None, gc.nogc()).unwrap();
        let value: Value = script_evaluation(agent, script.unbind(), gc.reborrow()).unwrap();
        out = value
            .unbind()
            .to_string(agent, gc)
            .unwrap()
            .to_string_lossy(agent)
            .into_owned();
    });
    println!("eval '1 + 2' completion value = {out:?}");
    assert!(out == "3" || out == "undefined", "got {out:?}");
}
