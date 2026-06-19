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

pub(crate) fn install_platform_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<LocationField>("__locationField", 1)?;
    engine.set_function::<LocationAssign>("__locationAssign", 1)?;
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

    #[test]
    fn location_reflects_base_url_on_boa() {
        location_reflects_base_url::<script_engine_boa::BoaEngine>();
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
