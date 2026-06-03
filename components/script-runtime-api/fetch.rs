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
/// the right Content-Type), and `fetch()` over the `__fetch` sink. Still missing
/// byte (BYOB) readers, `pipeTo` / `pipeThrough`, true async streaming,
/// multipart `formData()` parsing, and `AbortSignal`; binary bodies degrade
/// through the UTF-8 string sink.
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
    if (self.__stream && self.__stream.locked)
      return Promise.reject(new TypeError("Body is locked"));
    if (self.bodyUsed || (self.__stream && self.__stream._disturbed))
      return Promise.reject(new TypeError("Body has already been consumed."));
    self.bodyUsed = true;
    if (self.__stream) self.__stream._disturbed = true;
    return Promise.resolve(self.__body != null ? String(self.__body) : "");
  }
  // Lazily expose the body as a ReadableStream (the same one each access, so its
  // identity and lock state persist). Getting it does not disturb; reading does.
  function bodyStream(self) {
    if (self.__body == null) return null;
    if (!self.__stream) {
      var bytes = utf8Encode(self.__body);
      self.__stream = new ReadableStream({ start: function(c) { if (bytes.length) c.enqueue(bytes); c.close(); } });
      self.__stream._owner = self;
      if (self.bodyUsed) self.__stream._disturbed = true;
    }
    return self.__stream;
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
  function utf8Decode(bytes) {
    var out = '', i = 0, n = bytes.length;
    while (i < n) {
      var c = bytes[i++];
      if (c < 0x80) out += String.fromCharCode(c);
      else if (c >= 0xC0 && c < 0xE0) out += String.fromCharCode(((c & 0x1F) << 6) | (bytes[i++] & 0x3F));
      else if (c >= 0xE0 && c < 0xF0) out += String.fromCharCode(((c & 0x0F) << 12) | ((bytes[i++] & 0x3F) << 6) | (bytes[i++] & 0x3F));
      else {
        var cp = (((c & 0x07) << 18) | ((bytes[i++] & 0x3F) << 12) | ((bytes[i++] & 0x3F) << 6) | (bytes[i++] & 0x3F)) - 0x10000;
        out += String.fromCharCode(0xD800 + (cp >> 10), 0xDC00 + (cp & 0x3FF));
      }
    }
    return out;
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
    this._closed = false;
    this._errored = false;
    this._error = undefined;
    this._reader = null;
    this._disturbed = false;
    this._source = source;
    var self = this;
    this._controller = {
      enqueue: function(chunk) {
        if (self._closed || self._errored) throw new TypeError("Cannot enqueue on a closed stream");
        self._chunks.push(chunk);
      },
      close: function() { self._closed = true; },
      error: function(e) { self._errored = true; self._error = e; },
      get desiredSize() { return self._closed ? null : 1; }
    };
    if (typeof source.start === 'function') {
      try { source.start(this._controller); } catch (e) { this._controller.error(e); }
    }
  }
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
    this._disturbed = true; this._closed = true; this._chunks = [];
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
      try { s._source.pull(s._controller); } catch (e) { s._errored = true; s._error = e; }
      if (s._chunks.length > 0) return Promise.resolve({ value: s._chunks.shift(), done: false });
    }
    if (s._errored) { if (this._cRej) { this._cRej(s._error); this._cRej = null; } return Promise.reject(s._error); }
    if (this._cRes) { this._cRes(undefined); this._cRes = null; }
    return Promise.resolve({ value: undefined, done: true });
  };
  ReadableStreamDefaultReader.prototype.releaseLock = function() {
    if (this._stream) { if (this._stream._reader === this) this._stream._reader = null; this._stream = null; }
  };
  ReadableStreamDefaultReader.prototype.cancel = function(reason) {
    if (this._stream) return this._stream.cancel(reason);
    return Promise.resolve(undefined);
  };
  globalThis.ReadableStreamDefaultReader = ReadableStreamDefaultReader;

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

  // Serialize a FormData to a multipart/form-data body + content-type. Binary
  // field values degrade through the UTF-8 string sink (the known body-channel
  // limit); text fields and filenames round-trip.
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

  // WHATWG "extract a body": returns { body: string|null, type: string|null }.
  // Binary Blob / buffer bodies degrade through the UTF-8 string sink (a known
  // v1 limit; a bytes channel is a later lift).
  function extractBody(v) {
    if (v == null) return { body: null, type: null };
    if (typeof v === 'string') return { body: v, type: 'text/plain;charset=UTF-8' };
    if (v instanceof URLSearchParams) return { body: v.toString(), type: 'application/x-www-form-urlencoded;charset=UTF-8' };
    if (v instanceof Blob) return { body: utf8Decode(v._b), type: v.type ? v.type : null };
    if (v instanceof FormData) return serializeFormData(v);
    if (v instanceof ReadableStream) {
      // Drain the (fully-buffered) stream: chunks the source enqueued in start().
      // Async-pull streams degrade to what is already queued (a known limit).
      var parts = v._chunks.slice(); v._chunks = []; v._disturbed = true; v._closed = true;
      var total = 0, i;
      for (i = 0; i < parts.length; i++) total += parts[i].length;
      var all = new Uint8Array(total), off = 0;
      for (i = 0; i < parts.length; i++) { all.set(parts[i], off); off += parts[i].length; }
      return { body: utf8Decode(all), type: null };
    }
    if (v instanceof ArrayBuffer) return { body: utf8Decode(new Uint8Array(v)), type: null };
    if (ArrayBuffer.isView(v)) return { body: utf8Decode(new Uint8Array(v.buffer, v.byteOffset, v.byteLength)), type: null };
    return { body: String(v), type: 'text/plain;charset=UTF-8' };
  }

  // Parse a consumed body back to a FormData for `.formData()`. Handles
  // application/x-www-form-urlencoded fully; multipart parsing is a later slice.
  function parseFormData(text, contentType) {
    var fd = new FormData();
    var ct = String(contentType || '').toLowerCase();
    if (ct.indexOf('application/x-www-form-urlencoded') === 0 || ct === '') {
      var usp = new URLSearchParams(text);
      usp.forEach(function(val, key) { fd.append(key, val); });
    } else {
      throw new TypeError("multipart formData() parsing is not implemented");
    }
    return fd;
  }

  var bodyMixin = {
    text: function() { return consume(this); },
    json: function() { return consume(this).then(function(t) { return JSON.parse(t); }); },
    arrayBuffer: function() { return consume(this).then(function(t) { return utf8Encode(t).buffer; }); },
    bytes: function() { return consume(this).then(function(t) { return utf8Encode(t); }); },
    blob: function() {
      var self = this;
      var ct = (self.headers && self.headers.get) ? self.headers.get('content-type') : null;
      return consume(self).then(function(t) { return new Blob([t], { type: ct || '' }); });
    },
    formData: function() {
      var self = this;
      var ct = (self.headers && self.headers.get) ? self.headers.get('content-type') : '';
      return consume(self).then(function(t) { return parseFormData(t, ct); });
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

  // ---- Request ----
  function Request(input, init) {
    init = init || {};
    if (input instanceof Request) {
      this.url = input.url; this.method = input.method; this.headers = new Headers(input.headers);
      this.__body = input.__body; this.mode = input.mode; this.credentials = input.credentials;
      this.redirect = input.redirect; this.cache = input.cache; this.destination = input.destination;
      this.signal = input.signal;
    } else {
      this.url = __resolve_url(String(input)); this.method = 'GET'; this.headers = new Headers(); this.__body = null;
      this.mode = 'cors'; this.credentials = 'same-origin'; this.redirect = 'follow'; this.cache = 'default'; this.destination = '';
      this.signal = makeSignal();
    }
    if (init.signal !== undefined) this.signal = init.signal;
    if (init.method !== undefined) this.method = String(init.method).toUpperCase();
    if (init.headers !== undefined) this.headers = new Headers(init.headers);
    if (init.body !== undefined && init.body !== null) {
      var eb = extractBody(init.body);
      this.__body = eb.body;
      if (eb.type && !this.headers.has('content-type')) this.headers.set('content-type', eb.type);
    }
    if (init.mode !== undefined) this.mode = String(init.mode);
    if (init.credentials !== undefined) this.credentials = String(init.credentials);
    if (init.redirect !== undefined) this.redirect = String(init.redirect);
    if (init.cache !== undefined) this.cache = String(init.cache);
    this.bodyUsed = false;
    if ((this.method === 'GET' || this.method === 'HEAD') && this.__body != null)
      throw new TypeError("Request with GET/HEAD method cannot have body.");
  }
  Request.prototype.clone = function() {
    if (this.bodyUsed || (this.__stream && this.__stream.locked)) throw new TypeError("Body is disturbed or locked.");
    var r = new Request(this); r.__body = this.__body; r.bodyUsed = false; return r;
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
    if (body != null) {
      var eb = extractBody(body);
      this.__body = eb.body;
      if (eb.type && !this.headers.has('content-type')) this.headers.set('content-type', eb.type);
    } else this.__body = null;
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
    init = init || {};
    var r = new Response(JSON.stringify(data), init);
    // application/json wins unless the caller's init explicitly set a type (the
    // string-body extraction defaults to text/plain, which must not stick here).
    if (!new Headers(init.headers).has("content-type")) r.headers.set("content-type", "application/json");
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
      // A pre-aborted signal rejects with its reason (the fetch is synchronous, so
      // there is no mid-flight window; the pre-flight abort check is the one that
      // matters for the harness).
      if (req.signal && req.signal.aborted) { reject(abortReason(req.signal.reason)); return; }
      var o;
      try { o = JSON.parse(__fetch(req.method, req.url, headersFlat(req.headers), req.__body != null ? req.__body : "")); }
      catch (e) { reject(new TypeError("Failed to fetch")); return; }
      if (o.networkError) { reject(new TypeError("Failed to fetch")); return; }
      resolve(responseFromOutcome(o));
    });
  };
})();
"#;
