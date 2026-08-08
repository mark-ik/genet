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
/// hrefs (a scheme like `https:` / `data:` or a Windows drive) pass through. For a
/// remote document, root-relative and scheme-relative references stay on that remote
/// origin; a relative href joins its path directory. Bare local paths retain the
/// local-first behavior rather than being misread as a URL scheme.
pub fn resolve_href(base: &str, href: &str) -> String {
    if has_scheme(href) {
        return href.to_string();
    }

    if let Some((scheme, authority_end)) = remote_origin(base) {
        if let Some(network_path) = href.strip_prefix("//") {
            return format!("{scheme}://{network_path}");
        }
        if href.starts_with('/') {
            return format!("{}{}", &base[..authority_end], href);
        }

        let page_end = base.find(['?', '#']).unwrap_or(base.len());
        if href.starts_with('?') || href.starts_with('#') {
            return format!("{}{}", &base[..page_end], href);
        }
        let page = &base[..page_end];
        let path_start = authority_end.min(page.len());
        if let Some(index) = page[path_start..].rfind('/') {
            let cut = path_start + index + 1;
            return format!("{}{}", &page[..cut], href);
        }
        return format!("{page}/{href}");
    }

    if href.starts_with('/') || href.starts_with('\\') {
        return href.to_string();
    }
    let cut = base.rfind(['/', '\\']).map_or(0, |i| i + 1);
    format!("{}{}", &base[..cut], href)
}

/// The scheme and exclusive end of `scheme://authority` for an absolute web-like URL.
/// This intentionally stays small and dependency-free so local Windows paths keep the
/// same resolution contract as before; the resource fetcher decides which schemes it
/// actually supports.
fn remote_origin(base: &str) -> Option<(&str, usize)> {
    let scheme_end = base.find("://")?;
    let scheme = &base[..scheme_end];
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let after_authority = &base[scheme_end + 3..];
    let authority_len = after_authority
        .find(['/', '?', '#'])
        .unwrap_or(after_authority.len());
    Some((scheme, scheme_end + 3 + authority_len))
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

    #[test]
    fn resolve_href_keeps_remote_resources_on_their_origin() {
        assert_eq!(
            resolve_href("https://example.org/page/index.html", "/site.css?v=1"),
            "https://example.org/site.css?v=1"
        );
        assert_eq!(
            resolve_href("https://example.org", "site.css"),
            "https://example.org/site.css"
        );
        assert_eq!(
            resolve_href("https://example.org/a/b.html?x=1", "image.png"),
            "https://example.org/a/image.png"
        );
        assert_eq!(
            resolve_href(
                "https://example.org/a/b.html",
                "//cdn.example.org/style.css"
            ),
            "https://cdn.example.org/style.css"
        );
    }
}
