/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Window platform services: `location` (and, as they land, `localStorage` /
//! `history`).
//!
//! Same shape as the other surfaces ([`crate::dom`], [`crate::fetch`]): a few
//! native sinks that reach the host [`HostState`], plus a JS bootstrap that
//! assembles the ergonomic objects. `location` reflects the document URL already
//! held in `HostState::base_url` (the same value `__resolve_url` resolves
//! against), parsed WHATWG-correctly with `url::Url`.

use std::cell::RefCell;

use script_engine_api::{CallCx, NativeFn, ScriptEngine};

use crate::HostState;

/// Run `f` against the host state, recovered from the engine host-data slot.
/// `None` when no host state is set.
fn with_host<E: ScriptEngine, R>(
    cx: &mut E::CallCx<'_>,
    f: impl FnOnce(&mut HostState) -> R,
) -> Option<R> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let mut host = cell.borrow_mut();
    Some(f(&mut host))
}

// ── location ─────────────────────────────────────────────────────────────────

/// One component of the document URL, mirroring the WHATWG `Location` getters.
/// An absent / unparseable base yields `""` (only `href` survives a base that
/// is not an absolute URL).
fn location_field(href: Option<&str>, field: &str) -> String {
    // No document URL yet -> `about:blank` (the default top-level location).
    let href = href.unwrap_or("about:blank");
    let Ok(u) = url::Url::parse(href) else {
        return if field == "href" { href.to_string() } else { String::new() };
    };
    match field {
        "href" => u.as_str().to_string(),
        "protocol" => format!("{}:", u.scheme()),
        "hostname" => u.host_str().unwrap_or_default().to_string(),
        "port" => u.port().map(|p| p.to_string()).unwrap_or_default(),
        "host" => match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => String::new(),
        },
        "pathname" => u.path().to_string(),
        "search" => u.query().map(|q| format!("?{q}")).unwrap_or_default(),
        "hash" => u.fragment().map(|f| format!("#{f}")).unwrap_or_default(),
        "origin" => u.origin().ascii_serialization(),
        _ => String::new(),
    }
}

/// `__locationField(field)` -> the named URL component of the document URL.
struct LocationField;
impl<E: ScriptEngine> NativeFn<E> for LocationField {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let field = cx.value_to_string(&a0)?;
        let href = with_host::<E, _>(cx, |h| h.base_url.clone()).flatten();
        let out = location_field(href.as_deref(), &field);
        cx.make_string(&out)
    }
}

/// `__locationAssign(url)` -> resolve `url` against the current document URL and
/// adopt it as the new document URL. Real navigation is host-driven; in the
/// scripted runtime this updates the document's own notion of its URL, which is
/// what `location.href` / `assign` / `replace` and `__resolve_url` read.
struct LocationAssign;
impl<E: ScriptEngine> NativeFn<E> for LocationAssign {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let input = cx.value_to_string(&a0)?;
        with_host::<E, _>(cx, |h| {
            let resolved = match h.base_url.as_deref().and_then(|b| url::Url::parse(b).ok()) {
                Some(base) => {
                    base.join(&input).map(|u| u.to_string()).unwrap_or_else(|_| input.clone())
                }
                None => input.clone(),
            };
            h.base_url = Some(resolved);
        });
        Ok(cx.undefined())
    }
}

// ── localStorage ─────────────────────────────────────────────────────────────

/// `__storageGet(key)` -> the stored value, or `null` if absent.
struct StorageGet;
impl<E: ScriptEngine> NativeFn<E> for StorageGet {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let key = cx.value_to_string(&a0)?;
        let value =
            with_host::<E, _>(cx, |h| h.storage.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()))
                .flatten();
        match value {
            Some(v) => cx.make_string(&v),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__storageSet(key, value)` -> insert or replace, preserving insertion order.
struct StorageSet;
impl<E: ScriptEngine> NativeFn<E> for StorageSet {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let key = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let value = cx.value_to_string(&a1)?;
        with_host::<E, _>(cx, |h| {
            if let Some(entry) = h.storage.iter_mut().find(|(k, _)| *k == key) {
                entry.1 = value;
            } else {
                h.storage.push((key, value));
            }
        });
        Ok(cx.undefined())
    }
}

/// `__storageRemove(key)` -> remove if present.
struct StorageRemove;
impl<E: ScriptEngine> NativeFn<E> for StorageRemove {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let key = cx.value_to_string(&a0)?;
        with_host::<E, _>(cx, |h| h.storage.retain(|(k, _)| *k != key));
        Ok(cx.undefined())
    }
}

/// `__storageClear()` -> empty the store.
struct StorageClear;
impl<E: ScriptEngine> NativeFn<E> for StorageClear {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        with_host::<E, _>(cx, |h| h.storage.clear());
        Ok(cx.undefined())
    }
}

/// `__storageKey(index)` -> the nth key in insertion order, or `null`.
struct StorageKey;
impl<E: ScriptEngine> NativeFn<E> for StorageKey {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let index = cx.value_to_string(&a0)?.parse::<usize>().ok();
        let key =
            with_host::<E, _>(cx, |h| index.and_then(|i| h.storage.get(i)).map(|(k, _)| k.clone()))
                .flatten();
        match key {
            Some(k) => cx.make_string(&k),
            None => Ok(cx.make_null()),
        }
    }
}

/// `__storageLength()` -> the key count, as a string (no number-minting primitive
/// yet, so JS does `Number(...)`).
struct StorageLength;
impl<E: ScriptEngine> NativeFn<E> for StorageLength {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let n = with_host::<E, _>(cx, |h| h.storage.len()).unwrap_or(0);
        cx.make_string(&n.to_string())
    }
}

pub(crate) fn install_platform_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<LocationField>("__locationField", 1)?;
    engine.set_function::<LocationAssign>("__locationAssign", 1)?;
    engine.set_function::<StorageGet>("__storageGet", 1)?;
    engine.set_function::<StorageSet>("__storageSet", 2)?;
    engine.set_function::<StorageRemove>("__storageRemove", 1)?;
    engine.set_function::<StorageClear>("__storageClear", 0)?;
    engine.set_function::<StorageKey>("__storageKey", 1)?;
    engine.set_function::<StorageLength>("__storageLength", 0)?;
    engine.eval(PLATFORM_BOOTSTRAP)?;
    Ok(())
}

/// ES5-flavored (no classes / `let`) for the widest backend coverage, matching
/// the other bootstraps.
const PLATFORM_BOOTSTRAP: &str = r#"
(function() {
  // ── location ── a LIVE view of the document URL (HostState.base_url): the
  // getters re-read it each access, so it stays correct after `assign` / `href =`
  // (unlike a snapshot). `set_base_url` only updates the host's base URL now.
  var location = {};
  var getOnly = ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin'];
  for (var i = 0; i < getOnly.length; i++) {
    (function(p) {
      Object.defineProperty(location, p, {
        enumerable: true,
        configurable: true,
        get: function() { return __locationField(p); },
      });
    })(getOnly[i]);
  }
  Object.defineProperty(location, 'href', {
    enumerable: true,
    configurable: true,
    get: function() { return __locationField('href'); },
    set: function(v) { __locationAssign(String(v)); },
  });
  location.assign = function(url) { __locationAssign(String(url)); };
  location.replace = function(url) { __locationAssign(String(url)); };
  location.reload = function() {}; // real reload is host-driven
  location.toString = function() { return __locationField('href'); };
  globalThis.location = location;

  // ── localStorage ── an in-memory Storage (one origin per runtime). The methods
  // sit on a backing object; a Proxy adds the named-property access Storage has
  // (`localStorage.foo`, `'foo' in localStorage`, `delete`, `Object.keys`).
  var storageApi = {
    getItem: function(k) { return __storageGet(String(k)); },
    setItem: function(k, v) { __storageSet(String(k), String(v)); },
    removeItem: function(k) { __storageRemove(String(k)); },
    clear: function() { __storageClear(); },
    key: function(i) { return __storageKey(String(i >>> 0)); },
  };
  Object.defineProperty(storageApi, 'length', {
    configurable: true,
    get: function() { return Number(__storageLength()); },
  });
  var reserved = { getItem: 1, setItem: 1, removeItem: 1, clear: 1, key: 1, length: 1 };
  globalThis.localStorage = new Proxy(storageApi, {
    get: function(target, prop) {
      if (typeof prop !== 'string' || reserved[prop]) { return target[prop]; }
      var v = __storageGet(prop);
      return v === null ? undefined : v;
    },
    set: function(target, prop, value) {
      if (typeof prop === 'string' && !reserved[prop]) { __storageSet(prop, String(value)); }
      return true;
    },
    has: function(target, prop) {
      if (typeof prop === 'string' && reserved[prop]) { return true; }
      return typeof prop === 'string' && __storageGet(prop) !== null;
    },
    deleteProperty: function(target, prop) {
      if (typeof prop === 'string' && !reserved[prop]) { __storageRemove(prop); }
      return true;
    },
    ownKeys: function() {
      var keys = [];
      var n = Number(__storageLength());
      for (var i = 0; i < n; i++) { keys.push(__storageKey(String(i))); }
      return keys;
    },
    getOwnPropertyDescriptor: function(target, prop) {
      if (typeof prop === 'string' && reserved[prop]) {
        return Object.getOwnPropertyDescriptor(target, prop);
      }
      var v = __storageGet(String(prop));
      if (v === null) { return undefined; }
      return { value: v, writable: true, enumerable: true, configurable: true };
    },
  });
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;

    /// `location` reflects the document URL (`HostState::base_url`), parsed into
    /// the WHATWG components.
    fn location_reflects_base_url<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.set_base_url("https://example.com:8080/dir/page.html?q=1#frag").expect("base url");
        rt.eval(
            "console.log(location.href);\
             console.log(location.protocol);\
             console.log(location.host);\
             console.log(location.hostname);\
             console.log(location.port);\
             console.log(location.pathname);\
             console.log(location.search);\
             console.log(location.hash);\
             console.log(location.origin);\
             console.log('' + location);",
        )
        .expect("location script");
        assert_eq!(
            rt.host().borrow().console,
            vec![
                "https://example.com:8080/dir/page.html?q=1#frag",
                "https:",
                "example.com:8080",
                "example.com",
                "8080",
                "/dir/page.html",
                "?q=1",
                "#frag",
                "https://example.com:8080",
                "https://example.com:8080/dir/page.html?q=1#frag",
            ],
        );
    }

    /// `location.assign` / `location.href =` resolve relative to the current URL
    /// and update what the getters (and `__resolve_url`) read.
    fn location_assign_updates_url<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.set_base_url("https://example.com/a/b.html").expect("base url");
        rt.eval(
            "location.assign('../c.html'); console.log(location.href);\
             location.href = 'd.html'; console.log(location.href);\
             console.log(location.pathname);",
        )
        .expect("assign script");
        assert_eq!(
            rt.host().borrow().console,
            vec![
                "https://example.com/c.html",
                "https://example.com/d.html",
                "/d.html",
            ],
        );
    }

    /// `localStorage` round-trips via the methods and named access, reports
    /// `length` / `key(n)` in insertion order, and supports `in` / `Object.keys`
    /// / `removeItem` / `clear`.
    fn local_storage_round_trips<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.eval(
            "localStorage.setItem('a', '1');\
             localStorage.b = '2';\
             console.log(localStorage.getItem('a') + ',' + localStorage.a + ',' + localStorage.b);\
             console.log(String(localStorage.length));\
             console.log(localStorage.key(0) + ',' + localStorage.key(1) + ',' + (localStorage.key(2) === null));\
             console.log(localStorage.getItem('missing') === null);\
             console.log(('a' in localStorage) + ',' + ('missing' in localStorage));\
             console.log(Object.keys(localStorage).join('|'));\
             localStorage.removeItem('a');\
             console.log((localStorage.getItem('a') === null) + ',' + localStorage.length);\
             localStorage.clear();\
             console.log(String(localStorage.length));",
        )
        .expect("storage script");
        assert_eq!(
            rt.host().borrow().console,
            vec![
                "1,1,2",   // getItem('a'), .a, .b
                "2",       // length
                "a,b,true", // key(0), key(1), key(2)===null
                "true",    // getItem('missing')===null
                "true,false", // 'a' in / 'missing' in
                "a|b",     // Object.keys (insertion order)
                "true,1",  // getItem('a')===null after remove, length
                "0",       // length after clear
            ],
        );
    }

    #[test]
    fn location_reflects_base_url_on_boa() {
        location_reflects_base_url::<script_engine_boa::BoaEngine>();
    }
    #[test]
    fn local_storage_round_trips_on_boa() {
        local_storage_round_trips::<script_engine_boa::BoaEngine>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn local_storage_round_trips_on_nova() {
        local_storage_round_trips::<script_engine_nova::NovaEngine>();
    }
    #[test]
    fn location_assign_updates_url_on_boa() {
        location_assign_updates_url::<script_engine_boa::BoaEngine>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn location_reflects_base_url_on_nova() {
        location_reflects_base_url::<script_engine_nova::NovaEngine>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn location_assign_updates_url_on_nova() {
        location_assign_updates_url::<script_engine_nova::NovaEngine>();
    }
}
