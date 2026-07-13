// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/. */
//! End-to-end proof of the `fetch()` -> netfetcher binding, against an offline
//! mock server: JS `fetch(url)` -> the runtime's FetchHandler seam -> netfetcher
//! -> real HTTP (mockito) -> a resolved `Response`. Gated on the `netfetch`
//! feature so the heavy async net stack only compiles when asked:
//!   cargo test -p genet-wpt --features netfetch --test fetch_netfetcher
//!
//! This is the load-bearing foundation; running the WPT `fetch/` corpus on top
//! additionally needs the WPT Python server + the `.any.js` wrapping (see
//! docs/2026-06-02_wpt_dom_sweep_and_binding_globals.md's sibling fetch notes).
#![cfg(feature = "netfetch")]

use script_engine_api::ScriptEngine;
use script_engine_boa::BoaEngine;
use script_runtime_api::{FetchHandler, FetchOutcome, FetchRequest, Runtime};

/// The host fetch seam, backed by netfetcher. Bridges the runtime's *sync*
/// FetchHandler to netfetcher's async engine by `block_on`-ing on a private
/// current-thread runtime (the reference-host shape; Mere's FetcherPool would
/// own one runtime off the UI thread).
struct NetFetchHandler {
    rt: tokio::runtime::Runtime,
}

impl NetFetchHandler {
    fn new() -> Self {
        Self {
            rt: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        }
    }
}

impl FetchHandler for NetFetchHandler {
    fn fetch(&self, req: FetchRequest) -> FetchOutcome {
        let Ok(url) = url::Url::parse(&req.url) else {
            return FetchOutcome::network_error();
        };
        self.rt.block_on(async move {
            let mut request = netfetcher::Request::get(url);
            request.method = match req.method.as_str() {
                "HEAD" => netfetcher::Method::Head,
                "POST" => netfetcher::Method::Post,
                "PUT" => netfetcher::Method::Put,
                "DELETE" => netfetcher::Method::Delete,
                "PATCH" => netfetcher::Method::Patch,
                "OPTIONS" => netfetcher::Method::Options,
                _ => netfetcher::Method::Get,
            };
            request.headers = req.headers;
            request.body = req.body.map(bytes::Bytes::from);

            let cx = netfetcher::FetchContext::permissive();
            let resp = netfetcher::fetch(request, &cx).await;
            if resp.is_network_error() {
                return FetchOutcome::network_error();
            }
            let status = resp.status;
            let headers = resp.headers.clone();
            let response_type = match resp.response_type {
                netfetcher::ResponseType::Basic => "basic",
                netfetcher::ResponseType::Cors => "cors",
                netfetcher::ResponseType::Opaque => "opaque",
                netfetcher::ResponseType::OpaqueRedirect => "opaqueredirect",
                netfetcher::ResponseType::Error => "error",
            }
            .to_owned();
            let url = resp
                .url_list
                .last()
                .map(|u| u.to_string())
                .unwrap_or_default();
            let redirected = resp.url_list.len() > 1;
            let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            FetchOutcome {
                network_error: false,
                status,
                status_text: String::new(),
                response_type,
                url,
                redirected,
                headers,
                body,
            }
        })
    }
}

fn read(rt: &mut Runtime<BoaEngine>, expr: &str) -> String {
    let v = rt.eval(expr).expect("eval");
    rt.engine_mut()
        .value_to_string(&v)
        .expect("value_to_string")
}

#[test]
fn js_fetch_through_netfetcher_against_mock_server() {
    let mut server = mockito::Server::new();
    let _get = server
        .mock("GET", "/hi")
        .with_status(200)
        .with_header("content-type", "text/plain")
        .with_body("hello from netfetcher")
        .create();
    let _post = server
        .mock("POST", "/echo")
        .with_status(201)
        .with_body("created")
        .create();
    let base = server.url();

    let mut rt = Runtime::<BoaEngine>::new().unwrap();
    rt.set_fetch_handler(Box::new(NetFetchHandler::new()));

    // GET: status, a response header, and the body all flow back to JS.
    rt.eval(&format!(
        r#"
        var G = {{}};
        fetch("{base}/hi")
          .then(function(res) {{ G.status = res.status; G.ok = res.ok; G.ct = res.headers.get("content-type"); return res.text(); }})
          .then(function(t) {{ G.body = t; }});
        "#
    ))
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "String(G.status)"), "200");
    assert_eq!(read(&mut rt, "String(G.ok)"), "true");
    assert_eq!(read(&mut rt, "G.ct"), "text/plain");
    assert_eq!(read(&mut rt, "G.body"), "hello from netfetcher");

    // POST with a body: method + body reach netfetcher; the 201 comes back.
    rt.eval(&format!(
        r#"
        var P = {{}};
        fetch("{base}/echo", {{ method: "POST", body: "payload" }})
          .then(function(res) {{ P.status = res.status; return res.text(); }})
          .then(function(t) {{ P.body = t; }});
        "#
    ))
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "String(P.status)"), "201");
    assert_eq!(read(&mut rt, "P.body"), "created");

    // A connection failure surfaces as a rejected promise (network error).
    rt.eval(
        r#"
        var E = { rejected: false };
        fetch("http://127.0.0.1:1/nope").then(function() {}, function(err) { E.rejected = (err instanceof TypeError); });
        "#,
    )
    .unwrap();
    rt.run_microtasks();
    assert_eq!(read(&mut rt, "String(E.rejected)"), "true");
}
