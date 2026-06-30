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
use std::rc::Rc;

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
        return if field == "href" {
            href.to_string()
        } else {
            String::new()
        };
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

/// WHATWG-resolve `input` against the document `base` (if absolute); otherwise
/// return `input` unchanged. Shared by `location` and `history`.
fn resolve_against(base: Option<&str>, input: &str) -> String {
    match base.and_then(|b| url::Url::parse(b).ok()) {
        Some(b) => b
            .join(input)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| input.to_owned()),
        None => input.to_owned(),
    }
}

/// Seed the session history with one entry (the current document) on first use.
/// The initial URL is the document base, defaulting to `about:blank`.
fn ensure_history(h: &mut HostState) {
    if h.history.is_empty() {
        let url = h
            .base_url
            .clone()
            .unwrap_or_else(|| "about:blank".to_string());
        h.history.push(("null".to_string(), url));
        h.history_index = 0;
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
            h.base_url = Some(resolve_against(h.base_url.as_deref(), &input));
        });
        Ok(cx.undefined())
    }
}

// ── localStorage ─────────────────────────────────────────────────────────────

/// The host's backing for `localStorage` (e.g. a durable, persona + origin-partitioned
/// store over eidetic). When set, the `localStorage` sinks route through it instead of
/// the in-memory [`HostState::storage`] default (kept for tests / WPT / no-host runs).
/// Methods mirror the WHATWG `Storage` interface; the host owns persistence and the
/// insertion order that `key(n)` / `length` report. Install with
/// [`Runtime::set_local_storage_provider`](crate::Runtime::set_local_storage_provider).
pub trait StorageProvider {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn remove(&self, key: &str);
    fn clear(&self);
    /// The nth key in insertion order, or `None` when out of range.
    fn key(&self, index: usize) -> Option<String>;
    fn length(&self) -> usize;
}

/// Clone the localStorage provider out of host state (so it is not borrowed while
/// invoked). `None` = no provider, so the sinks fall back to in-memory storage.
fn host_local_storage<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Option<Rc<dyn StorageProvider>> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let provider = cell.borrow().local_storage.clone();
    provider
}

/// `__storageGet(key)` -> the stored value, or `null` if absent.
struct StorageGet;
impl<E: ScriptEngine> NativeFn<E> for StorageGet {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let key = cx.value_to_string(&a0)?;
        let value = match host_local_storage::<E>(cx) {
            Some(provider) => provider.get(&key),
            None => with_host::<E, _>(cx, |h| {
                h.storage
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.clone())
            })
            .flatten(),
        };
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
        if let Some(provider) = host_local_storage::<E>(cx) {
            provider.set(&key, &value);
        } else {
            with_host::<E, _>(cx, |h| {
                if let Some(entry) = h.storage.iter_mut().find(|(k, _)| *k == key) {
                    entry.1 = value;
                } else {
                    h.storage.push((key, value));
                }
            });
        }
        Ok(cx.undefined())
    }
}

/// `__storageRemove(key)` -> remove if present.
struct StorageRemove;
impl<E: ScriptEngine> NativeFn<E> for StorageRemove {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let key = cx.value_to_string(&a0)?;
        if let Some(provider) = host_local_storage::<E>(cx) {
            provider.remove(&key);
        } else {
            with_host::<E, _>(cx, |h| h.storage.retain(|(k, _)| *k != key));
        }
        Ok(cx.undefined())
    }
}

/// `__storageClear()` -> empty the store.
struct StorageClear;
impl<E: ScriptEngine> NativeFn<E> for StorageClear {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        if let Some(provider) = host_local_storage::<E>(cx) {
            provider.clear();
        } else {
            with_host::<E, _>(cx, |h| h.storage.clear());
        }
        Ok(cx.undefined())
    }
}

/// `__storageKey(index)` -> the nth key in insertion order, or `null`.
struct StorageKey;
impl<E: ScriptEngine> NativeFn<E> for StorageKey {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let index = cx.value_to_string(&a0)?.parse::<usize>().ok();
        let key = match host_local_storage::<E>(cx) {
            Some(provider) => index.and_then(|i| provider.key(i)),
            None => with_host::<E, _>(cx, |h| {
                index.and_then(|i| h.storage.get(i)).map(|(k, _)| k.clone())
            })
            .flatten(),
        };
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
        let n = match host_local_storage::<E>(cx) {
            Some(provider) => provider.length(),
            None => with_host::<E, _>(cx, |h| h.storage.len()).unwrap_or(0),
        };
        cx.make_string(&n.to_string())
    }
}

// ── history ──────────────────────────────────────────────────────────────────

/// `__historyPush(stateJson, url, hasUrl)` -> push a new entry (dropping any
/// forward entries) and make it current. `hasUrl == "true"` resolves `url`
/// against the current document URL and adopts it; otherwise the URL is unchanged.
struct HistoryPush;
impl<E: ScriptEngine> NativeFn<E> for HistoryPush {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let state = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let url = cx.value_to_string(&a1)?;
        let a2 = cx.arg(2);
        let has_url = cx.value_to_string(&a2)? == "true";
        with_host::<E, _>(cx, |h| {
            ensure_history(h);
            let entry_url = if has_url {
                resolve_against(h.base_url.as_deref(), &url)
            } else {
                h.history[h.history_index].1.clone()
            };
            h.history.truncate(h.history_index + 1);
            h.history.push((state, entry_url.clone()));
            h.history_index = h.history.len() - 1;
            if has_url {
                h.base_url = Some(entry_url);
            }
        });
        Ok(cx.undefined())
    }
}

/// `__historyReplace(stateJson, url, hasUrl)` -> replace the current entry in
/// place (same `hasUrl` semantics as push).
struct HistoryReplace;
impl<E: ScriptEngine> NativeFn<E> for HistoryReplace {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let state = cx.value_to_string(&a0)?;
        let a1 = cx.arg(1);
        let url = cx.value_to_string(&a1)?;
        let a2 = cx.arg(2);
        let has_url = cx.value_to_string(&a2)? == "true";
        with_host::<E, _>(cx, |h| {
            ensure_history(h);
            let entry_url = if has_url {
                resolve_against(h.base_url.as_deref(), &url)
            } else {
                h.history[h.history_index].1.clone()
            };
            let idx = h.history_index;
            h.history[idx] = (state, entry_url.clone());
            if has_url {
                h.base_url = Some(entry_url);
            }
        });
        Ok(cx.undefined())
    }
}

/// `__historyState()` -> the current entry's serialized state JSON.
struct HistoryState;
impl<E: ScriptEngine> NativeFn<E> for HistoryState {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let state = with_host::<E, _>(cx, |h| {
            ensure_history(h);
            h.history[h.history_index].0.clone()
        })
        .unwrap_or_else(|| "null".to_string());
        cx.make_string(&state)
    }
}

/// `__historyLength()` -> the entry count, as a string.
struct HistoryLength;
impl<E: ScriptEngine> NativeFn<E> for HistoryLength {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let n = with_host::<E, _>(cx, |h| {
            ensure_history(h);
            h.history.len()
        })
        .unwrap_or(1);
        cx.make_string(&n.to_string())
    }
}

/// `__historyGo(delta)` -> move the current entry by `delta` (clamped to range)
/// and adopt that entry's URL. No `popstate` is fired (the scripted tier has no
/// navigation/event-loop integration); `state` / the document URL update.
struct HistoryGo;
impl<E: ScriptEngine> NativeFn<E> for HistoryGo {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let a0 = cx.arg(0);
        let delta = cx.value_to_string(&a0)?.parse::<i64>().unwrap_or(0);
        with_host::<E, _>(cx, |h| {
            ensure_history(h);
            let last = h.history.len() as i64 - 1;
            let target = (h.history_index as i64 + delta).clamp(0, last);
            h.history_index = target as usize;
            h.base_url = Some(h.history[h.history_index].1.clone());
        });
        Ok(cx.undefined())
    }
}

pub(crate) fn install_platform_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<LocationField>("__locationField", 1)?;
    engine.set_function::<LocationAssign>("__locationAssign", 1)?;
    engine.set_function::<HistoryPush>("__historyPush", 3)?;
    engine.set_function::<HistoryReplace>("__historyReplace", 3)?;
    engine.set_function::<HistoryState>("__historyState", 0)?;
    engine.set_function::<HistoryLength>("__historyLength", 0)?;
    engine.set_function::<HistoryGo>("__historyGo", 1)?;
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

  // ── history ── pushState / replaceState / state / length / go / back / forward.
  // No popstate / real navigation (the scripted tier has none); state + URL
  // bookkeeping is correct. State round-trips through JSON (non-JSON state -> null).
  function stateJson(state) {
    var s = JSON.stringify(state);
    return s === undefined ? 'null' : s;
  }
  var history = {
    pushState: function(state, title, url) {
      __historyPush(stateJson(state), url == null ? '' : String(url), url == null ? 'false' : 'true');
    },
    replaceState: function(state, title, url) {
      __historyReplace(stateJson(state), url == null ? '' : String(url), url == null ? 'false' : 'true');
    },
    go: function(delta) { __historyGo(String((delta || 0) | 0)); },
    back: function() { __historyGo('-1'); },
    forward: function() { __historyGo('1'); },
  };
  Object.defineProperty(history, 'state', {
    enumerable: true, configurable: true,
    get: function() { return JSON.parse(__historyState()); },
  });
  Object.defineProperty(history, 'length', {
    enumerable: true, configurable: true,
    get: function() { return Number(__historyLength()); },
  });
  globalThis.history = history;
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
        rt.set_base_url("https://example.com:8080/dir/page.html?q=1#frag")
            .expect("base url");
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
        rt.set_base_url("https://example.com/a/b.html")
            .expect("base url");
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
                "1,1,2",      // getItem('a'), .a, .b
                "2",          // length
                "a,b,true",   // key(0), key(1), key(2)===null
                "true",       // getItem('missing')===null
                "true,false", // 'a' in / 'missing' in
                "a|b",        // Object.keys (insertion order)
                "true,1",     // getItem('a')===null after remove, length
                "0",          // length after clear
            ],
        );
    }

    /// With a host `StorageProvider` installed, the `localStorage` sinks route to it
    /// (the durable host backing) instead of the in-memory default. (Native session
    /// store 6b.)
    fn local_storage_routes_to_provider<E: ScriptEngine>() {
        use std::cell::RefCell;
        use std::rc::Rc;

        struct Stub {
            items: Rc<RefCell<Vec<(String, String)>>>,
        }
        impl crate::StorageProvider for Stub {
            fn get(&self, key: &str) -> Option<String> {
                self.items
                    .borrow()
                    .iter()
                    .find(|(k, _)| k.as_str() == key)
                    .map(|(_, v)| v.clone())
            }
            fn set(&self, key: &str, value: &str) {
                let mut items = self.items.borrow_mut();
                if let Some(entry) = items.iter_mut().find(|(k, _)| k.as_str() == key) {
                    entry.1 = value.to_string();
                } else {
                    items.push((key.to_string(), value.to_string()));
                }
            }
            fn remove(&self, key: &str) {
                self.items.borrow_mut().retain(|(k, _)| k.as_str() != key);
            }
            fn clear(&self) {
                self.items.borrow_mut().clear();
            }
            fn key(&self, index: usize) -> Option<String> {
                self.items.borrow().get(index).map(|(k, _)| k.clone())
            }
            fn length(&self) -> usize {
                self.items.borrow().len()
            }
        }

        let items = Rc::new(RefCell::new(Vec::new()));
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.set_local_storage_provider(Box::new(Stub {
            items: items.clone(),
        }));
        rt.eval(
            "localStorage.setItem('a', '1');\
             localStorage.setItem('b', '2');\
             console.log(localStorage.getItem('a') + ',' + localStorage.length + ',' + localStorage.key(1));\
             localStorage.removeItem('a');\
             console.log((localStorage.getItem('a') === null) + ',' + localStorage.length);",
        )
        .expect("storage provider script");
        assert_eq!(rt.host().borrow().console, vec!["1,2,b", "true,1"]);
        // The writes landed in the host-backed store, not the in-memory default.
        assert_eq!(*items.borrow(), vec![("b".to_string(), "2".to_string())]);
        assert!(rt.host().borrow().storage.is_empty());
    }

    /// `history` records state + URL across pushState / replaceState, keeps
    /// `length` correct (dropping forward entries on a new push), navigates with
    /// back / forward, and syncs the document URL.
    fn history_navigation<E: ScriptEngine>() {
        let mut rt = Runtime::<E>::new().expect("runtime");
        rt.set_base_url("https://example.com/a").expect("base url");
        rt.eval(
            "history.pushState({n: 1}, '', '/b');\
             console.log(history.length + ',' + location.pathname + ',' + history.state.n);\
             history.pushState({n: 2}, '', 'c');\
             console.log(history.length + ',' + location.pathname + ',' + history.state.n);\
             history.back();\
             console.log(location.pathname + ',' + history.state.n);\
             history.replaceState({n: 9}, '');\
             console.log(history.length + ',' + location.pathname + ',' + history.state.n);\
             history.pushState({n: 3}, '', '/d');\
             console.log(history.length + ',' + location.pathname);\
             history.forward();\
             console.log(history.length + ',' + location.pathname);\
             console.log(String(history.state));",
        )
        .expect("history script");
        assert_eq!(
            rt.host().borrow().console,
            vec![
                "2,/b,1",          // pushState /b
                "3,/c,2",          // pushState c (relative)
                "/b,1",            // back -> entry 1
                "3,/b,9",          // replaceState (no url) keeps length + url
                "3,/d",            // pushState /d drops the dropped-forward /c entry
                "3,/d",            // forward clamps at the last entry
                "[object Object]", // history.state is the parsed object
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
    #[test]
    fn local_storage_routes_to_provider_on_boa() {
        local_storage_routes_to_provider::<script_engine_boa::BoaEngine>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn local_storage_routes_to_provider_on_nova() {
        local_storage_routes_to_provider::<script_engine_nova::NovaEngine>();
    }
    #[test]
    fn history_navigation_on_boa() {
        history_navigation::<script_engine_boa::BoaEngine>();
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn history_navigation_on_nova() {
        history_navigation::<script_engine_nova::NovaEngine>();
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
