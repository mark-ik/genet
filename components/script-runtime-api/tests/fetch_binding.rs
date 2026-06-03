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

// ---- Fetch API object semantics (no network / handler needed) ----

#[test]
fn headers_object_semantics() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    // append combines; get joins with ", "; case-insensitive; iteration sorted.
    assert_eq!(
        read(&mut rt, r#"(function(){var h=new Headers();h.append("X-A","1");h.append("x-a","2");return h.get("X-A");})()"#),
        "1, 2"
    );
    assert_eq!(read(&mut rt, r#"(function(){var h=new Headers({"Content-Type":"text/plain"});return String(h.has("content-type"));})()"#), "true");
    assert_eq!(read(&mut rt, r#"(function(){var h=new Headers({a:"1"});h.set("A","9");h["delete"]("z");return h.get("a");})()"#), "9");
    // sorted iteration of names.
    assert_eq!(
        read(&mut rt, r#"(function(){var h=new Headers();h.append("b","2");h.append("a","1");var ks=[];h.forEach(function(v,k){ks.push(k);});return ks.join(",");})()"#),
        "a,b"
    );
    assert_eq!(read(&mut rt, r#"(function(){var h=new Headers();h.append("set-cookie","x=1");h.append("set-cookie","y=2");return h.getSetCookie().join("|");})()"#), "x=1|y=2");
    // invalid header name throws TypeError.
    assert_eq!(read(&mut rt, r#"(function(){try{new Headers().append("a b","1");return "noThrow";}catch(e){return e instanceof TypeError ? "TypeError" : "other";}})()"#), "TypeError");
}

#[test]
fn request_object_semantics() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    assert_eq!(read(&mut rt, r#"new Request("http://x/y").method"#), "GET");
    assert_eq!(read(&mut rt, r#"new Request("http://x/y", {method:"post"}).method"#), "POST");
    assert_eq!(read(&mut rt, r#"new Request("http://x/y").url"#), "http://x/y");
    // GET + body throws.
    assert_eq!(read(&mut rt, r#"(function(){try{new Request("http://x",{method:"GET",body:"b"});return "noThrow";}catch(e){return e instanceof TypeError?"TypeError":"other";}})()"#), "TypeError");
    // clone copies; reading body works.
    assert_eq!(read(&mut rt, r#"new Request("http://x",{method:"POST",body:"hi"}).clone().method"#), "POST");
    // header init carried.
    assert_eq!(read(&mut rt, r#"new Request("http://x",{headers:{Accept:"text/html"}}).headers.get("accept")"#), "text/html");
}

#[test]
fn response_object_semantics() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    assert_eq!(read(&mut rt, r#"new Response("hi",{status:201,statusText:"Created"}).status"#), "201");
    assert_eq!(read(&mut rt, r#"String(new Response("hi").ok)"#), "true");
    assert_eq!(read(&mut rt, r#"String(new Response(null,{status:404}).ok)"#), "false");
    // out-of-range status throws RangeError.
    assert_eq!(read(&mut rt, r#"(function(){try{new Response(null,{status:99});return "noThrow";}catch(e){return e instanceof RangeError?"RangeError":"other";}})()"#), "RangeError");
    // statics.
    assert_eq!(read(&mut rt, r#"Response.error().type"#), "error");
    assert_eq!(read(&mut rt, r#"String(Response.error().status)"#), "0");
    assert_eq!(read(&mut rt, r#"String(Response.redirect("http://h/",301).status)"#), "301");
    assert_eq!(read(&mut rt, r#"Response.redirect("http://h/",301).headers.get("location")"#), "http://h/");
    assert_eq!(read(&mut rt, r#"Response.json({a:1}).headers.get("content-type")"#), "application/json");
}

#[test]
fn body_is_single_use() {
    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    // text() then json() of the same body: second read rejects.
    rt.eval(
        r#"
        var B = {};
        var r = new Response('{"k":1}');
        r.text().then(function(t){ B.first = t; return r.json(); }).then(
          function(){ B.second = "resolved"; },
          function(e){ B.second = (e instanceof TypeError) ? "rejected" : "other"; }
        );
        "#,
    )
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "B.first"), r#"{"k":1}"#);
    assert_eq!(read(&mut rt, "B.second"), "rejected", "a consumed body cannot be re-read");
    // json() parses.
    rt.eval(r#"var J={}; new Response('{"k":42}').json().then(function(o){J.k=o.k;});"#).unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "String(J.k)"), "42");
}
