// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `fetch()` host seam end-to-end through the JS surface, with a stub
//! handler (no real network). Proves: fetch() returns a Promise that resolves to
//! a Response carrying the handler's status/headers/body; the request reaches the
//! handler with method/url/headers/body intact; and no handler = a network error
//! (rejected promise). Backend: Boa (pure Rust, all targets).

use script_engine_api::ScriptEngine;
use script_engine_boa::BoaEngine;
use script_runtime_api::{FetchHandler, FetchOutcome, FetchRequest, Runtime};

/// Echoes the request back: 200, a couple of headers describing the request, and
/// a body naming the method + url. Records the seen request for assertions.
struct EchoFetch;

impl FetchHandler for EchoFetch {
    fn fetch(&self, req: FetchRequest) -> FetchOutcome {
        let accept = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("accept"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        FetchOutcome {
            network_error: false,
            status: 200,
            status_text: "OK".to_owned(),
            response_type: "basic".to_owned(),
            url: req.url.clone(),
            headers: vec![
                ("content-type".to_owned(), "text/plain".to_owned()),
                ("x-echo-method".to_owned(), req.method.clone()),
                ("x-echo-accept".to_owned(), accept),
                ("x-echo-body".to_owned(), req.body.is_some().to_string()),
            ],
            body: format!("echo:{}:{}", req.method, req.url).into_bytes(),
        }
    }
}

fn read(rt: &mut Runtime<BoaEngine>, expr: &str) -> String {
    let v = rt.eval(expr).expect("eval");
    rt.engine_mut().value_to_string(&v).expect("value_to_string")
}

#[test]
fn fetch_resolves_to_response_through_the_handler() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    rt.set_fetch_handler(Box::new(EchoFetch));

    rt.eval(
        r#"
        var R = {};
        fetch("http://example.test/path", { headers: { Accept: "text/plain" } })
          .then(function(res) {
            R.status = res.status;
            R.ok = res.ok;
            R.type = res.type;
            R.url = res.url;
            R.ct = res.headers.get("content-type");
            R.method = res.headers.get("x-echo-method");
            R.accept = res.headers.get("x-echo-accept");
            R.hadBody = res.headers.get("x-echo-body");
            return res.text();
          })
          .then(function(body) { R.body = body; });
        "#,
    )
    .unwrap();
    rt.run_microtasks();

    assert_eq!(read(&mut rt, "String(R.status)"), "200");
    assert_eq!(read(&mut rt, "String(R.ok)"), "true");
    assert_eq!(read(&mut rt, "R.type"), "basic");
    assert_eq!(read(&mut rt, "R.url"), "http://example.test/path");
    assert_eq!(read(&mut rt, "R.ct"), "text/plain");
    assert_eq!(read(&mut rt, "R.method"), "GET");
    assert_eq!(read(&mut rt, "R.accept"), "text/plain", "request headers reach the handler");
    assert_eq!(read(&mut rt, "R.hadBody"), "false", "GET has no body");
    assert_eq!(read(&mut rt, "R.body"), "echo:GET:http://example.test/path");
}

#[test]
fn post_body_reaches_the_handler() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    rt.set_fetch_handler(Box::new(EchoFetch));
    rt.eval(
        r#"
        var R = {};
        fetch("http://x/y", { method: "post", body: "hello" })
          .then(function(res) { R.method = res.headers.get("x-echo-method"); R.hadBody = res.headers.get("x-echo-body"); });
        "#,
    )
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "R.method"), "POST");
    assert_eq!(read(&mut rt, "R.hadBody"), "true");
}

#[test]
fn no_handler_is_a_network_error() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    // No set_fetch_handler → every fetch is a network error (rejected promise).
    rt.eval(
        r#"
        var R = { rejected: false };
        fetch("http://x/y").then(function() { R.rejected = false; }, function(e) { R.rejected = (e instanceof TypeError); });
        "#,
    )
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "String(R.rejected)"), "true", "no handler rejects with a TypeError");
}
