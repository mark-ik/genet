/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pragmatic local-first link resolution, shared by the static ([`document`]) and
//! scripted ([`scripted`]) loaders (and the chrome viewer) so they cannot drift —
//! and, being dependency-free, usable from the headless `scripted` profile without
//! dragging in the `document` module's render/`data-url` stack. This is deliberately
//! *not* the full WHATWG URL algorithm: it resolves bare local paths (which a real
//! `Url::join` mishandles — a Windows `C:\…` path parses as a `c:` scheme) as well as
//! `http(s):`/`data:`/`file:` bases. The module-resolution path that needs `./`/`../`
//! normalization uses `url::Url::join` instead (see `scripted::eval_module_reporting`).
//!
//! [`document`]: crate::document
//! [`scripted`]: crate::scripted

/// Resolve a link `href` against the `base` URL the document was loaded from. Absolute
/// hrefs (a scheme like `https:` / `data:`, a Windows drive, or a root path) pass
/// through; a relative href joins onto the base's directory (everything up to its last
/// `/` or `\`). Pragmatic local-first resolution, not the full URL algorithm.
pub fn resolve_href(base: &str, href: &str) -> String {
    if has_scheme(href) || href.starts_with('/') || href.starts_with('\\') {
        return href.to_string();
    }
    let cut = base.rfind(['/', '\\']).map_or(0, |i| i + 1);
    format!("{}{}", &base[..cut], href)
}

/// Whether `url` begins with a URL scheme (`name:`) or a Windows drive (`C:`). A bare
/// relative path (`page.html`, `sub/page.html`) has neither.
fn has_scheme(url: &str) -> bool {
    match url.find(':') {
        Some(i) if i > 0 => url[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `resolve_href` joins a relative link onto the base's directory and passes
    /// absolute hrefs (scheme / root path) through unchanged.
    #[test]
    fn resolve_href_joins_relative_and_passes_absolute() {
        assert_eq!(resolve_href("docs/a.html", "b.html"), "docs/b.html");
        assert_eq!(resolve_href("a.html", "sub/c.html"), "sub/c.html");
        assert_eq!(
            resolve_href("file:///x/a.html", "b.html"),
            "file:///x/b.html"
        );
        assert_eq!(
            resolve_href("a.html", "https://example.org/p"),
            "https://example.org/p"
        );
        assert_eq!(
            resolve_href("a.html", "data:text/html,<p>x</p>"),
            "data:text/html,<p>x</p>"
        );
        assert_eq!(resolve_href("docs/a.html", "/root.html"), "/root.html");
    }
}
