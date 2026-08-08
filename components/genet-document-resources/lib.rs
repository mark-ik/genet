//! Engine-neutral document resource discovery.
//!
//! A host chooses byte policy through [`ResourceFetcher`]; this component
//! discovers resources, preserves stylesheet order and source identity, and
//! records every unresolved dependency. CSS and layout engines only consume the
//! resulting immutable records.

#![deny(unsafe_code)]

use std::collections::HashMap;

pub use genet_host_api::ResourceFetcher;
use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

type ByteFetcher<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

/// The HTML node kind which owns an author stylesheet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StylesheetOwner {
    Inline,
    Linked,
}

/// One author stylesheet, in document linking order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStylesheet {
    pub owner: StylesheetOwner,
    /// `None` for inline sheets without a known document identity.
    pub source_url: Option<String>,
    /// Link `media`, retained for the selected style engine to evaluate.
    pub media: Option<String>,
    pub text: String,
    pub document_order: u64,
}

/// Kinds of bytes a document engine may consume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    Image,
    Font,
}

/// A host-fetched dependency, attributed both to its source spelling and to
/// its source-relative resolved identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedResource {
    pub kind: ResourceKind,
    pub authored_url: String,
    pub resolved_url: String,
    pub bytes: Vec<u8>,
}

/// A dependency which did not become usable bytes. These diagnostics are part
/// of the resolved set so engines never have to represent a failed load as an
/// empty success.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceDiagnostic {
    LinkedStylesheetNoByteAuthority {
        authored_url: String,
        resolved_url: String,
    },
    LinkedStylesheetUnavailable {
        authored_url: String,
        resolved_url: String,
    },
    LinkedStylesheetInvalidUtf8 {
        authored_url: String,
        resolved_url: String,
    },
    ResourceUnavailable {
        kind: ResourceKind,
        authored_url: String,
        resolved_url: String,
    },
    UnsupportedScheme {
        kind: ResourceKind,
        authored_url: String,
        resolved_url: String,
    },
    /// Livery has no import rule object yet. It is intentionally reported
    /// rather than stripping the rule and overstating support.
    ImportRulePendingR5 { source_url: Option<String> },
}

/// The host-owned resource view of one parsed HTML document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResolvedDocumentResources {
    pub document_url: Option<String>,
    pub stylesheets: Vec<ResolvedStylesheet>,
    pub resources: Vec<ResolvedResource>,
    pub diagnostics: Vec<ResourceDiagnostic>,
}

impl ResolvedDocumentResources {
    /// Resolve all loadable dependencies through the host's byte contract.
    pub fn resolve<D, Fetch>(dom: &D, document_url: Option<&str>, fetcher: &Fetch) -> Self
    where
        D: LayoutDom,
        Fetch: ResourceFetcher + ?Sized,
    {
        let mut fetch = |url: &str| fetcher.fetch(url);
        resolve_with(dom, document_url, &mut fetch)
    }

    /// Discover only inline content. Linked resources remain visible as
    /// diagnostics because this parse has no authority to fetch their bytes.
    pub fn discover<D>(dom: &D, document_url: Option<&str>) -> Self
    where
        D: LayoutDom,
    {
        collect(dom, document_url, None)
    }

    /// Return the text in retained author-sheet order.
    pub fn stylesheet_text(&self) -> Vec<&str> {
        self.stylesheets
            .iter()
            .map(|sheet| sheet.text.as_str())
            .collect()
    }
}

/// Resolve resources through a closure. This is useful for compatibility
/// adapters which already have a host byte callback without inventing another
/// fetch trait.
pub fn resolve_with<D>(
    dom: &D,
    document_url: Option<&str>,
    fetch: &mut ByteFetcher<'_>,
) -> ResolvedDocumentResources
where
    D: LayoutDom,
{
    collect(dom, document_url, Some(fetch))
}

fn collect<D>(
    dom: &D,
    document_url: Option<&str>,
    mut fetch: Option<&mut ByteFetcher<'_>>,
) -> ResolvedDocumentResources
where
    D: LayoutDom,
{
    let mut result = ResolvedDocumentResources {
        document_url: document_url.map(str::to_owned),
        ..Default::default()
    };
    let mut order = 0;
    collect_stylesheets(
        dom,
        dom.document(),
        document_url,
        &mut fetch,
        &mut order,
        &mut result,
    );

    let mut cached = HashMap::<String, Option<Vec<u8>>>::new();
    collect_document_resources(
        dom,
        dom.document(),
        document_url,
        &mut fetch,
        &mut cached,
        &mut result,
    );
    let sheets = result.stylesheets.clone();
    for sheet in &sheets {
        if starts_with_import(&sheet.text) {
            result
                .diagnostics
                .push(ResourceDiagnostic::ImportRulePendingR5 {
                    source_url: sheet.source_url.clone(),
                });
        }
        collect_stylesheet_resources(
            &sheet.text,
            sheet.source_url.as_deref(),
            &mut fetch,
            &mut cached,
            &mut result,
        );
    }
    result
}

fn collect_stylesheets<D>(
    dom: &D,
    node: D::NodeId,
    document_url: Option<&str>,
    fetch: &mut Option<&mut ByteFetcher<'_>>,
    order: &mut u64,
    result: &mut ResolvedDocumentResources,
) where
    D: LayoutDom,
{
    if dom
        .element_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("style"))
    {
        let mut text = String::new();
        collect_text(dom, node, &mut text);
        if !text.trim().is_empty() {
            result.stylesheets.push(ResolvedStylesheet {
                owner: StylesheetOwner::Inline,
                source_url: document_url.map(str::to_owned),
                media: None,
                text,
                document_order: *order,
            });
            *order = order.saturating_add(1);
        }
    }

    if is_stylesheet_link(dom, node) {
        let namespace = Namespace::default();
        let href = LocalName::from("href");
        let media = LocalName::from("media");
        if let Some(authored_url) = dom
            .attribute(node, &namespace, &href)
            .map(str::trim)
            .filter(|href| !href.is_empty())
        {
            let resolved_url = resolve_url(document_url, authored_url);
            let media = dom
                .attribute(node, &namespace, &media)
                .map(str::trim)
                .filter(|media| !media.is_empty())
                .map(str::to_owned);
            match fetch.as_deref_mut() {
                None => {
                    result
                        .diagnostics
                        .push(ResourceDiagnostic::LinkedStylesheetNoByteAuthority {
                            authored_url: authored_url.to_owned(),
                            resolved_url,
                        })
                },
                Some(fetch) => {
                    match fetch(&resolved_url) {
                        Some(bytes) => match String::from_utf8(bytes) {
                            Ok(text) => {
                                result.stylesheets.push(ResolvedStylesheet {
                                    owner: StylesheetOwner::Linked,
                                    source_url: Some(resolved_url),
                                    media,
                                    text,
                                    document_order: *order,
                                });
                                *order = order.saturating_add(1);
                            },
                            Err(_) => result.diagnostics.push(
                                ResourceDiagnostic::LinkedStylesheetInvalidUtf8 {
                                    authored_url: authored_url.to_owned(),
                                    resolved_url,
                                },
                            ),
                        },
                        None => result.diagnostics.push(
                            ResourceDiagnostic::LinkedStylesheetUnavailable {
                                authored_url: authored_url.to_owned(),
                                resolved_url,
                            },
                        ),
                    }
                },
            }
        }
    }

    for child in dom.dom_children(node) {
        collect_stylesheets(dom, child, document_url, fetch, order, result);
    }
}

fn collect_document_resources<D>(
    dom: &D,
    node: D::NodeId,
    document_url: Option<&str>,
    fetch: &mut Option<&mut ByteFetcher<'_>>,
    cached: &mut HashMap<String, Option<Vec<u8>>>,
    result: &mut ResolvedDocumentResources,
) where
    D: LayoutDom,
{
    let namespace = Namespace::default();
    let loading = LocalName::from("loading");
    let attribute = match dom.element_name(node).map(|name| name.local.as_ref()) {
        Some("img" | "embed") => Some("src"),
        Some("object") => Some("data"),
        Some("video") => Some("poster"),
        _ => None,
    };
    let lazy = dom
        .element_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("img"))
        && dom
            .attribute(node, &namespace, &loading)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("lazy"));
    if !lazy
        && let Some(attribute) = attribute
        && let Some(authored_url) = dom
            .attribute(node, &namespace, &LocalName::from(attribute))
            .map(str::trim)
            .filter(|url| !url.is_empty())
    {
        collect_resource(
            ResourceKind::Image,
            authored_url,
            document_url,
            fetch,
            cached,
            result,
        );
    }
    for child in dom.dom_children(node) {
        collect_document_resources(dom, child, document_url, fetch, cached, result);
    }
}

fn collect_stylesheet_resources(
    css: &str,
    base_url: Option<&str>,
    fetch: &mut Option<&mut ByteFetcher<'_>>,
    cached: &mut HashMap<String, Option<Vec<u8>>>,
    result: &mut ResolvedDocumentResources,
) {
    let mut cursor = 0;
    let lower = css.to_ascii_lowercase();
    while let Some(found) = lower[cursor..].find("url(") {
        let start = cursor + found + 4;
        let Some(close) = css[start..].find(')') else {
            break;
        };
        let authored_url = css[start..start + close].trim().trim_matches(['\'', '"']);
        if !authored_url.is_empty() {
            collect_resource(
                resource_kind_for_css_url(authored_url),
                authored_url,
                base_url,
                fetch,
                cached,
                result,
            );
        }
        cursor = start + close + 1;
    }
}

fn collect_resource(
    kind: ResourceKind,
    authored_url: &str,
    base_url: Option<&str>,
    fetch: &mut Option<&mut ByteFetcher<'_>>,
    cached: &mut HashMap<String, Option<Vec<u8>>>,
    result: &mut ResolvedDocumentResources,
) {
    if authored_url.starts_with('#') {
        return;
    }
    let resolved_url = resolve_url(base_url, authored_url);
    if result.resources.iter().any(|resource| {
        resource.kind == kind
            && resource.authored_url == authored_url
            && resource.resolved_url == resolved_url
    }) {
        return;
    }
    let Some(fetch) = fetch.as_deref_mut() else {
        result
            .diagnostics
            .push(ResourceDiagnostic::ResourceUnavailable {
                kind,
                authored_url: authored_url.to_owned(),
                resolved_url,
            });
        return;
    };
    let bytes = cached
        .entry(resolved_url.clone())
        .or_insert_with(|| fetch(&resolved_url));
    match bytes {
        Some(bytes) => result.resources.push(ResolvedResource {
            kind,
            authored_url: authored_url.to_owned(),
            resolved_url,
            bytes: bytes.clone(),
        }),
        None if explicitly_unsupported_scheme(&resolved_url) => {
            result
                .diagnostics
                .push(ResourceDiagnostic::UnsupportedScheme {
                    kind,
                    authored_url: authored_url.to_owned(),
                    resolved_url,
                });
        },
        None => result
            .diagnostics
            .push(ResourceDiagnostic::ResourceUnavailable {
                kind,
                authored_url: authored_url.to_owned(),
                resolved_url,
            }),
    }
}

fn collect_text<D>(dom: &D, node: D::NodeId, output: &mut String)
where
    D: LayoutDom,
{
    if dom.kind(node) == NodeKind::Text {
        output.push_str(dom.text(node).unwrap_or(""));
    }
    for child in dom.dom_children(node) {
        collect_text(dom, child, output);
    }
}

fn is_stylesheet_link<D>(dom: &D, node: D::NodeId) -> bool
where
    D: LayoutDom,
{
    let namespace = Namespace::default();
    let rel = LocalName::from("rel");
    dom.element_name(node)
        .is_some_and(|name| name.local.as_ref().eq_ignore_ascii_case("link"))
        && dom.attribute(node, &namespace, &rel).is_some_and(|tokens| {
            tokens
                .split_ascii_whitespace()
                .any(|token| token.eq_ignore_ascii_case("stylesheet"))
        })
}

fn resource_kind_for_css_url(url: &str) -> ResourceKind {
    let path = url
        .split_once(['?', '#'])
        .map_or(url, |(path, _)| path)
        .to_ascii_lowercase();
    if [".woff", ".woff2", ".ttf", ".otf", ".eot"]
        .iter()
        .any(|extension| path.ends_with(extension))
    {
        ResourceKind::Font
    } else {
        ResourceKind::Image
    }
}

fn starts_with_import(css: &str) -> bool {
    let css = css.trim_start_matches(|ch: char| ch.is_whitespace());
    css.get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("@import"))
}

fn explicitly_unsupported_scheme(url: &str) -> bool {
    let scheme = url.split_once(':').map(|(scheme, _)| scheme);
    matches!(scheme, Some("about" | "javascript" | "mailto" | "tel"))
}

/// Resolve an authored URL against the identity of the resource containing it.
/// It keeps bare Windows paths local-first while retaining remote root-relative
/// and scheme-relative URL behavior.
pub fn resolve_url(base_url: Option<&str>, authored_url: &str) -> String {
    let Some(base) = base_url else {
        return authored_url.to_owned();
    };
    if has_scheme(authored_url) {
        return authored_url.to_owned();
    }
    if let Some((scheme, authority_end)) = remote_origin(base) {
        if let Some(network_path) = authored_url.strip_prefix("//") {
            return format!("{scheme}://{network_path}");
        }
        if authored_url.starts_with('/') {
            return format!("{}{}", &base[..authority_end], authored_url);
        }
        let page_end = base.find(['?', '#']).unwrap_or(base.len());
        if authored_url.starts_with('?') || authored_url.starts_with('#') {
            return format!("{}{}", &base[..page_end], authored_url);
        }
        let page = &base[..page_end];
        let path_start = authority_end.min(page.len());
        if let Some(index) = page[path_start..].rfind('/') {
            return format!("{}{}", &page[..path_start + index + 1], authored_url);
        }
        return format!("{page}/{authored_url}");
    }
    if authored_url.starts_with('/') || authored_url.starts_with('\\') {
        return authored_url.to_owned();
    }
    let cut = base.rfind(['/', '\\']).map_or(0, |index| index + 1);
    format!("{}{}", &base[..cut], authored_url)
}

fn has_scheme(url: &str) -> bool {
    match url.find(':') {
        Some(index) if index > 0 => url[..index].chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        }),
        _ => false,
    }
}

fn remote_origin(base: &str) -> Option<(&str, usize)> {
    let scheme_end = base.find("://")?;
    let scheme = &base[..scheme_end];
    if scheme.is_empty()
        || !scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
    {
        return None;
    }
    let after_authority = &base[scheme_end + 3..];
    let authority_len = after_authority
        .find(['/', '?', '#'])
        .unwrap_or(after_authority.len());
    Some((scheme, scheme_end + 3 + authority_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use genet_static_dom::StaticDocument;

    struct Fetch;
    impl ResourceFetcher for Fetch {
        fn fetch(&self, url: &str) -> Option<Vec<u8>> {
            match url {
                "https://example.test/page/site.css" => Some(
                    b".hero { background-image: url(images/hero.png); } @font-face { src: url(fonts/text.woff2); }"
                        .to_vec(),
                ),
                "https://example.test/page/images/hero.png" => Some(vec![1, 2, 3]),
                "https://example.test/page/fonts/text.woff2" => Some(vec![4, 5, 6]),
                "https://example.test/page/logo.png" => Some(vec![7]),
                _ => None,
            }
        }
    }

    #[test]
    fn preserves_interleaved_sheet_order_media_and_source_identity() {
        let document = StaticDocument::parse(
            r#"<style>.first { color: red }</style><link rel="preload stylesheet" href="site.css" media="screen"><style>.last { color: blue }</style>"#,
        );
        let resources = ResolvedDocumentResources::resolve(
            &document,
            Some("https://example.test/page/index.html"),
            &Fetch,
        );
        assert_eq!(resources.stylesheets.len(), 3);
        assert_eq!(resources.stylesheets[0].owner, StylesheetOwner::Inline);
        assert_eq!(resources.stylesheets[1].media.as_deref(), Some("screen"));
        assert_eq!(
            resources.stylesheets[1].source_url.as_deref(),
            Some("https://example.test/page/site.css")
        );
        assert!(resources.stylesheets[2].text.contains("blue"));
        assert!(
            resources
                .resources
                .iter()
                .any(|resource| resource.kind == ResourceKind::Font)
        );
    }

    #[test]
    fn fetch_free_discovery_retains_inline_and_explains_linked_sheet() {
        let document = StaticDocument::parse(
            r#"<style>p { color: red }</style><link rel="stylesheet" href="site.css">"#,
        );
        let resources = ResolvedDocumentResources::discover(
            &document,
            Some("https://example.test/page/index.html"),
        );
        assert_eq!(resources.stylesheets.len(), 1);
        assert!(matches!(
            resources.diagnostics.as_slice(),
            [ResourceDiagnostic::LinkedStylesheetNoByteAuthority { .. }]
        ));
    }

    #[test]
    fn leading_import_is_an_explicit_r5_diagnostic() {
        let document =
            StaticDocument::parse(r#"<style>@import url("later.css"); p { color: red }</style>"#);
        let resources = ResolvedDocumentResources::discover(&document, None);
        assert!(matches!(
            resources.diagnostics.as_slice(),
            [ResourceDiagnostic::ImportRulePendingR5 { .. }, ..]
        ));
    }

    #[test]
    fn resolves_link_and_css_urls_against_their_own_sources() {
        assert_eq!(
            resolve_url(Some("https://example.test/docs/page.html"), "css/site.css"),
            "https://example.test/docs/css/site.css"
        );
        assert_eq!(
            resolve_url(
                Some("https://example.test/docs/css/site.css"),
                "images/logo.png"
            ),
            "https://example.test/docs/css/images/logo.png"
        );
    }
}
