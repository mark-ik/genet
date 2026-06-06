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
    /// The HTTP cache mode name (`default` / `no-store` / `reload` / `no-cache` /
    /// `force-cache` / `only-if-cached`); the host maps it to its cache engine.
    pub cache: String,
    /// The redirect mode name (`follow` / `error` / `manual`); the host maps it to
    /// its redirect handling.
    pub redirect: String,
    /// The request mode name (`cors` / `no-cors` / `same-origin` / `navigate`); the
    /// host maps it to its CORS / response-tainting model.
    pub mode: String,
    /// The request's referrer URL (the initiator document), or empty for none; the
    /// host derives the `Referer` header from it per [`Self::referrer_policy`].
    pub referrer: String,
    /// The referrer policy name (`` / `no-referrer` / `origin` / `unsafe-url` / …);
    /// the host maps it to its referrer engine.
    pub referrer_policy: String,
    /// The credentials mode name (`omit` / `same-origin` / `include`); the host maps
    /// it to whether cookies/auth travel with the request.
    pub credentials: String,
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
    /// At least one redirect was followed (drives `Response.redirected`).
    pub redirected: bool,
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
            redirected: false,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

/// The host seam: a host (the WPT runner, or Mere's content actor) implements it
/// over a real network engine. Install with `Runtime::set_fetch_handler`.
///
/// Two shapes. A **synchronous** host implements only [`FetchHandler::fetch`] and
/// answers in place (the runner's offline mock, a `block_on` over netfetcher); the
/// JS `fetch()` Promise resolves in the same tick. An **async / deferred** host
/// overrides [`FetchHandler::start`], spawns the work off-thread, returns `None`,
/// and later drives [`Runtime::settle_fetch`] / [`Runtime::fail_fetch`] when the
/// reply arrives. The deferred handler owns its own delivery channel (it is the
/// actor-mailbox seam: a send into the script actor's inbox), so the runtime stays
/// network-free and the engine stays `!Send`. Deferred delivery is what makes
/// mid-flight abort and streaming response bodies possible.
pub trait FetchHandler {
    /// Begin a fetch. The default bridges to [`fetch`](FetchHandler::fetch) and
    /// answers **inline**: returning `Some(outcome)` resolves the JS Promise in the
    /// same tick (no Rust-to-JS re-entry, no pump). A deferred host spawns the work,
    /// returns `None` (Promise left pending), and settles later by `id`.
    fn start(&self, _id: u64, request: FetchRequest) -> Option<FetchOutcome> {
        Some(self.fetch(request))
    }
    /// Synchronous convenience: a host with the answer in hand implements only this.
    fn fetch(&self, _request: FetchRequest) -> FetchOutcome {
        FetchOutcome::network_error()
    }
    /// Cancel an in-flight deferred request (from `AbortController.abort()`). The
    /// default is a no-op (synchronous hosts have nothing in flight).
    fn cancel(&self, _id: u64) {}
}

/// Clone the installed handler out from under the `HostState` borrow, so the
/// handler call holds no borrow (it must not be live if the handler re-enters a
/// native sink). `None` = no handler installed.
fn host_handler<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Option<std::rc::Rc<dyn FetchHandler>> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let h = cell.borrow().fetch.clone();
    h
}

/// `__fetch_start(id, method, url, headers, body)` — start a fetch for Promise
/// `id`. `headers` is a newline-delimited `k,v,k,v` list; `body` is the binary
/// string (empty = no body). Returns the JSON outcome string when the host
/// answered inline (sync), or `""` when the fetch is deferred (the JS bootstrap
/// leaves the Promise pending for a later `__fetchSettle` / `__fetchFail`). With no
/// handler installed, every fetch is an inline network error.
pub(crate) struct FetchStart;

impl<E: ScriptEngine> NativeFn<E> for FetchStart {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let id = cx.value_to_string(&a0)?.parse::<u64>().unwrap_or(0);
        let a1 = cx.arg(1);
        let method = cx.value_to_string(&a1)?;
        let a2 = cx.arg(2);
        let url = cx.value_to_string(&a2)?;
        let a3 = cx.arg(3);
        let headers_flat = cx.value_to_string(&a3)?;
        let a4 = cx.arg(4);
        let body_str = cx.value_to_string(&a4)?;
        let a5 = cx.arg(5);
        let cache = cx.value_to_string(&a5)?;
        let a6 = cx.arg(6);
        let redirect = cx.value_to_string(&a6)?;
        let a7 = cx.arg(7);
        let mode = cx.value_to_string(&a7)?;
        let a8 = cx.arg(8);
        let referrer = cx.value_to_string(&a8)?;
        let a9 = cx.arg(9);
        let referrer_policy = cx.value_to_string(&a9)?;
        let a10 = cx.arg(10);
        let credentials = cx.value_to_string(&a10)?;

        let headers = parse_flat_headers(&headers_flat);
        // The body crosses as a lossless "binary string": each JS char code (0-255)
        // is one byte. `char as u8` recovers the byte (every char is <= 0xFF).
        let body = (!body_str.is_empty()).then(|| body_str.chars().map(|c| c as u8).collect::<Vec<u8>>());
        let request = FetchRequest {
            method,
            url,
            headers,
            body,
            cache,
            redirect,
            mode,
            referrer,
            referrer_policy,
            credentials,
        };

        // Clone the handler before calling it (no borrow held across `start`).
        let outcome = match host_handler::<E>(cx) {
            Some(handler) => handler.start(id, request),
            None => Some(FetchOutcome::network_error()),
        };
        match outcome {
            Some(o) => cx.make_string(&encode_outcome(&o)), // inline (sync) answer
            None => cx.make_string(""),                     // deferred: settle later
        }
    }
}

/// `__fetch_abort(id)` — relay `AbortController.abort()` to the host so it can
/// cancel the in-flight deferred request. Tolerant of unknown / already-settled ids.
pub(crate) struct FetchAbort;

impl<E: ScriptEngine> NativeFn<E> for FetchAbort {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let id = cx.value_to_string(&a0)?.parse::<u64>().unwrap_or(0);
        if let Some(handler) = host_handler::<E>(cx) {
            handler.cancel(id);
        }
        Ok(cx.undefined())
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

/// `__url_parse(input, base)` — parse `input` (optionally against a non-empty
/// `base`) and return its WHATWG components as JSON, or `""` on failure. Backs the
/// JS `URL` constructor.
pub(crate) struct UrlParse;

impl<E: ScriptEngine> NativeFn<E> for UrlParse {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let input = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let base = cx.value_to_string(&a1)?;
        let parsed = if base.is_empty() {
            url::Url::parse(&input)
        } else {
            url::Url::parse(&base).and_then(|b| b.join(&input))
        };
        cx.make_string(&parsed.map(|u| url_components_json(&u)).unwrap_or_default())
    }
}

/// `__url_with(href, part, value)` — parse `href`, apply the WHATWG setter for
/// `part`, and return the new components as JSON (or `""` on failure). Backs the JS
/// `URL` component setters via the `url` crate.
pub(crate) struct UrlWith;

impl<E: ScriptEngine> NativeFn<E> for UrlWith {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let href = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let part = cx.value_to_string(&a1)?;
        let a2 = cx.arg(2);
        let value = cx.value_to_string(&a2)?;

        let result = url::Url::parse(&href).ok().and_then(|mut u| {
            match part.as_str() {
                "href" => return url::Url::parse(&value).ok().map(|u| url_components_json(&u)),
                "protocol" => {
                    let _ = u.set_scheme(value.trim_end_matches(':'));
                }
                "username" => {
                    let _ = u.set_username(&value);
                }
                "password" => {
                    let _ = u.set_password((!value.is_empty()).then_some(value.as_str()));
                }
                "hostname" => {
                    let _ = u.set_host((!value.is_empty()).then_some(value.as_str()));
                }
                "host" => {
                    let (h, p) = value.split_once(':').unwrap_or((value.as_str(), ""));
                    let _ = u.set_host((!h.is_empty()).then_some(h));
                    let _ = u.set_port(p.parse().ok());
                }
                "port" => {
                    let _ = u.set_port(value.parse().ok());
                }
                "pathname" => u.set_path(&value),
                "search" => u.set_query((!value.is_empty()).then_some(value.trim_start_matches('?'))),
                "hash" => u.set_fragment((!value.is_empty()).then_some(value.trim_start_matches('#'))),
                _ => return None,
            }
            Some(url_components_json(&u))
        });
        cx.make_string(&result.unwrap_or_default())
    }
}

/// The WHATWG URL components of `u` as a JSON object (the shape the JS `URL`
/// reads): `href`, `protocol` (scheme + `:`), `username`, `password`, `host`
/// (host + `:port`), `hostname`, `port`, `origin`, `pathname`, `search` (with a
/// leading `?`), `hash` (with a leading `#`).
fn url_components_json(u: &url::Url) -> String {
    let host = match (u.host_str(), u.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => String::new(),
    };
    let mut s = String::new();
    s.push('{');
    s.push_str("\"href\":");
    push_json_str(&mut s, u.as_str());
    s.push_str(",\"protocol\":");
    push_json_str(&mut s, &format!("{}:", u.scheme()));
    s.push_str(",\"username\":");
    push_json_str(&mut s, u.username());
    s.push_str(",\"password\":");
    push_json_str(&mut s, u.password().unwrap_or(""));
    s.push_str(",\"host\":");
    push_json_str(&mut s, &host);
    s.push_str(",\"hostname\":");
    push_json_str(&mut s, u.host_str().unwrap_or(""));
    s.push_str(",\"port\":");
    push_json_str(&mut s, &u.port().map(|p| p.to_string()).unwrap_or_default());
    s.push_str(",\"origin\":");
    push_json_str(&mut s, &u.origin().ascii_serialization());
    s.push_str(",\"pathname\":");
    push_json_str(&mut s, u.path());
    s.push_str(",\"search\":");
    push_json_str(&mut s, &u.query().map(|q| format!("?{q}")).unwrap_or_default());
    s.push_str(",\"hash\":");
    push_json_str(&mut s, &u.fragment().map(|f| format!("#{f}")).unwrap_or_default());
    s.push('}');
    s
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
/// dep). The body crosses losslessly as a "binary string": each byte becomes a
/// char (code point 0-255), which `push_json_str` escapes safely and the bootstrap
/// maps back to bytes — so binary bodies survive intact.
pub(crate) fn encode_outcome(o: &FetchOutcome) -> String {
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
    s.push_str(&format!(",\"redirected\":{}", o.redirected));
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
    let body_bin: String = o.body.iter().map(|&b| b as char).collect();
    push_json_str(&mut s, &body_bin);
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

/// Install the deferred fetch sinks (`__fetch_start` / `__fetch_abort`) and the
/// `fetch()` / `Request` / `Response` / `Headers` bootstrap.
pub(crate) fn install_fetch_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<FetchStart>("__fetch_start", 11)?;
    engine.set_function::<FetchAbort>("__fetch_abort", 1)?;
    engine.set_function::<ResolveUrl>("__resolve_url", 1)?;
    engine.set_function::<UrlParse>("__url_parse", 2)?;
    engine.set_function::<UrlWith>("__url_with", 3)?;
    engine.eval(FETCH_BOOTSTRAP)?;
    Ok(())
}

/// The Fetch API JS surface: `Headers` (with validation + sorted iteration +
/// getSetCookie), `TextEncoder` / `TextDecoder`, `URLSearchParams`, `Blob` /
/// `File`, `FormData`, a buffered `ReadableStream` (+ default reader), `Request`,
/// `Response` (+ `error`/`redirect`/`json` statics, `body` as a stream), a shared
/// body mixin (`text`/`json`/`arrayBuffer`/`blob`/`formData`) with WHATWG body
/// extraction (string / URLSearchParams / Blob / FormData / buffers / stream set
/// the right Content-Type), and `fetch()` over the `__fetch` sink. Bodies cross
/// that sink as a lossless binary string, so binary request / response bodies are
/// exact. `formData()` parses both urlencoded and multipart bodies. Streams are a
/// buffered model (`ReadableStream` / `WritableStream` / `TransformStream` +
/// `pipeTo` / `pipeThrough`); still missing byte (BYOB) readers, genuinely async
/// producers, and strict rejection of malformed multipart.
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
    // Normalize: strip leading/trailing HTTP whitespace (tab/LF/CR/space), then a
    // valid value has no interior NUL/CR/LF.
    v = String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
    if (/[\r\n\0]/.test(v)) throw new TypeError("Invalid header value");
    return v;
  }

  // ---- Header guards (WHATWG): request / request-no-cors / response / immutable. ----
  var FORBIDDEN_REQUEST = {
    'accept-charset': 1, 'accept-encoding': 1, 'access-control-request-headers': 1,
    'access-control-request-method': 1, 'connection': 1, 'content-length': 1,
    'cookie': 1, 'cookie2': 1, 'date': 1, 'dnt': 1, 'expect': 1, 'host': 1,
    'keep-alive': 1, 'origin': 1, 'permissions-policy': 1, 'referer': 1,
    'set-cookie': 1, 'te': 1, 'trailer': 1, 'transfer-encoding': 1, 'upgrade': 1, 'via': 1
  };
  var METHOD_OVERRIDE = { 'x-http-method': 1, 'x-http-method-override': 1, 'x-method-override': 1 };
  // name + value are already lower-cased name / normalized value.
  function isForbiddenRequestHeader(name, value) {
    if (FORBIDDEN_REQUEST[name] || /^proxy-/.test(name) || /^sec-/.test(name)) return true;
    if (METHOD_OVERRIDE[name]) {
      var toks = String(value).split(',');
      for (var i = 0; i < toks.length; i++) {
        var t = toks[i].replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "").toLowerCase();
        if (t === 'connect' || t === 'trace' || t === 'track') return true;
      }
    }
    return false;
  }
  function isForbiddenResponseHeader(name) {
    return name === 'set-cookie' || name === 'set-cookie2';
  }
  // A CORS-unsafe request-header byte: < 0x20 (except HT), or one of "():<>?@[\]{} or 0x7F.
  var CORS_UNSAFE_RE = /[\x00-\x08\x0a-\x1f"():<>?@\[\\\]{}\x7f]/;
  function isNoCorsSafelisted(name, value) {
    if (name === 'accept' || name === 'accept-language' || name === 'content-language') {
      return value.length <= 128 && !CORS_UNSAFE_RE.test(value);
    }
    if (name === 'content-type') {
      if (value.length > 128) return false;
      var mime = value.split(';')[0].replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "").toLowerCase();
      return mime === 'application/x-www-form-urlencoded' || mime === 'multipart/form-data' || mime === 'text/plain';
    }
    return false;
  }
  // Whether `guard` permits writing (name, value). `immutable` throws; the others
  // silently drop a disallowed header (the WHATWG "fill"/append behaviour).
  function guardAllows(guard, name, value) {
    switch (guard) {
      case 'immutable': throw new TypeError("Headers are immutable");
      case 'request': return !isForbiddenRequestHeader(name, value);
      case 'request-no-cors': return isNoCorsSafelisted(name, value);
      case 'response': return !isForbiddenResponseHeader(name);
      default: return true;
    }
  }

  // %IteratorPrototype% (two protos up from an array iterator), so our iterators
  // sit on the right chain for `checkIteratorProperties`-style WPT assertions.
  var IteratorProto = (hasSym && [][Symbol.iterator])
    ? Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()))
    : Object.prototype;
  // Build an iterator whose own prototype carries `next` (configurable/enumerable/
  // writable) and chains to %IteratorPrototype%.
  function makeIter(nextFn) {
    var proto = Object.create(IteratorProto);
    proto.next = nextFn;
    if (hasSym) proto[Symbol.iterator] = function() { return this; };
    return Object.create(proto);
  }
  // Snapshot iterator over a fixed array (URLSearchParams / FormData).
  function makeIterator(arr) {
    var i = 0;
    return makeIter(function() {
      return i < arr.length ? { value: arr[i++], done: false } : { value: undefined, done: true };
    });
  }
  // Live iterator over a Headers object: re-reads the sorted list each step, so
  // appends/deletes during iteration are observed (per the Headers iteration tests).
  // kind 0 = key, 1 = value, 2 = entry.
  function headersIter(headers, kind) {
    var i = 0;
    return makeIter(function() {
      var s = headers._sorted();
      if (i >= s.length) return { value: undefined, done: true };
      var p = s[i++];
      var v = kind === 0 ? p[0] : kind === 1 ? p[1] : [p[0], p[1]];
      return { value: v, done: false };
    });
  }

  function Headers(init) {
    this._h = [];
    if (init === undefined) return; // new Headers() / new Headers(undefined): empty
    if (init === null || typeof init !== 'object') throw new TypeError("Invalid HeadersInit");
    if (init instanceof Headers) {
      for (var i = 0; i < init._h.length; i++) this._h.push([init._h[i][0], init._h[i][1]]);
    } else if (Array.isArray(init) || (hasSym && typeof init[Symbol.iterator] === 'function')) {
      // sequence<sequence<ByteString>>: each entry is a [name, value] pair.
      var seq = Array.isArray(init) ? init : Array.from(init);
      for (var j = 0; j < seq.length; j++) {
        if (seq[j] == null || seq[j].length !== 2) throw new TypeError("Invalid header entry");
        this.append(seq[j][0], seq[j][1]);
      }
    } else {
      // record<ByteString, ByteString>: own enumerable keys, in order (the
      // Symbol.iterator probe above + Object.keys here match WebIDL trap ordering).
      var keys = Object.keys(init);
      for (var k = 0; k < keys.length; k++) this.append(keys[k], init[keys[k]]);
    }
  }
  Headers.prototype.append = function(n, v) {
    n = checkName(n); v = checkValue(v);
    // For request-no-cors the safelist check runs against the *combined* value.
    var check = v;
    if (this._guard === 'request-no-cors') { var cur = this.get(n); if (cur !== null) check = cur + ", " + v; }
    if (!guardAllows(this._guard || 'none', n, check)) return;
    this._h.push([n, v]);
  };
  Headers.prototype.set = function(n, v) {
    n = checkName(n); v = checkValue(v);
    if (!guardAllows(this._guard || 'none', n, v)) return;
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
    if (!guardAllows(this._guard || 'none', n, '')) return;
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
  Headers.prototype.entries = function() { return headersIter(this, 2); };
  Headers.prototype.keys = function() { return headersIter(this, 0); };
  Headers.prototype.values = function() { return headersIter(this, 1); };
  if (hasSym) Headers.prototype[Symbol.iterator] = Headers.prototype.entries;
  globalThis.Headers = Headers;

  // ---- Body mixin: bytes-backed, single-use. ----
  // The internal body is raw bytes (`__bytes`, a Uint8Array or null). Every
  // accessor derives from it, so binary bodies are exact: text/json UTF-8-decode,
  // arrayBuffer/bytes/blob hand back the bytes. takeBytes is *synchronous* (the
  // disturb/lock check + the bytes); accessors then resolve a promise with the
  // already-computed result. That matters: resolving with a primitive (text) is
  // immune to a poisoned `Object.prototype.then`, which a userland Promise.resolve
  // of the raw byte array would adopt (the broken-then WPT tests).
  // Permanently lock + disturb a body stream: consuming a body acquires a reader
  // and never releases it, so `body.getReader()` afterwards throws (the
  // disturbed-5 tests). The sentinel is not a real reader, so releaseLock can't
  // clear it.
  function lockBody(s) { s._disturbed = true; if (!s._reader) s._reader = { __sentinel: true }; }
  // Drain a (buffered) body stream to bytes, validating every chunk is a view
  // (a bad chunk -> TypeError, per the bad-chunk tests).
  function drainStreamSync(s) {
    var parts = s._chunks.slice(); s._chunks = [];
    var total = 0, i, p;
    for (i = 0; i < parts.length; i++) {
      p = parts[i];
      if (!ArrayBuffer.isView(p)) throw new TypeError("ReadableStream chunk is not a Uint8Array");
      total += p.byteLength;
    }
    var all = new Uint8Array(total), off = 0;
    for (i = 0; i < parts.length; i++) {
      p = parts[i];
      all.set(new Uint8Array(p.buffer, p.byteOffset, p.byteLength), off);
      off += p.byteLength;
    }
    return all;
  }
  function takeBytes(self) {
    if (self.__stream && self.__stream.locked)
      throw new TypeError("Body is locked");
    if (self.bodyUsed || (self.__stream && self.__stream._disturbed))
      throw new TypeError("Body has already been consumed.");
    self.bodyUsed = true;
    var bytes;
    if (self.__bytes != null) {
      bytes = self.__bytes;                       // buffered body
    } else if (self.__stream) {
      bytes = drainStreamSync(self.__stream);     // live stream-backed body
      self.__bytes = bytes;
    } else {
      bytes = new Uint8Array(0);
    }
    if (self.__stream) lockBody(self.__stream);
    return bytes;
  }
  function settled(fn) { try { return Promise.resolve(fn()); } catch (e) { return Promise.reject(e); } }
  // The body's ReadableStream. A stream-backed body returns that very stream; a
  // buffered body lazily wraps its bytes (same stream each access, so identity and
  // lock state persist). Getting it does not disturb; reading does.
  function bodyStream(self) {
    if (self.__stream) return self.__stream;
    if (self.__bytes == null) return null;
    var bytes = self.__bytes;
    self.__stream = new ReadableStream({ start: function(c) { if (bytes.length) c.enqueue(bytes.slice(0)); c.close(); } });
    self.__stream._owner = self;
    if (self.bodyUsed) lockBody(self.__stream);
    return self.__stream;
  }
  // Copy a body for clone(): buffered bytes copy by reference; an already-closed
  // stream is tee'd from its snapshot. A live (still-arriving) stream cannot be
  // tee'd losslessly by the buffered tee, so cloning one throws rather than
  // silently truncating both copies (a real fan-out tee is a later refinement).
  function cloneBodyInto(src, dst) {
    if (src.__stream && src.__bytes == null && !src.__stream._closed) {
      throw new TypeError("Cannot clone a body with a streaming (in-flight) ReadableStream");
    }
    dst.__bytes = src.__bytes;
    if (src.__stream && src.__bytes == null) {
      var t = src.__stream.tee();
      src.__stream = t[0]; src.__stream._owner = src;
      dst.__stream = t[1]; dst.__stream._owner = dst;
    } else {
      dst.__stream = null;
    }
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
  // WHATWG "UTF-8 decode": strip a leading UTF-8 BOM, then run the UTF-8 decoder
  // replacing every ill-formed sequence with U+FFFD (overlong, out-of-range, lone
  // continuation, and truncated sequences all collapse to the replacement char).
  function utf8Decode(bytes) {
    var out = '', i = 0, n = bytes.length;
    if (n >= 3 && bytes[0] === 0xEF && bytes[1] === 0xBB && bytes[2] === 0xBF) i = 3;
    while (i < n) {
      var b = bytes[i++];
      if (b < 0x80) { out += String.fromCharCode(b); continue; }
      var needed, cp, lower = 0x80, upper = 0xBF;
      if (b >= 0xC2 && b <= 0xDF) { needed = 1; cp = b & 0x1F; }
      else if (b >= 0xE0 && b <= 0xEF) { needed = 2; cp = b & 0x0F; if (b === 0xE0) lower = 0xA0; else if (b === 0xED) upper = 0x9F; }
      else if (b >= 0xF0 && b <= 0xF4) { needed = 3; cp = b & 0x07; if (b === 0xF0) lower = 0x90; else if (b === 0xF4) upper = 0x8F; }
      else { out += '�'; continue; }
      var ok = true;
      for (var k = 0; k < needed; k++) {
        if (i >= n) { ok = false; break; }
        var nb = bytes[i];
        var lo = (k === 0) ? lower : 0x80, hi = (k === 0) ? upper : 0xBF;
        if (nb < lo || nb > hi) { ok = false; break; }
        cp = (cp << 6) | (nb & 0x3F); i++;
      }
      if (!ok) { out += '�'; continue; }
      if (cp <= 0xFFFF) out += String.fromCharCode(cp);
      else { cp -= 0x10000; out += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF)); }
    }
    return out;
  }

  // Body bytes cross the native __fetch sink as a lossless "binary string": each
  // char code (0-255) is one byte. These convert a Uint8Array to/from that form.
  function bytesToBinaryString(bytes) {
    var s = '', CH = 0x8000;
    for (var i = 0; i < bytes.length; i += CH) {
      s += String.fromCharCode.apply(null, bytes.subarray(i, i + CH));
    }
    return s;
  }
  function binaryStringToBytes(s) {
    var b = new Uint8Array(s.length);
    for (var i = 0; i < s.length; i++) b[i] = s.charCodeAt(i) & 0xFF;
    return b;
  }

  // ---- TextEncoder / TextDecoder (UTF-8) ----
  function TextEncoder() { this.encoding = 'utf-8'; }
  TextEncoder.prototype.encode = function(s) { return utf8Encode(s === undefined ? '' : String(s)); };
  TextEncoder.prototype.encodeInto = function(s, dest) {
    var b = utf8Encode(String(s)), n = Math.min(b.length, dest.length);
    for (var i = 0; i < n; i++) dest[i] = b[i];
    return { read: n, written: n };
  };
  globalThis.TextEncoder = TextEncoder;
  function TextDecoder(label, opts) {
    this.encoding = 'utf-8'; this.fatal = !!(opts && opts.fatal); this.ignoreBOM = !!(opts && opts.ignoreBOM);
  }
  TextDecoder.prototype.decode = function(input) {
    if (input == null) return '';
    var bytes;
    if (input instanceof ArrayBuffer) bytes = new Uint8Array(input);
    else if (ArrayBuffer.isView(input)) bytes = new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    else bytes = input;
    return utf8Decode(bytes);
  };
  globalThis.TextDecoder = TextDecoder;

  // ---- ReadableStream (fully-buffered model) ----
  // Bodies are already buffered (the __fetch sink returns the whole body), so a
  // stream is a queue of chunks the source enqueues in start()/pull(). Enough for
  // the fetch body-as-stream tests; true async streaming, byte (BYOB) readers,
  // and pipeTo/pipeThrough are deferred.
  function ReadableStream(source, strategy) {
    source = source || {};
    this._chunks = [];
    this._waiters = []; // parked read() promises {res, rej} for a live (incremental) stream
    this._closed = false;
    this._errored = false;
    this._error = undefined;
    this._reader = null;
    this._disturbed = false;
    this._owner = null;
    this._source = source;
    var self = this;
    this._controller = {
      enqueue: function(chunk) {
        if (self._closed || self._errored) throw new TypeError("Cannot enqueue on a closed stream");
        // Hand the chunk straight to a parked reader, else buffer it.
        if (self._waiters.length > 0) self._waiters.shift().res({ value: chunk, done: false });
        else self._chunks.push(chunk);
      },
      close: function() { self._closeStream(); },
      error: function(e) { self._errorStream(e); },
      get desiredSize() { return self._closed ? null : 1; }
    };
    if (typeof source.start === 'function') {
      try { source.start(this._controller); } catch (e) { this._errorStream(e); }
    }
  }
  // Close: wake every parked reader with done, and resolve the reader's `closed`.
  ReadableStream.prototype._closeStream = function() {
    if (this._closed || this._errored) return;
    this._closed = true;
    while (this._waiters.length) this._waiters.shift().res({ value: undefined, done: true });
    if (this._reader && this._reader._cRes) { this._reader._cRes(undefined); this._reader._cRes = null; }
  };
  ReadableStream.prototype._errorStream = function(e) {
    if (this._closed || this._errored) return;
    this._errored = true; this._error = e;
    while (this._waiters.length) this._waiters.shift().rej(e);
    if (this._reader && this._reader._cRej) { this._reader._cRej(e); this._reader._cRej = null; }
  };
  Object.defineProperty(ReadableStream.prototype, 'locked', { configurable: true, get: function() { return this._reader !== null; } });
  ReadableStream.prototype.getReader = function(opts) {
    // BYOB readers are not implemented; ignore the mode and hand back a default
    // reader so `getReader({mode:'byob'})` tests still read the bytes.
    if (this._reader) throw new TypeError("ReadableStream is already locked to a reader");
    var r = new ReadableStreamDefaultReader(this);
    this._reader = r;
    return r;
  };
  ReadableStream.prototype.cancel = function(reason) {
    this._disturbed = true; this._chunks = [];
    if (this._owner) this._owner.bodyUsed = true;
    this._closeStream(); // wake any parked readers with done
    if (typeof this._source.cancel === 'function') { try { this._source.cancel(reason); } catch (e) {} }
    return Promise.resolve(undefined);
  };
  ReadableStream.prototype.tee = function() {
    // Buffered: snapshot the remaining chunks into two independent streams.
    var chunks = this._chunks.slice();
    this._disturbed = true;
    function mk() {
      return new ReadableStream({ start: function(c) { for (var i = 0; i < chunks.length; i++) c.enqueue(chunks[i]); c.close(); } });
    }
    return [mk(), mk()];
  };
  if (hasSym) ReadableStream.prototype[Symbol.asyncIterator] = function() {
    var reader = this.getReader();
    return { next: function() { return reader.read(); }, 'return': function() { reader.releaseLock(); return Promise.resolve({ value: undefined, done: true }); } };
  };
  globalThis.ReadableStream = ReadableStream;

  function ReadableStreamDefaultReader(stream) {
    this._stream = stream;
    var self = this;
    this.closed = new Promise(function(res, rej) { self._cRes = res; self._cRej = rej; });
    if (stream._closed) { this._cRes(undefined); }
    else if (stream._errored) { this._cRej(stream._error); }
  }
  ReadableStreamDefaultReader.prototype.read = function() {
    var s = this._stream;
    if (!s) return Promise.reject(new TypeError("Reader has been released"));
    s._disturbed = true;
    if (s._owner) s._owner.bodyUsed = true;
    if (s._chunks.length > 0) return Promise.resolve({ value: s._chunks.shift(), done: false });
    if (!s._closed && !s._errored && typeof s._source.pull === 'function') {
      try { s._source.pull(s._controller); } catch (e) { s._errorStream(e); }
      if (s._chunks.length > 0) return Promise.resolve({ value: s._chunks.shift(), done: false });
    }
    if (s._errored) { if (this._cRej) { this._cRej(s._error); this._cRej = null; } return Promise.reject(s._error); }
    if (s._closed) { if (this._cRes) { this._cRes(undefined); this._cRes = null; } return Promise.resolve({ value: undefined, done: true }); }
    // Live stream, nothing buffered yet: park until enqueue / close / error.
    return new Promise(function(res, rej) { s._waiters.push({ res: res, rej: rej }); });
  };
  ReadableStreamDefaultReader.prototype.releaseLock = function() {
    if (this._stream) { if (this._stream._reader === this) this._stream._reader = null; this._stream = null; }
  };
  ReadableStreamDefaultReader.prototype.cancel = function(reason) {
    if (this._stream) return this._stream.cancel(reason);
    return Promise.resolve(undefined);
  };
  globalThis.ReadableStreamDefaultReader = ReadableStreamDefaultReader;

  // ---- WritableStream (buffered model) ----
  function WritableStream(sink, strategy) {
    sink = sink || {};
    this._sink = sink;
    this._writer = null;
    this._state = 'writable';
    this._stored = undefined;
    var self = this;
    this._controller = { error: function(e) { if (self._state === 'writable') { self._state = 'errored'; self._stored = e; } }, signal: undefined };
    if (typeof sink.start === 'function') {
      try { sink.start(this._controller); } catch (e) { this._state = 'errored'; this._stored = e; }
    }
  }
  Object.defineProperty(WritableStream.prototype, 'locked', { configurable: true, get: function() { return this._writer !== null; } });
  WritableStream.prototype.getWriter = function() {
    if (this._writer) throw new TypeError("WritableStream is already locked to a writer");
    var w = new WritableStreamDefaultWriter(this);
    this._writer = w;
    return w;
  };
  WritableStream.prototype.abort = function(reason) {
    if (this._state === 'writable') { this._state = 'errored'; this._stored = reason; if (typeof this._sink.abort === 'function') { try { this._sink.abort(reason); } catch (e) {} } }
    return Promise.resolve(undefined);
  };
  WritableStream.prototype.close = function() {
    if (this._state === 'writable') { this._state = 'closed'; if (typeof this._sink.close === 'function') { try { this._sink.close(); } catch (e) {} } }
    return Promise.resolve(undefined);
  };
  globalThis.WritableStream = WritableStream;

  function WritableStreamDefaultWriter(stream) {
    this._stream = stream;
    this.closed = Promise.resolve(undefined);
    this.ready = Promise.resolve(undefined);
    this.desiredSize = 1;
  }
  WritableStreamDefaultWriter.prototype.write = function(chunk) {
    var s = this._stream;
    if (!s) return Promise.reject(new TypeError("Writer has been released"));
    if (s._state === 'errored') return Promise.reject(s._stored);
    if (typeof s._sink.write === 'function') {
      try { return Promise.resolve(s._sink.write(chunk, s._controller)); } catch (e) { return Promise.reject(e); }
    }
    return Promise.resolve(undefined);
  };
  WritableStreamDefaultWriter.prototype.close = function() { return this._stream ? this._stream.close() : Promise.resolve(undefined); };
  WritableStreamDefaultWriter.prototype.abort = function(reason) { return this._stream ? this._stream.abort(reason) : Promise.resolve(undefined); };
  WritableStreamDefaultWriter.prototype.releaseLock = function() { if (this._stream) { if (this._stream._writer === this) this._stream._writer = null; this._stream = null; } };
  globalThis.WritableStreamDefaultWriter = WritableStreamDefaultWriter;

  // pipeTo: lock + disturb the source synchronously (the by-pipe tests check
  // bodyUsed right after the call), then pump chunks to the writable. The pump is
  // best-effort over the buffered model.
  ReadableStream.prototype.pipeTo = function(dest, options) {
    if (this.locked) return Promise.reject(new TypeError("ReadableStream is locked"));
    if (!dest || dest.locked) return Promise.reject(new TypeError("WritableStream is locked"));
    var reader = this.getReader();
    this._disturbed = true;
    if (this._owner) this._owner.bodyUsed = true;
    var writer = dest.getWriter();
    return new Promise(function(resolve, reject) {
      function pump() {
        reader.read().then(function(r) {
          if (r.done) { writer.close().then(function() { resolve(undefined); }, function() { resolve(undefined); }); return; }
          Promise.resolve(writer.write(r.value)).then(pump, reject);
        }, reject);
      }
      pump();
    });
  };
  ReadableStream.prototype.pipeThrough = function(pair, options) {
    if (this.locked) throw new TypeError("ReadableStream is locked");
    if (!pair || !pair.writable || !pair.readable) throw new TypeError("pipeThrough needs a {writable, readable} pair");
    if (pair.writable.locked) throw new TypeError("WritableStream is locked");
    this.pipeTo(pair.writable, options);
    return pair.readable;
  };

  // ---- TransformStream (identity / transformer.transform) ----
  function TransformStream(transformer) {
    transformer = transformer || {};
    var self = this;
    this.readable = new ReadableStream({ start: function(c) { self.__rc = c; } });
    var controller = {
      enqueue: function(chunk) { if (self.__rc) self.__rc.enqueue(chunk); },
      terminate: function() { if (self.__rc) self.__rc.close(); },
      error: function(e) { self.__rc && self.__rc.error(e); }
    };
    this.writable = new WritableStream({
      write: function(chunk) {
        if (typeof transformer.transform === 'function') return transformer.transform(chunk, controller);
        controller.enqueue(chunk);
      },
      close: function() { if (typeof transformer.flush === 'function') transformer.flush(controller); if (self.__rc) self.__rc.close(); },
      abort: function() {}
    });
    if (typeof transformer.start === 'function') { try { transformer.start(controller); } catch (e) {} }
  }
  globalThis.TransformStream = TransformStream;

  // ---- URLSearchParams ----
  function uspEnc(s) {
    return encodeURIComponent(String(s)).replace(/%20/g, '+')
      .replace(/[!'()~]/g, function(c) { return '%' + c.charCodeAt(0).toString(16).toUpperCase(); });
  }
  function uspDec(s) { return decodeURIComponent(String(s).replace(/\+/g, ' ')); }
  function URLSearchParams(init) {
    this._l = [];
    if (init == null || init === '') return;
    if (init instanceof URLSearchParams) {
      for (var i = 0; i < init._l.length; i++) this._l.push([init._l[i][0], init._l[i][1]]);
    } else if (typeof init === 'string') {
      var q = init.charAt(0) === '?' ? init.slice(1) : init;
      if (q) {
        var pairs = q.split('&');
        for (var k = 0; k < pairs.length; k++) {
          if (!pairs[k]) continue;
          var eq = pairs[k].indexOf('=');
          var nm = eq < 0 ? pairs[k] : pairs[k].slice(0, eq);
          var vl = eq < 0 ? '' : pairs[k].slice(eq + 1);
          this._l.push([uspDec(nm), uspDec(vl)]);
        }
      }
    } else if (Array.isArray(init)) {
      for (var a = 0; a < init.length; a++) {
        if (init[a].length !== 2) throw new TypeError("Invalid URLSearchParams pair");
        this.append(init[a][0], init[a][1]);
      }
    } else { for (var key in init) this.append(key, init[key]); }
  }
  URLSearchParams.prototype.append = function(n, v) { this._l.push([String(n), String(v)]); };
  URLSearchParams.prototype['delete'] = function(n) { n = String(n); this._l = this._l.filter(function(p) { return p[0] !== n; }); };
  URLSearchParams.prototype.get = function(n) { n = String(n); for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === n) return this._l[i][1]; return null; };
  URLSearchParams.prototype.getAll = function(n) { n = String(n); var o = []; for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === n) o.push(this._l[i][1]); return o; };
  URLSearchParams.prototype.has = function(n) { n = String(n); for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === n) return true; return false; };
  URLSearchParams.prototype.set = function(n, v) {
    n = String(n); v = String(v); var done = false; var out = [];
    for (var i = 0; i < this._l.length; i++) {
      if (this._l[i][0] === n) { if (!done) { out.push([n, v]); done = true; } }
      else out.push(this._l[i]);
    }
    if (!done) out.push([n, v]);
    this._l = out;
  };
  URLSearchParams.prototype.sort = function() { this._l.sort(function(a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; }); };
  URLSearchParams.prototype.forEach = function(cb, thisArg) { for (var i = 0; i < this._l.length; i++) cb.call(thisArg, this._l[i][1], this._l[i][0], this); };
  URLSearchParams.prototype.entries = function() { return makeIterator(this._l.map(function(p) { return [p[0], p[1]]; })); };
  URLSearchParams.prototype.keys = function() { return makeIterator(this._l.map(function(p) { return p[0]; })); };
  URLSearchParams.prototype.values = function() { return makeIterator(this._l.map(function(p) { return p[1]; })); };
  URLSearchParams.prototype.toString = function() { var o = []; for (var i = 0; i < this._l.length; i++) o.push(uspEnc(this._l[i][0]) + '=' + uspEnc(this._l[i][1])); return o.join('&'); };
  Object.defineProperty(URLSearchParams.prototype, 'size', { configurable: true, get: function() { return this._l.length; } });
  if (hasSym) URLSearchParams.prototype[Symbol.iterator] = URLSearchParams.prototype.entries;
  // Replace the pair list from a query string (URL.search setter feeding back in).
  URLSearchParams.prototype._reload = function(search) { this._l = new URLSearchParams(search)._l; };
  // Mutators notify an owning URL (if any) so url.href reflects the change.
  ['append', 'set', 'delete', 'sort'].forEach(function(m) {
    var orig = URLSearchParams.prototype[m];
    URLSearchParams.prototype[m] = function() {
      var r = orig.apply(this, arguments);
      if (this._onchange) this._onchange(this.toString());
      return r;
    };
  });
  globalThis.URLSearchParams = URLSearchParams;

  // ---- URL (WHATWG, backed by the Rust url crate via __url_parse / __url_with) ----
  function URL(url, base) {
    var j = __url_parse(String(url), (base === undefined || base === null) ? "" : String(base));
    if (!j) throw new TypeError("Failed to construct 'URL': Invalid URL");
    this._c = JSON.parse(j);
    this._sp = null;
  }
  function urlComponent(name) {
    Object.defineProperty(URL.prototype, name, {
      configurable: true,
      get: function() { return this._c[name]; },
      set: function(v) {
        var j = __url_with(this._c.href, name, String(v));
        if (j) { this._c = JSON.parse(j); if (this._sp) this._sp._reload(this._c.search); }
      }
    });
  }
  ['protocol', 'username', 'password', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'].forEach(urlComponent);
  Object.defineProperty(URL.prototype, 'href', {
    configurable: true,
    get: function() { return this._c.href; },
    set: function(v) {
      var j = __url_parse(String(v), "");
      if (!j) throw new TypeError("Invalid URL");
      this._c = JSON.parse(j);
      if (this._sp) this._sp._reload(this._c.search);
    }
  });
  Object.defineProperty(URL.prototype, 'origin', { configurable: true, get: function() { return this._c.origin; } });
  Object.defineProperty(URL.prototype, 'searchParams', {
    configurable: true,
    get: function() {
      if (!this._sp) {
        var self = this;
        this._sp = new URLSearchParams(this._c.search);
        this._sp._onchange = function(q) { var j = __url_with(self._c.href, 'search', q ? '?' + q : ''); if (j) self._c = JSON.parse(j); };
      }
      return this._sp;
    }
  });
  URL.prototype.toString = function() { return this._c.href; };
  URL.prototype.toJSON = function() { return this._c.href; };
  URL.parse = function(url, base) { try { return new URL(url, base); } catch (e) { return null; } };
  URL.canParse = function(url, base) { return !!__url_parse(String(url), (base === undefined || base === null) ? "" : String(base)); };
  globalThis.URL = URL;

  // ---- Blob / File ----
  function toBytes(part) {
    if (part instanceof Blob) return part._b;
    if (part instanceof ArrayBuffer) return new Uint8Array(part.slice(0));
    if (ArrayBuffer.isView(part)) return new Uint8Array(part.buffer.slice(part.byteOffset, part.byteOffset + part.byteLength));
    return utf8Encode(String(part));
  }
  function Blob(parts, opts) {
    opts = opts || {};
    var chunks = [], total = 0;
    if (parts != null) {
      if (typeof parts !== 'object' || typeof parts.length !== 'number')
        throw new TypeError("Blob parts must be a sequence");
      for (var i = 0; i < parts.length; i++) { var b = toBytes(parts[i]); chunks.push(b); total += b.length; }
    }
    var all = new Uint8Array(total), off = 0;
    for (var j = 0; j < chunks.length; j++) { all.set(chunks[j], off); off += chunks[j].length; }
    this._b = all;
    this.size = total;
    var t = opts.type === undefined ? '' : String(opts.type);
    this.type = /[^ -~]/.test(t) ? '' : t.toLowerCase();
  }
  Blob.prototype.text = function() { var self = this; return Promise.resolve(utf8Decode(self._b)); };
  Blob.prototype.arrayBuffer = function() { return Promise.resolve(this._b.slice(0).buffer); };
  Blob.prototype.slice = function(start, end, type) {
    var s = this._b.slice(start || 0, end === undefined ? this._b.length : end);
    var b = new Blob([], { type: type || '' }); b._b = s; b.size = s.length; return b;
  };
  globalThis.Blob = Blob;

  function File(parts, name, opts) {
    if (name === undefined) throw new TypeError("File requires a name");
    Blob.call(this, parts, opts);
    this.name = String(name);
    this.lastModified = (opts && opts.lastModified != null) ? (opts.lastModified | 0) : 0;
  }
  File.prototype = Object.create(Blob.prototype);
  File.prototype.constructor = File;
  globalThis.File = File;

  // ---- FormData ----
  function FormData() { this._l = []; }
  FormData.prototype.append = function(name, value, filename) {
    name = String(name);
    if (value instanceof Blob) {
      var fv = value;
      if (filename !== undefined && !(value instanceof File)) {
        fv = new File([value], filename, { type: value.type });
      } else if (filename !== undefined) {
        fv = new File([value], filename, { type: value.type });
      }
      this._l.push([name, fv]);
    } else this._l.push([name, String(value)]);
  };
  FormData.prototype.set = function(name, value, filename) {
    name = String(name); this['delete'](name); this.append(name, value, filename);
  };
  FormData.prototype['delete'] = function(name) { name = String(name); this._l = this._l.filter(function(p) { return p[0] !== name; }); };
  FormData.prototype.get = function(name) { name = String(name); for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === name) return this._l[i][1]; return null; };
  FormData.prototype.getAll = function(name) { name = String(name); var o = []; for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === name) o.push(this._l[i][1]); return o; };
  FormData.prototype.has = function(name) { name = String(name); for (var i = 0; i < this._l.length; i++) if (this._l[i][0] === name) return true; return false; };
  FormData.prototype.forEach = function(cb, thisArg) { for (var i = 0; i < this._l.length; i++) cb.call(thisArg, this._l[i][1], this._l[i][0], this); };
  FormData.prototype.entries = function() { return makeIterator(this._l.map(function(p) { return [p[0], p[1]]; })); };
  FormData.prototype.keys = function() { return makeIterator(this._l.map(function(p) { return p[0]; })); };
  FormData.prototype.values = function() { return makeIterator(this._l.map(function(p) { return p[1]; })); };
  if (hasSym) FormData.prototype[Symbol.iterator] = FormData.prototype.entries;
  globalThis.FormData = FormData;

  // Serialize a FormData to a multipart/form-data body + content-type. File field
  // content is spliced as text here (the one remaining lossy spot for binary file
  // parts); text fields and filenames round-trip exactly.
  function serializeFormData(fd) {
    var boundary = '----serval' + Math.floor(Math.random() * 0x100000000).toString(16) + Math.floor(Math.random() * 0x100000000).toString(16);
    var s = '';
    for (var i = 0; i < fd._l.length; i++) {
      var name = fd._l[i][0], value = fd._l[i][1];
      s += '--' + boundary + '\r\n';
      if (value instanceof Blob) {
        var fn = (value instanceof File) ? value.name : 'blob';
        s += 'Content-Disposition: form-data; name="' + name + '"; filename="' + fn + '"\r\n';
        s += 'Content-Type: ' + (value.type || 'application/octet-stream') + '\r\n\r\n';
        s += utf8Decode(value._b) + '\r\n';
      } else {
        s += 'Content-Disposition: form-data; name="' + name + '"\r\n\r\n';
        s += value + '\r\n';
      }
    }
    s += '--' + boundary + '--\r\n';
    return { body: s, type: 'multipart/form-data; boundary=' + boundary };
  }

  // WHATWG "extract a body": returns { bytes: Uint8Array|null, type: string|null }.
  // Blob / ArrayBuffer / typed-array bodies carry their bytes directly, so binary
  // is exact end-to-end (the body crosses __fetch as a lossless binary string).
  function extractBody(v) {
    if (v == null) return { bytes: null, stream: null, type: null };
    if (typeof v === 'string') return { bytes: utf8Encode(v), stream: null, type: 'text/plain;charset=UTF-8' };
    if (v instanceof URLSearchParams) return { bytes: utf8Encode(v.toString()), stream: null, type: 'application/x-www-form-urlencoded;charset=UTF-8' };
    // Blob / buffers carry their bytes directly: no text round-trip, so binary is exact.
    if (v instanceof Blob) return { bytes: v._b.slice(0), stream: null, type: v.type ? v.type : null };
    if (v instanceof FormData) { var r = serializeFormData(v); return { bytes: utf8Encode(r.body), stream: null, type: r.type }; }
    if (v instanceof ReadableStream) {
      // A stream body stays a live stream (consumed lazily): a stream already
      // locked or disturbed is not a usable body (the from-stream tests).
      if (v.locked || v._disturbed) throw new TypeError("Body stream is already locked or disturbed");
      return { bytes: null, stream: v, type: null };
    }
    if (v instanceof ArrayBuffer) return { bytes: new Uint8Array(v.slice(0)), stream: null, type: null };
    if (ArrayBuffer.isView(v)) return { bytes: new Uint8Array(v.buffer.slice(v.byteOffset, v.byteOffset + v.byteLength)), stream: null, type: null };
    return { bytes: utf8Encode(String(v)), stream: null, type: 'text/plain;charset=UTF-8' };
  }

  // Parse a multipart/form-data body (already decoded to text) into a FormData.
  // Text fields round-trip exactly; a file part becomes a File (its content via
  // the UTF-8 text, the one lossy spot for binary parts).
  function parseMultipart(text, boundary) {
    var fd = new FormData();
    var segments = text.split("--" + boundary);
    for (var i = 0; i < segments.length; i++) {
      var part = segments[i];
      if (part === "" || /^--\r?\n?$/.test(part)) continue; // preamble / closing delim
      part = part.replace(/^\r\n/, "").replace(/\r\n$/, "");
      var sep = part.indexOf("\r\n\r\n");
      if (sep < 0) continue;
      var head = part.slice(0, sep), content = part.slice(sep + 4);
      var nameM = /name="([^"]*)"/i.exec(head);
      if (!nameM) continue;
      var fileM = /filename="([^"]*)"/i.exec(head);
      var ctM = /content-type:\s*([^\r\n]+)/i.exec(head);
      if (fileM) {
        fd.append(nameM[1], new File([utf8Encode(content)], fileM[1], { type: ctM ? ctM[1].replace(/\s+$/, "") : "" }));
      } else {
        fd.append(nameM[1], content);
      }
    }
    return fd;
  }
  // Parse a consumed body back to a FormData for `.formData()`:
  // application/x-www-form-urlencoded or multipart/form-data.
  function parseFormData(text, contentType) {
    var ct = String(contentType || '').toLowerCase();
    if (ct.indexOf('multipart/form-data') === 0) {
      var bM = /boundary=("?)([^";]+)\1/i.exec(String(contentType));
      if (!bM) throw new TypeError("multipart/form-data without boundary");
      return parseMultipart(text, bM[2]);
    }
    if (ct.indexOf('application/x-www-form-urlencoded') === 0 || ct === '') {
      var fd = new FormData();
      new URLSearchParams(text).forEach(function(val, key) { fd.append(key, val); });
      return fd;
    }
    throw new TypeError("Unsupported content-type for formData(): " + ct);
  }

  // A body is "live" when it is backed by a stream that has not closed yet (an
  // incremental network response fed by __fetchPushChunk). Buffered bodies and
  // already-closed streams use the synchronous takeBytes path, so their behaviour
  // (and the broken-then immunity of text/json) is unchanged.
  function isLive(self) { return self.__stream && self.__bytes == null && !self.__stream._closed; }
  function bodyGuardError(self) {
    // An errored (e.g. aborted) body rejects immediately with the stored reason,
    // ahead of the locked / consumed guards — so consuming an aborted response
    // rejects in the current microtask, and a second call rejects the same way.
    if (self.__stream && self.__stream._errored) return self.__stream._error;
    if (self.__stream && self.__stream.locked) return new TypeError("Body is locked");
    if (self.bodyUsed || (self.__stream && self.__stream._disturbed)) return new TypeError("Body has already been consumed.");
    return null;
  }
  // Drain a live stream to completion via its reader, returning Promise<Uint8Array>.
  function drainLive(self) {
    self.bodyUsed = true;
    var reader = self.__stream.getReader();
    var parts = [], total = 0;
    function pump() {
      return reader.read().then(function(r) {
        if (r.done) {
          var all = new Uint8Array(total), off = 0;
          for (var i = 0; i < parts.length; i++) { all.set(parts[i], off); off += parts[i].length; }
          return all;
        }
        if (!ArrayBuffer.isView(r.value)) throw new TypeError("ReadableStream chunk is not a Uint8Array");
        parts.push(r.value); total += r.value.length;
        return pump();
      });
    }
    return pump();
  }
  // text/json resolve with a primitive even on the live path (broken-then immune);
  // arrayBuffer/bytes/blob resolve with an object, as on the buffered path.
  var bodyMixin = {
    text: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      if (isLive(s)) return drainLive(s).then(function(b) { return utf8Decode(b); });
      return settled(function() { return utf8Decode(takeBytes(s)); });
    },
    json: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      if (isLive(s)) return drainLive(s).then(function(b) { return JSON.parse(utf8Decode(b)); });
      return settled(function() { return JSON.parse(utf8Decode(takeBytes(s))); });
    },
    arrayBuffer: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      if (isLive(s)) return drainLive(s).then(function(b) { return b.slice(0).buffer; });
      return settled(function() { return takeBytes(s).slice(0).buffer; });
    },
    bytes: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      if (isLive(s)) return drainLive(s).then(function(b) { return b.slice(0); });
      return settled(function() { return takeBytes(s).slice(0); });
    },
    blob: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      var ct = (s.headers && s.headers.get) ? s.headers.get('content-type') : null;
      if (isLive(s)) return drainLive(s).then(function(b) { return new Blob([b], { type: ct || '' }); });
      return settled(function() { return new Blob([takeBytes(s)], { type: ct || '' }); });
    },
    formData: function() {
      var s = this; var err = bodyGuardError(s); if (err) return Promise.reject(err);
      var ct = (s.headers && s.headers.get) ? s.headers.get('content-type') : '';
      if (isLive(s)) return drainLive(s).then(function(b) { return parseFormData(utf8Decode(b), ct); });
      return settled(function() { return parseFormData(utf8Decode(takeBytes(s)), ct); });
    }
  };

  // ---- AbortController / AbortSignal ----
  // AbortSignal is an EventTarget (the global EventTarget bootstrap supplies
  // addEventListener / dispatchEvent). It has no public constructor; controllers
  // and the statics mint one via makeSignal.
  function AbortSignal() { throw new TypeError("Illegal constructor"); }
  AbortSignal.prototype = Object.create(EventTarget.prototype);
  AbortSignal.prototype.constructor = AbortSignal;
  AbortSignal.prototype.throwIfAborted = function() { if (this.aborted) throw this.reason; };
  function makeSignal() {
    var s = Object.create(AbortSignal.prototype);
    EventTarget.call(s);
    s.aborted = false; s.reason = undefined; s.onabort = null;
    return s;
  }
  function abortReason(reason) {
    return reason !== undefined ? reason : new DOMException("signal is aborted without reason", "AbortError");
  }
  function signalAbort(signal, reason) {
    if (signal.aborted) return;
    signal.aborted = true;
    signal.reason = abortReason(reason);
    var ev = new Event('abort');
    if (typeof signal.onabort === 'function') { try { signal.onabort.call(signal, ev); } catch (e) {} }
    signal.dispatchEvent(ev);
  }
  // A fresh signal that follows its source signals: aborts (with the source's
  // reason) when any source aborts. A Request's signal and a clone's signal are
  // new dependent signals (WHATWG), never the same object as the input's.
  function dependentSignal(sources) {
    var s = makeSignal();
    for (var i = 0; i < sources.length; i++) {
      var src = sources[i];
      if (!src) continue;
      if (src.aborted) { s.aborted = true; s.reason = src.reason; break; }
      (function(src) { src.addEventListener('abort', function() { signalAbort(s, src.reason); }); })(src);
    }
    return s;
  }
  AbortSignal.abort = function(reason) { var s = makeSignal(); s.aborted = true; s.reason = abortReason(reason); return s; };
  AbortSignal.timeout = function(ms) {
    var s = makeSignal();
    setTimeout(function() { signalAbort(s, new DOMException("signal timed out", "TimeoutError")); }, ms);
    return s;
  };
  AbortSignal.any = function(signals) {
    var s = makeSignal();
    for (var i = 0; i < signals.length; i++) { if (signals[i].aborted) { s.aborted = true; s.reason = signals[i].reason; return s; } }
    for (var j = 0; j < signals.length; j++) {
      (function(src) { src.addEventListener('abort', function() { signalAbort(s, src.reason); }); })(signals[j]);
    }
    return s;
  };
  globalThis.AbortSignal = AbortSignal;

  function AbortController() { this.signal = makeSignal(); }
  AbortController.prototype.abort = function(reason) { signalAbort(this.signal, reason); };
  globalThis.AbortController = AbortController;

  // ---- Request init validation (WHATWG) ----
  var NORMALIZE_METHODS = { DELETE: 1, GET: 1, HEAD: 1, OPTIONS: 1, POST: 1, PUT: 1 };
  var FORBIDDEN_METHODS = { CONNECT: 1, TRACE: 1, TRACK: 1 };
  var SIMPLE_METHODS = { GET: 1, HEAD: 1, POST: 1 };
  function normalizeMethod(m) {
    m = String(m);
    if (!TOKEN_RE.test(m)) throw new TypeError("Invalid method: '" + m + "'");
    var up = m.toUpperCase();
    if (FORBIDDEN_METHODS[up]) throw new TypeError("Forbidden method: " + m);
    return NORMALIZE_METHODS[up] ? up : m;
  }
  var ENUMS = {
    mode: { 'same-origin': 1, 'no-cors': 1, 'cors': 1, 'navigate': 1 },
    credentials: { 'omit': 1, 'same-origin': 1, 'include': 1 },
    cache: { 'default': 1, 'no-store': 1, 'reload': 1, 'no-cache': 1, 'force-cache': 1, 'only-if-cached': 1 },
    redirect: { 'follow': 1, 'error': 1, 'manual': 1 },
    referrerPolicy: {
      '': 1, 'no-referrer': 1, 'no-referrer-when-downgrade': 1, 'same-origin': 1, 'origin': 1,
      'strict-origin': 1, 'origin-when-cross-origin': 1, 'strict-origin-when-cross-origin': 1, 'unsafe-url': 1
    }
  };
  function checkEnum(kind, v) {
    var s = String(v);
    if (!ENUMS[kind][s]) throw new TypeError("Invalid " + kind + ": '" + s + "'");
    return s;
  }

  // ---- Request ----
  function Request(input, init) {
    if (!(this instanceof Request)) throw new TypeError("Failed to construct 'Request': use 'new'");
    init = init || {};
    if (init.window !== undefined && init.window !== null) throw new TypeError("RequestInit window must be null");
    if (input instanceof Request) {
      this.url = input.url; this.method = input.method; this.headers = new Headers(input.headers);
      this.__bytes = input.__bytes; this.__stream = input.__stream || null; this.mode = input.mode; this.credentials = input.credentials;
      this.redirect = input.redirect; this.cache = input.cache; this.destination = input.destination;
      this.referrer = input.referrer; this.referrerPolicy = input.referrerPolicy;
      this.signal = input.signal;
    } else {
      // Resolve leniently (relative URLs resolve at fetch when there is no base),
      // then validate: a resolved absolute URL must not carry credentials; an
      // unresolvable URL is an error only when a real document base exists (with no
      // base, a relative URL legitimately stays unresolved).
      this.url = __resolve_url(String(input));
      var pj = __url_parse(this.url, "");
      if (pj) {
        var pc = JSON.parse(pj);
        if (pc.username || pc.password) throw new TypeError("Request URL cannot have credentials");
        this.url = pc.href;
      } else if (typeof location !== 'undefined' && location && location.href && location.href !== 'about:blank') {
        throw new TypeError("Failed to construct 'Request': invalid URL");
      }
      this.method = 'GET'; this.headers = new Headers(); this.__bytes = null; this.__stream = null;
      this.mode = 'cors'; this.credentials = 'same-origin'; this.redirect = 'follow'; this.cache = 'default'; this.destination = '';
      // Default referrer is the client (the document URL, resolved at fetch).
      this.referrer = 'about:client'; this.referrerPolicy = '';
      this.signal = makeSignal();
    }
    // The request's signal is a fresh dependent signal following the input
    // request's signal (if any) and init.signal (if given) — never a shared ref.
    var __sigSources = [];
    if (input instanceof Request) __sigSources.push(input.signal);
    if (init.signal !== undefined && init.signal !== null) __sigSources.push(init.signal);
    this.signal = dependentSignal(__sigSources);
    if (init.method !== undefined) this.method = normalizeMethod(init.method);
    if (init.mode !== undefined) { if (String(init.mode) === 'navigate') throw new TypeError("Cannot construct a Request with mode 'navigate'"); this.mode = checkEnum('mode', init.mode); }
    if (init.credentials !== undefined) this.credentials = checkEnum('credentials', init.credentials);
    if (init.cache !== undefined) this.cache = checkEnum('cache', init.cache);
    if (init.redirect !== undefined) this.redirect = checkEnum('redirect', init.redirect);
    if (init.referrerPolicy !== undefined) this.referrerPolicy = checkEnum('referrerPolicy', init.referrerPolicy);
    // referrer: "" = no referrer; "about:client" = default (the document); else a URL.
    if (init.referrer !== undefined) this.referrer = String(init.referrer);
    // no-cors restricts the method to GET/HEAD/POST.
    if (this.mode === 'no-cors' && !SIMPLE_METHODS[this.method]) throw new TypeError("Method '" + this.method + "' not allowed in no-cors mode");
    // only-if-cached requires same-origin mode.
    if (this.cache === 'only-if-cached' && this.mode !== 'same-origin') throw new TypeError("only-if-cached requires same-origin mode");
    if (init.headers !== undefined) this.headers = new Headers(init.headers);
    if (init.body !== undefined && init.body !== null) {
      // A ReadableStream body is an upload stream; it requires duplex: "half".
      if (init.body instanceof ReadableStream && init.duplex !== 'half')
        throw new TypeError("Request with a ReadableStream body requires 'duplex: \"half\"'");
      var eb = extractBody(init.body);
      this.__bytes = eb.bytes; this.__stream = eb.stream;
      if (this.__stream) this.__stream._owner = this;
      if (eb.type && !this.headers.has('content-type')) this.headers.set('content-type', eb.type);
    }
    // The request header guard (from the mode) drops forbidden / non-safelisted
    // headers and governs later append/set/delete.
    this.headers._guard = (this.mode === 'no-cors') ? 'request-no-cors' : 'request';
    var __g = this.headers._guard, __hs = this.headers;
    __hs._h = __hs._h.filter(function(p) {
      try { return guardAllows(__g, p[0], p[1]); } catch (e) { return false; }
    });
    this.bodyUsed = false;
    if ((this.method === 'GET' || this.method === 'HEAD') && (this.__bytes != null || this.__stream))
      throw new TypeError("Request with GET/HEAD method cannot have body.");
  }
  Request.prototype.clone = function() {
    if (this.bodyUsed || (this.__stream && this.__stream.locked)) throw new TypeError("Body is disturbed or locked.");
    var r = new Request(this); r.bodyUsed = false; cloneBodyInto(this, r); return r;
  };
  Request.prototype.text = bodyMixin.text;
  Request.prototype.json = bodyMixin.json;
  Request.prototype.arrayBuffer = bodyMixin.arrayBuffer;
  Request.prototype.bytes = bodyMixin.bytes;
  Request.prototype.blob = bodyMixin.blob;
  Request.prototype.formData = bodyMixin.formData;
  Object.defineProperty(Request.prototype, 'body', { configurable: true, get: function() { return bodyStream(this); } });
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
    // Response guard: a script-built Response cannot carry set-cookie / set-cookie2
    // (a network Response sets its headers with the guard bypassed; see
    // responseFromOutcome). Filter the init headers, then govern later writes.
    this.headers._h = this.headers._h.filter(function(p) { return !isForbiddenResponseHeader(p[0]); });
    this.headers._guard = 'response';
    this.__bytes = null; this.__stream = null;
    if (body != null) {
      var eb = extractBody(body);
      this.__bytes = eb.bytes; this.__stream = eb.stream;
      if (this.__stream) this.__stream._owner = this;
      if (eb.type && !this.headers.has('content-type')) this.headers.set('content-type', eb.type);
    }
    this.bodyUsed = false;
  }
  Response.prototype.text = bodyMixin.text;
  Response.prototype.json = bodyMixin.json;
  Response.prototype.arrayBuffer = bodyMixin.arrayBuffer;
  Response.prototype.bytes = bodyMixin.bytes;
  Response.prototype.blob = bodyMixin.blob;
  Response.prototype.formData = bodyMixin.formData;
  Object.defineProperty(Response.prototype, 'body', { configurable: true, get: function() { return bodyStream(this); } });
  Response.prototype.clone = function() {
    if (this.bodyUsed || (this.__stream && this.__stream.locked)) throw new TypeError("Body is disturbed or locked.");
    var r = Object.create(Response.prototype);
    r.status = this.status; r.statusText = this.statusText; r.ok = this.ok;
    r.type = this.type; r.url = this.url; r.redirected = this.redirected;
    r.headers = new Headers(this.headers); r.headers._guard = this.headers._guard; r.bodyUsed = false;
    cloneBodyInto(this, r);
    return r;
  };
  Response.error = function() {
    var r = new Response(null, { status: 200 });
    r.status = 0; r.ok = false; r.type = "error"; r.headers._guard = 'immutable'; return r;
  };
  Response.redirect = function(url, status) {
    status = (status === undefined) ? 302 : (status | 0);
    if ([301, 302, 303, 307, 308].indexOf(status) === -1) throw new RangeError("Invalid redirect status " + status);
    var r = new Response(null, { status: status });
    r.headers.set("location", String(url));
    return r;
  };
  Response.json = function(data, init) {
    init = init || {};
    var r = new Response(JSON.stringify(data), init);
    // application/json wins unless the caller's init explicitly set a type (the
    // string-body extraction defaults to text/plain, which must not stick here).
    if (!new Headers(init.headers).has("content-type")) r.headers.set("content-type", "application/json");
    return r;
  };
  globalThis.Response = Response;

  // Build a Response from the native __fetch outcome. The body crossed as a binary
  // string; decode it straight to bytes (bypassing extractBody, which would treat
  // it as text) so binary responses are exact.
  // A status-0 (opaqueredirect / opaque) filtered response cannot go through the
  // Response ctor, which rejects < 200. Build a 200 placeholder then override —
  // the same trick as Response.error. `o.status || 200` would also mis-map 0 to
  // 200 (0 is falsy), so route every outcome through here.
  function makeFilteredShell(o) {
    var st = (o.status == null) ? 200 : (o.status | 0);
    if (st >= 200 && st <= 599) {
      return new Response(null, { status: st, statusText: o.statusText || "" });
    }
    var r = new Response(null, { status: 200 });
    r.status = st; r.statusText = o.statusText || ""; r.ok = st >= 200 && st < 300;
    return r;
  }
  // A null-body status (WHATWG): the response's body is null regardless of any
  // bytes on the wire, so Response.body reads as null.
  function isNullBodyStatus(s) { return s === 204 || s === 205 || s === 304; }
  function responseFromOutcome(o) {
    var r = makeFilteredShell(o);
    // Network headers are set with the guard bypassed (a real response keeps
    // set-cookie, readable via getSetCookie); the guard only blocks later writes.
    r.headers = new Headers(o.headers); r.headers._guard = 'response';
    r.__bytes = isNullBodyStatus(r.status) ? null : binaryStringToBytes(o.body != null ? o.body : "");
    r.type = o.type || "default"; r.url = o.url || ""; r.redirected = !!o.redirected; return r;
  }
  function headersFlat(h) {
    var flat = [];
    for (var i = 0; i < h._h.length; i++) { flat.push(h._h[i][0]); flat.push(h._h[i][1]); }
    return flat.join("\n");
  }

  // ---- Deferred fetch registry ----
  // Each in-flight fetch() owns an entry keyed by a monotonic id. A synchronous
  // host answers inline (__fetch_start returns the outcome JSON, resolved in this
  // tick). A deferred host returns "" and the entry stays pending until Rust drives
  // __fetchSettle / __fetchFail (or __fetchPushChunk / __fetchClose for streams).
  // Single authority, delete-once: every terminal removes the entry exactly once.
  var __pending = Object.create(null);
  globalThis.__pending = __pending;          // Object.keys count drives the host's quiescence probe
  var __nextFetchId = 1;

  function settleEntry(e, o) {
    if (o.networkError) e.reject(new TypeError('Failed to fetch'));
    else e.resolve(responseFromOutcome(o));
  }

  globalThis.fetch = function(input, init) {
    var req;
    try { req = new Request(input, init); } catch (e) { return Promise.reject(e); }
    if (!req.headers.has("accept")) req.headers.append("accept", "*/*");
    if (!req.headers.has("accept-language")) req.headers.append("accept-language", "*");
    // A pre-aborted signal rejects synchronously with its reason and allocates no
    // id (preserves the immediate-reject ordering + reason identity).
    if (req.signal && req.signal.aborted) {
      var pre = abortReason(req.signal.reason);
      if (req.__stream && !req.__stream._disturbed) { try { req.__stream.cancel(pre); } catch (x) {} }
      return Promise.reject(pre);
    }
    var id = __nextFetchId++;
    return new Promise(function(resolve, reject) {
      var entry = { resolve: resolve, reject: reject, controller: null, settled: false, method: req.method };
      __pending[id] = entry;
      // Mid-flight abort: relay to the host (cancel the in-flight work) and reject
      // with the signal's reason. JS mints the reason once so the same instance
      // flows to the body and the rejection (promise_rejects_exactly).
      if (req.signal) req.signal.addEventListener('abort', function() {
        var e = __pending[id]; if (!e) return;
        delete __pending[id];
        var err = abortReason(req.signal.reason);
        try { __fetch_abort(id); } catch (x) {}
        // Mid-stream abort errors the in-flight body; pre-headers abort rejects the
        // Promise. (A settled streaming entry has a controller but no pending Promise.)
        if (e.controller) { try { e.controller.error(err); } catch (x) {} }
        if (!e.settled) { e.settled = true; e.reject(err); }
      });
      // Resolve the referrer: "about:client" (the default) / undefined -> the
      // document URL; "" -> no referrer; otherwise the given URL (resolved).
      var docHref = (typeof location !== 'undefined' && location && location.href) ? location.href : "";
      var referrer = req.referrer;
      if (referrer === undefined || referrer === 'about:client') referrer = docHref;
      else if (referrer === '') referrer = "";
      else { try { referrer = new URL(referrer, docHref || undefined).href; } catch (e) { referrer = ""; } }
      var inline = __fetch_start(id, req.method, req.url, headersFlat(req.headers),
                                 req.__bytes != null ? bytesToBinaryString(req.__bytes) : "",
                                 req.cache || "default", req.redirect || "follow",
                                 req.mode || "cors", referrer, req.referrerPolicy || "",
                                 req.credentials || "same-origin");
      if (inline) {
        // Synchronous host answered in this tick (today's path; one pump drains).
        var e = __pending[id];
        if (e && !e.settled) { e.settled = true; delete __pending[id]; settleEntry(e, JSON.parse(inline)); }
      }
    });
  };

  // Rust-invoked deferred terminals (eval'd from Runtime::settle_fetch / fail_fetch;
  // Rust cannot call a held JS function, so it evals these). Guard on presence so a
  // reply racing an abort is a no-op.
  globalThis.__fetchSettle = function(id, ojson) {
    var e = __pending[id]; if (!e || e.settled) return; e.settled = true; delete __pending[id];
    settleEntry(e, JSON.parse(ojson));
  };
  globalThis.__fetchFail = function(id, msg) {
    var e = __pending[id]; if (!e || e.settled) return; e.settled = true; delete __pending[id];
    e.reject(new TypeError(msg || 'Failed to fetch'));
  };

  // ---- Streaming deferred response (incremental body) ----
  // A deferred host can early-settle with status + headers, then feed the body as
  // it arrives. __fetchStartStream resolves the Promise with a Response whose body
  // is a LIVE stream; the entry STAYS in __pending (with its controller) so chunks
  // and close route to it; __fetchClose removes it.
  globalThis.__fetchStartStream = function(id, ojson) {
    var e = __pending[id]; if (!e || e.settled) return; e.settled = true;
    var o = JSON.parse(ojson);
    if (o.networkError) { delete __pending[id]; e.reject(new TypeError('Failed to fetch')); return; }
    var controller = null;
    var stream = new ReadableStream({ start: function(c) { controller = c; } });
    var r = makeFilteredShell(o);
    r.headers = new Headers(o.headers); r.headers._guard = 'response'; // network headers, guard bypassed
    r.type = o.type || "default"; r.url = o.url || ""; r.redirected = !!o.redirected;
    if (isNullBodyStatus(r.status) || e.method === 'HEAD') {
      // A null-body status (or a HEAD response) has no body; drop the (empty)
      // stream so .body is null. A trailing __fetchClose just removes the pending
      // entry (controller stays null).
      r.__bytes = null; r.__stream = null; e.controller = null;
    } else {
      r.__bytes = null; r.__stream = stream; stream._owner = r;
      e.controller = controller;
    }
    e.resolve(r);
  };
  globalThis.__fetchPushChunk = function(id, arr) {
    var e = __pending[id]; if (!e || !e.controller) return;
    var u8 = new Uint8Array(arr.length);
    for (var i = 0; i < arr.length; i++) u8[i] = arr[i] & 0xFF;
    try { e.controller.enqueue(u8); } catch (x) {}
  };
  globalThis.__fetchClose = function(id) {
    var e = __pending[id]; if (!e) return; delete __pending[id];
    if (e.controller) { try { e.controller.close(); } catch (x) {} }
  };
})();
"#;
