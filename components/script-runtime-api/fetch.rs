// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `fetch()` host seam.
//!
//! `fetch()` is the one host capability that needs the network, which Mere owns
//! (the layering: serval/the runtime never link a network stack). So the runtime
//! exposes a *sync* [`FetchHandler`] trait — a host (e.g. the WPT runner, or
//! Mere) implements it over an async engine like netfetcher, doing the async work
//! inside (`block_on`) — and the JS `fetch()` / `Request` / `Response` / `Headers`
//! surface is a bootstrap over a single native sink (`__fetch`). No network
//! dependency enters this crate; only the trait does.
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

/// `__resolve_url(url)` — resolve `url` against the document base URL (WHATWG URL
/// resolution via the `url` crate). An already-absolute `url` is returned
/// unchanged; a relative one with no base set is returned as-is (so a network
/// fetch of it fails, the disk-mode default). Backs relative `Request` / `fetch()`
/// URLs in server-mode WPT runs.
pub(crate) struct ResolveUrl;

impl<E: ScriptEngine> NativeFn<E> for ResolveUrl {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let input = cx.value_to_string(&a0)?;
        let base = host_base_url::<E>(cx);
        cx.make_string(&resolve_against(base.as_deref(), &input))
    }
}

/// Read the document base URL out of host state, if any.
fn host_base_url<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Option<String> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let base = cell.borrow().base_url.clone();
    base
}

/// WHATWG-resolve `input` against `base`. Absolute `input` wins; with no base, a
/// relative `input` is returned unchanged.
fn resolve_against(base: Option<&str>, input: &str) -> String {
    match base.and_then(|b| url::Url::parse(b).ok()) {
        Some(b) => b.join(input).map(|u| u.to_string()).unwrap_or_else(|_| input.to_owned()),
        None => input.to_owned(),
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

/// Install the `__fetch` sink and the `fetch()` / `Request` / `Response` /
/// `Headers` bootstrap.
pub(crate) fn install_fetch_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<Fetch>("__fetch", 4)?;
    engine.set_function::<ResolveUrl>("__resolve_url", 1)?;
    engine.eval(FETCH_BOOTSTRAP)?;
    Ok(())
}

/// The Fetch API JS surface: `Headers` (with validation + sorted iteration +
/// getSetCookie), `Request`, `Response` (+ `error`/`redirect`/`json` statics), a
/// shared body mixin (`text`/`json`/`arrayBuffer`), and `fetch()` over the
/// `__fetch` sink. Covers the object-semantics surface the WPT fetch/ tests
/// exercise; still missing streaming bodies, `FormData`/`Blob`, and `AbortSignal`.
const FETCH_BOOTSTRAP: &str = r#"
(function() {
  var hasSym = (typeof Symbol !== 'undefined' && Symbol.iterator);

  // RFC 7230 token for header names; values reject CR/LF/NUL and trim OWS.
  var TOKEN_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  function checkName(n) {
    n = String(n);
    if (!TOKEN_RE.test(n)) throw new TypeError("Invalid header name: '" + n + "'");
    return n.toLowerCase();
  }
  function checkValue(v) {
    v = String(v).replace(/^[ \t]+|[ \t]+$/g, "");
    if (/[\r\n\0]/.test(v)) throw new TypeError("Invalid header value");
    return v;
  }

  function makeIterator(arr) {
    var i = 0;
    var it = { next: function() { return i < arr.length ? { value: arr[i++], done: false } : { value: undefined, done: true }; } };
    if (hasSym) it[Symbol.iterator] = function() { return this; };
    return it;
  }

  function Headers(init) {
    this._h = [];
    if (init) {
      if (init instanceof Headers) { for (var i = 0; i < init._h.length; i++) this._h.push([init._h[i][0], init._h[i][1]]); }
      else if (Array.isArray(init)) {
        for (var j = 0; j < init.length; j++) {
          if (!init[j] || init[j].length !== 2) throw new TypeError("Invalid header entry");
          this.append(init[j][0], init[j][1]);
        }
      } else { for (var k in init) this.append(k, init[k]); }
    }
  }
  Headers.prototype.append = function(n, v) { this._h.push([checkName(n), checkValue(v)]); };
  Headers.prototype.set = function(n, v) {
    n = checkName(n); v = checkValue(v);
    this._h = this._h.filter(function(p) { return p[0] !== n; });
    this._h.push([n, v]);
  };
  Headers.prototype.get = function(n) {
    n = checkName(n);
    var out = [];
    for (var i = 0; i < this._h.length; i++) if (this._h[i][0] === n) out.push(this._h[i][1]);
    return out.length ? out.join(", ") : null;
  };
  Headers.prototype.has = function(n) {
    n = checkName(n);
    for (var i = 0; i < this._h.length; i++) if (this._h[i][0] === n) return true;
    return false;
  };
  Headers.prototype['delete'] = function(n) {
    n = checkName(n);
    this._h = this._h.filter(function(p) { return p[0] !== n; });
  };
  Headers.prototype.getSetCookie = function() {
    var out = [];
    for (var i = 0; i < this._h.length; i++) if (this._h[i][0] === 'set-cookie') out.push(this._h[i][1]);
    return out;
  };
  // Sorted, combined view for iteration (set-cookie kept un-combined).
  Headers.prototype._sorted = function() {
    var names = {}, order = [];
    for (var i = 0; i < this._h.length; i++) {
      var k = this._h[i][0];
      if (!(k in names)) { names[k] = []; order.push(k); }
      names[k].push(this._h[i][1]);
    }
    order.sort();
    var out = [];
    for (var j = 0; j < order.length; j++) {
      var n = order[j];
      if (n === 'set-cookie') { for (var c = 0; c < names[n].length; c++) out.push([n, names[n][c]]); }
      else out.push([n, names[n].join(", ")]);
    }
    return out;
  };
  Headers.prototype.forEach = function(cb, thisArg) {
    var s = this._sorted();
    for (var i = 0; i < s.length; i++) cb.call(thisArg, s[i][1], s[i][0], this);
  };
  Headers.prototype.entries = function() { return makeIterator(this._sorted()); };
  Headers.prototype.keys = function() { return makeIterator(this._sorted().map(function(p) { return p[0]; })); };
  Headers.prototype.values = function() { return makeIterator(this._sorted().map(function(p) { return p[1]; })); };
  if (hasSym) Headers.prototype[Symbol.iterator] = Headers.prototype.entries;
  globalThis.Headers = Headers;

  // ---- Body mixin: text / json / arrayBuffer, single-use. ----
  function consume(self) {
    if (self.bodyUsed) return Promise.reject(new TypeError("Body has already been consumed."));
    self.bodyUsed = true;
    return Promise.resolve(self.__body != null ? String(self.__body) : "");
  }
  function utf8Encode(s) {
    var b = [];
    for (var i = 0; i < s.length; i++) {
      var c = s.charCodeAt(i);
      if (c < 0x80) b.push(c);
      else if (c < 0x800) b.push(0xC0 | (c >> 6), 0x80 | (c & 0x3F));
      else if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
        var cp = 0x10000 + ((c & 0x3FF) << 10) + (s.charCodeAt(++i) & 0x3FF);
        b.push(0xF0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3F), 0x80 | ((cp >> 6) & 0x3F), 0x80 | (cp & 0x3F));
      } else b.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F));
    }
    return new Uint8Array(b);
  }
  var bodyMixin = {
    text: function() { return consume(this); },
    json: function() { return consume(this).then(function(t) { return JSON.parse(t); }); },
    arrayBuffer: function() { return consume(this).then(function(t) { return utf8Encode(t).buffer; }); }
  };

  // ---- Request ----
  function Request(input, init) {
    init = init || {};
    if (input instanceof Request) {
      this.url = input.url; this.method = input.method; this.headers = new Headers(input.headers);
      this.__body = input.__body; this.mode = input.mode; this.credentials = input.credentials;
      this.redirect = input.redirect; this.cache = input.cache; this.destination = input.destination;
    } else {
      this.url = __resolve_url(String(input)); this.method = 'GET'; this.headers = new Headers(); this.__body = null;
      this.mode = 'cors'; this.credentials = 'same-origin'; this.redirect = 'follow'; this.cache = 'default'; this.destination = '';
    }
    if (init.method !== undefined) this.method = String(init.method).toUpperCase();
    if (init.headers !== undefined) this.headers = new Headers(init.headers);
    if (init.body !== undefined && init.body !== null) this.__body = String(init.body);
    if (init.mode !== undefined) this.mode = String(init.mode);
    if (init.credentials !== undefined) this.credentials = String(init.credentials);
    if (init.redirect !== undefined) this.redirect = String(init.redirect);
    if (init.cache !== undefined) this.cache = String(init.cache);
    this.bodyUsed = false;
    if ((this.method === 'GET' || this.method === 'HEAD') && this.__body != null)
      throw new TypeError("Request with GET/HEAD method cannot have body.");
  }
  Request.prototype.clone = function() {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed.");
    var r = new Request(this); r.__body = this.__body; r.bodyUsed = false; return r;
  };
  Request.prototype.text = bodyMixin.text;
  Request.prototype.json = bodyMixin.json;
  Request.prototype.arrayBuffer = bodyMixin.arrayBuffer;
  globalThis.Request = Request;

  // ---- Response ----
  function Response(body, init) {
    init = init || {};
    var status = (init.status !== undefined) ? (init.status | 0) : 200;
    if (status < 200 || status > 599) throw new RangeError("Response status " + status + " out of range");
    this.status = status;
    this.statusText = (init.statusText !== undefined) ? String(init.statusText) : "";
    this.ok = this.status >= 200 && this.status < 300;
    this.type = "default"; this.url = ""; this.redirected = false;
    this.headers = new Headers(init.headers);
    this.__body = (body != null) ? String(body) : null;
    this.bodyUsed = false;
  }
  Response.prototype.text = bodyMixin.text;
  Response.prototype.json = bodyMixin.json;
  Response.prototype.arrayBuffer = bodyMixin.arrayBuffer;
  Response.prototype.clone = function() {
    if (this.bodyUsed) throw new TypeError("Body has already been consumed.");
    var r = Object.create(Response.prototype);
    r.status = this.status; r.statusText = this.statusText; r.ok = this.ok;
    r.type = this.type; r.url = this.url; r.redirected = this.redirected;
    r.headers = new Headers(this.headers); r.__body = this.__body; r.bodyUsed = false;
    return r;
  };
  Response.error = function() {
    var r = new Response(null, { status: 200 });
    r.status = 0; r.ok = false; r.type = "error"; return r;
  };
  Response.redirect = function(url, status) {
    status = (status === undefined) ? 302 : (status | 0);
    if ([301, 302, 303, 307, 308].indexOf(status) === -1) throw new RangeError("Invalid redirect status " + status);
    var r = new Response(null, { status: status });
    r.headers.set("location", String(url));
    return r;
  };
  Response.json = function(data, init) {
    var r = new Response(JSON.stringify(data), init);
    if (!r.headers.has("content-type")) r.headers.set("content-type", "application/json");
    return r;
  };
  globalThis.Response = Response;

  // Build a Response from the native __fetch outcome (network fields set).
  function responseFromOutcome(o) {
    var r = new Response(o.body != null ? o.body : "", { status: o.status || 200, statusText: o.statusText || "", headers: o.headers });
    r.type = o.type || "default"; r.url = o.url || ""; return r;
  }
  function headersFlat(h) {
    var flat = [];
    for (var i = 0; i < h._h.length; i++) { flat.push(h._h[i][0]); flat.push(h._h[i][1]); }
    return flat.join("\n");
  }

  globalThis.fetch = function(input, init) {
    var req;
    try { req = new Request(input, init); } catch (e) { return Promise.reject(e); }
    if (!req.headers.has("accept")) req.headers.append("accept", "*/*");
    return new Promise(function(resolve, reject) {
      var o;
      try { o = JSON.parse(__fetch(req.method, req.url, headersFlat(req.headers), req.__body != null ? req.__body : "")); }
      catch (e) { reject(new TypeError("Failed to fetch")); return; }
      if (o.networkError) { reject(new TypeError("Failed to fetch")); return; }
      resolve(responseFromOutcome(o));
    });
  };
})();
"#;
