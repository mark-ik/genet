// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `fetch()` host seam.
//!
//! `fetch()` is the one host capability that needs the network, which Mere owns
//! (the layering: serval/the runtime never link a network stack). So the runtime
//! exposes a *sync* [`FetchHandler`] trait — a host (e.g. the WPT runner, or
//! Mere) implements it over an async engine like netfetcher, doing the async work
//! inside (`block_on`) — and the JS `fetch()` / `Response` / `Headers` surface is
//! a bootstrap over a single native sink (`__fetch`). No network dependency
//! enters this crate; only the trait does.
//!
//! Async shape: `fetch()` returns a real `Promise`, but the handler runs to
//! completion synchronously and the Promise resolves at the next microtask
//! checkpoint. That suffices for the testharness runner (cooperative, not truly
//! concurrent); a future streaming/abortable path is a separate lift.

use std::cell::RefCell;

use script_engine_api::{CallCx, NativeFn, ScriptEngine};

use crate::HostState;

/// A fetch the host should perform on script's behalf.
pub struct FetchRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

/// The result handed back to script. A Fetch *network error* is
/// `network_error == true` (script sees a rejected promise / `TypeError`).
pub struct FetchOutcome {
    pub network_error: bool,
    pub status: u16,
    pub status_text: String,
    /// `basic` | `cors` | `opaque` | `opaqueredirect` | `error`.
    pub response_type: String,
    /// Final URL after redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl FetchOutcome {
    pub fn network_error() -> Self {
        Self {
            network_error: true,
            status: 0,
            status_text: String::new(),
            response_type: "error".to_owned(),
            url: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// The host seam. Implement over a real network engine (e.g. netfetcher) and
/// install with `Runtime::set_fetch_handler`. Synchronous: do any async work
/// inside (the runtime calls this from a native callback, off no event loop).
pub trait FetchHandler {
    fn fetch(&self, request: FetchRequest) -> FetchOutcome;
}

/// `__fetch(method, url, headers, body)` — the single native sink behind the JS
/// `fetch()` bootstrap. `headers` is a newline-delimited `k,v,k,v` list; `body`
/// is a string (empty = no body). Returns a JSON outcome string the bootstrap
/// parses. With no handler installed, every fetch is a network error.
pub(crate) struct Fetch;

impl<E: ScriptEngine> NativeFn<E> for Fetch {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let method = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let url = cx.value_to_string(&a1)?;
        let a2 = cx.arg(2);
        let headers_flat = cx.value_to_string(&a2)?;
        let a3 = cx.arg(3);
        let body_str = cx.value_to_string(&a3)?;

        let headers = parse_flat_headers(&headers_flat);
        let body = (!body_str.is_empty()).then(|| body_str.into_bytes());
        let request = FetchRequest { method, url, headers, body };

        let outcome = match cx.host_data() {
            Some(data) => match data.downcast_ref::<RefCell<HostState>>() {
                Some(cell) => match cell.borrow().fetch.as_ref() {
                    Some(handler) => handler.fetch(request),
                    None => FetchOutcome::network_error(),
                },
                None => FetchOutcome::network_error(),
            },
            None => FetchOutcome::network_error(),
        };
        cx.make_string(&encode_outcome(&outcome))
    }
}

/// Split a newline-delimited `k,v,k,v` header list into pairs (a trailing odd
/// element, if any, is dropped). A header name/value never contains a raw newline.
fn parse_flat_headers(flat: &str) -> Vec<(String, String)> {
    if flat.is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = flat.split('\n').collect();
    parts
        .chunks_exact(2)
        .map(|kv| (kv[0].to_owned(), kv[1].to_owned()))
        .collect()
}

/// Encode the outcome as a JSON object the bootstrap parses. Hand-rolled (no JSON
/// dep): the body is UTF-8-lossy (binary bodies degrade to replacement chars — a
/// known v1 limit; a bytes channel is a later lift).
fn encode_outcome(o: &FetchOutcome) -> String {
    let mut s = String::new();
    s.push('{');
    s.push_str(&format!("\"networkError\":{},", o.network_error));
    s.push_str(&format!("\"status\":{},", o.status));
    s.push_str("\"statusText\":");
    push_json_str(&mut s, &o.status_text);
    s.push_str(",\"type\":");
    push_json_str(&mut s, &o.response_type);
    s.push_str(",\"url\":");
    push_json_str(&mut s, &o.url);
    s.push_str(",\"headers\":[");
    for (i, (k, v)) in o.headers.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('[');
        push_json_str(&mut s, k);
        s.push(',');
        push_json_str(&mut s, v);
        s.push(']');
    }
    s.push_str("],\"body\":");
    push_json_str(&mut s, &String::from_utf8_lossy(&o.body));
    s.push('}');
    s
}

/// Append `s` as a JSON string literal (quotes + minimal escaping).
fn push_json_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Install the `__fetch` sink and the `fetch()` / `Response` / `Headers` bootstrap.
pub(crate) fn install_fetch_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<Fetch>("__fetch", 4)?;
    engine.eval(FETCH_BOOTSTRAP)?;
    Ok(())
}

/// Minimal Fetch API: `Headers`, `Response` (status/ok/type/url/headers +
/// text/json/clone), and `fetch()` over the `__fetch` sink. Enough for the
/// testharness fetch tests' object surface; not the full spec (no `Request`
/// object, streaming bodies, or `arrayBuffer`/`blob` yet).
const FETCH_BOOTSTRAP: &str = r#"
(function() {
  function Headers(init) {
    this._h = [];
    if (init) {
      if (init instanceof Headers) { var self = this; init.forEach(function(v, k) { self.append(k, v); }); }
      else if (Array.isArray(init)) { for (var i = 0; i < init.length; i++) this.append(init[i][0], init[i][1]); }
      else { for (var k in init) this.append(k, init[k]); }
    }
  }
  Headers.prototype.append = function(n, v) { this._h.push([String(n).toLowerCase(), String(v)]); };
  Headers.prototype.set = function(n, v) {
    n = String(n).toLowerCase();
    this._h = this._h.filter(function(p) { return p[0] !== n; });
    this._h.push([n, String(v)]);
  };
  Headers.prototype.get = function(n) {
    n = String(n).toLowerCase();
    var out = [];
    for (var i = 0; i < this._h.length; i++) if (this._h[i][0] === n) out.push(this._h[i][1]);
    return out.length ? out.join(", ") : null;
  };
  Headers.prototype.has = function(n) { return this.get(n) !== null; };
  Headers.prototype['delete'] = function(n) {
    n = String(n).toLowerCase();
    this._h = this._h.filter(function(p) { return p[0] !== n; });
  };
  Headers.prototype.forEach = function(cb, thisArg) {
    for (var i = 0; i < this._h.length; i++) cb.call(thisArg, this._h[i][1], this._h[i][0], this);
  };
  globalThis.Headers = Headers;

  function Response(o) {
    o = o || {};
    this.status = o.status || 0;
    this.statusText = o.statusText || "";
    this.ok = this.status >= 200 && this.status < 300;
    this.type = o.type || "default";
    this.url = o.url || "";
    this.redirected = false;
    this.headers = new Headers(o.headers);
    this.__body = (o.body != null) ? o.body : "";
    this.bodyUsed = false;
  }
  Response.prototype.text = function() {
    if (this.bodyUsed) return Promise.reject(new TypeError("body already used"));
    this.bodyUsed = true;
    return Promise.resolve(this.__body);
  };
  Response.prototype.json = function() { return this.text().then(function(t) { return JSON.parse(t); }); };
  Response.prototype.clone = function() {
    var r = Object.create(Response.prototype);
    r.status = this.status; r.statusText = this.statusText; r.ok = this.ok;
    r.type = this.type; r.url = this.url; r.redirected = this.redirected;
    r.headers = this.headers; r.__body = this.__body; r.bodyUsed = false;
    return r;
  };
  globalThis.Response = Response;

  function normalizeHeaders(h) {
    var out = [];
    if (!h) return out;
    if (h instanceof Headers) { h.forEach(function(v, k) { out.push([k, v]); }); }
    else if (Array.isArray(h)) { for (var i = 0; i < h.length; i++) out.push([String(h[i][0]), String(h[i][1])]); }
    else { for (var k in h) out.push([k, String(h[k])]); }
    return out;
  }
  globalThis.fetch = function(input, init) {
    init = init || {};
    var url = (input && typeof input === 'object') ? input.url : String(input);
    var method = String(init.method || 'GET').toUpperCase();
    var headers = normalizeHeaders(init.headers);
    var flat = [];
    for (var i = 0; i < headers.length; i++) { flat.push(headers[i][0]); flat.push(headers[i][1]); }
    var body = (init.body != null) ? String(init.body) : "";
    return new Promise(function(resolve, reject) {
      var o;
      try { o = JSON.parse(__fetch(method, url, flat.join("\n"), body)); }
      catch (e) { reject(new TypeError("Failed to fetch")); return; }
      if (o.networkError) { reject(new TypeError("Failed to fetch")); return; }
      resolve(new Response(o));
    });
  };
})();
"#;
