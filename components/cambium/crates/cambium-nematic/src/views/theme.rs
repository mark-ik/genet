/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Theming for smolweb views.
//!
//! The default is a **per-site palette**: the document's host is hashed into a
//! hue and the whole palette derives from it, so each capsule has its own
//! consistent color identity (the Lagrange approach). Presets override it: a
//! neutral `Plain` sheet, fixed `Light` / `Dark`, the host app palette (`App`),
//! or the OS scheme (`System`).
//!
//! A view emits semantic elements with classes; [`stylesheet`] produces the CSS
//! for those classes under a theme. The host applies it the way it applies any
//! document stylesheet (genet-layout's `inline_stylesheets`).

/// How a smolweb document is colored.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SmolwebTheme {
    /// Default: a palette derived from the site's host, so each capsule has its
    /// own consistent color identity (the Lagrange approach).
    #[default]
    Site,
    /// A neutral, un-themed default stylesheet.
    Plain,
    /// A fixed light theme.
    Light,
    /// A fixed dark theme.
    Dark,
    /// The host application's palette — the host supplies it (e.g. derived from its
    /// tinct theme seeds), so smolweb pages match the surrounding app chrome.
    App(SmolwebPalette),
    /// Follow the OS light/dark scheme. The host resolves the OS scheme and passes
    /// the matching theme; absent that, this renders light.
    System,
}

/// A document palette: the colours the smolweb stylesheet is built from. The fixed
/// themes construct one internally; a host supplies its own through
/// [`SmolwebTheme::App`] (e.g. mapped from tinct), the seam that lets smolweb pages
/// match the app theme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmolwebPalette {
    /// Page background.
    pub bg: String,
    /// Body text.
    pub fg: String,
    /// Link colour.
    pub link: String,
    /// Quote text / border, and other muted accents.
    pub quote: String,
    /// Preformatted / code-block background.
    pub pre_bg: String,
}

/// The CSS for a smolweb document under `theme`. `site_url` seeds the per-site
/// palette for [`SmolwebTheme::Site`] (ignored by the fixed and app themes).
pub fn stylesheet(theme: SmolwebTheme, site_url: &str) -> String {
    let palette = match theme {
        SmolwebTheme::Site => site_palette(site_url),
        SmolwebTheme::Plain => plain(),
        // System is host-resolved to light/dark before it reaches here; absent a
        // resolution it renders light.
        SmolwebTheme::Light | SmolwebTheme::System => light(),
        SmolwebTheme::Dark => dark(),
        SmolwebTheme::App(palette) => palette,
    };
    render_css(&palette)
}

fn plain() -> SmolwebPalette {
    SmolwebPalette {
        bg: "#ffffff".into(),
        fg: "#1a1a1a".into(),
        link: "#0b57d0".into(),
        quote: "#555555".into(),
        pre_bg: "#f4f4f4".into(),
    }
}

fn light() -> SmolwebPalette {
    SmolwebPalette {
        bg: "#fbfaf7".into(),
        fg: "#23211c".into(),
        link: "#1a6e57".into(),
        quote: "#5b574e".into(),
        pre_bg: "#f0eee8".into(),
    }
}

fn dark() -> SmolwebPalette {
    SmolwebPalette {
        bg: "#16181c".into(),
        fg: "#e6e3dc".into(),
        link: "#7db4ff".into(),
        quote: "#a8a49a".into(),
        pre_bg: "#21242a".into(),
    }
}

/// A light palette tinted by the site's host hue — the Lagrange per-site look.
fn site_palette(site_url: &str) -> SmolwebPalette {
    let hue = hue_from_host(site_url);
    SmolwebPalette {
        bg: format!("hsl({hue}, 30%, 97%)"),
        fg: format!("hsl({hue}, 25%, 15%)"),
        link: format!("hsl({hue}, 70%, 36%)"),
        quote: format!("hsl({hue}, 18%, 40%)"),
        pre_bg: format!("hsl({hue}, 28%, 93%)"),
    }
}

/// Hash the URL's host into a hue in `0..360`. Stable per host, so a capsule keeps
/// one identity across its pages. djb2 over the host bytes.
fn hue_from_host(site_url: &str) -> u16 {
    let after_scheme = site_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(site_url);
    let host = after_scheme.split('/').next().unwrap_or("");
    let mut hash: u32 = 5381;
    for byte in host.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(byte));
    }
    (hash % 360) as u16
}

fn render_css(p: &SmolwebPalette) -> String {
    let SmolwebPalette {
        bg,
        fg,
        link,
        quote,
        pre_bg,
    } = p;
    format!(
        ".gemtext {{ background:{bg}; color:{fg}; padding:1.5rem 2rem; \
line-height:1.5; font-family:serif; max-width:48rem; }}
.gemtext-h1 {{ font-size:1.8rem; font-weight:700; margin:1.2rem 0 0.6rem; }}
.gemtext-h2 {{ font-size:1.4rem; font-weight:700; margin:1.1rem 0 0.5rem; }}
.gemtext-h3 {{ font-size:1.15rem; font-weight:700; margin:1rem 0 0.4rem; }}
.gemtext-text {{ margin:0.5rem 0; }}
.gemtext-linkline {{ margin:0.25rem 0; }}
.gemtext-link {{ color:{link}; text-decoration:none; }}
.gemtext-link:hover {{ text-decoration:underline; }}
.gemtext-list {{ margin:0.5rem 0; padding-left:1.5rem; }}
.gemtext-item {{ margin:0.15rem 0; }}
.gemtext-quote {{ margin:0.6rem 0; padding:0.2rem 0 0.2rem 1rem; \
border-left:3px solid {quote}; color:{quote}; font-style:italic; }}
.gemtext-pre {{ background:{pre_bg}; padding:0.75rem 1rem; overflow-x:auto; \
font-family:monospace; white-space:pre; margin:0.6rem 0; }}
.gopher {{ background:{bg}; color:{fg}; padding:1.5rem 2rem; line-height:1.5; \
font-family:serif; max-width:48rem; }}
.gopher-info {{ background:{pre_bg}; font-family:monospace; white-space:pre; \
overflow-x:auto; padding:0.5rem 1rem; margin:0.4rem 0; }}
.gopher-error {{ color:{quote}; font-style:italic; margin:0.25rem 0; }}
.gopher-itemline {{ margin:0.2rem 0; }}
.gopher-type {{ color:{quote}; font-family:monospace; font-size:0.85em; \
margin-right:0.5rem; }}
.gopher-link {{ color:{link}; text-decoration:none; }}
.gopher-link:hover {{ text-decoration:underline; }}
.feed {{ background:{bg}; color:{fg}; padding:1.5rem 2rem; line-height:1.5; \
font-family:serif; max-width:48rem; }}
.feed-title {{ font-size:1.8rem; font-weight:700; margin:0.5rem 0 0.2rem; }}
.feed-subtitle {{ color:{quote}; margin:0 0 1rem; }}
.feed-entry {{ border-top:1px solid {pre_bg}; padding:0.9rem 0; }}
.feed-entry-title {{ font-size:1.2rem; font-weight:600; margin:0 0 0.2rem; }}
.feed-entry-link {{ color:{link}; text-decoration:none; }}
.feed-entry-link:hover {{ text-decoration:underline; }}
.feed-entry-date {{ color:{quote}; font-size:0.85em; }}
.feed-entry-summary {{ margin:0.3rem 0 0; }}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_hue_is_stable_and_host_scoped() {
        // Same host, different paths -> same hue (one identity per capsule).
        let a = stylesheet(SmolwebTheme::Site, "gemini://example.test/a");
        let b = stylesheet(SmolwebTheme::Site, "gemini://example.test/b/c");
        assert_eq!(a, b);
        // Different hosts -> different palettes (with overwhelming likelihood).
        let other = stylesheet(SmolwebTheme::Site, "gemini://elsewhere.test/");
        assert_ne!(a, other);
    }

    #[test]
    fn presets_render_their_classes() {
        for theme in [SmolwebTheme::Plain, SmolwebTheme::Light, SmolwebTheme::Dark] {
            let css = stylesheet(theme, "");
            assert!(css.contains(".gemtext-link"));
            assert!(css.contains(".gemtext-pre"));
        }
    }

    #[test]
    fn default_is_site() {
        assert_eq!(SmolwebTheme::default(), SmolwebTheme::Site);
    }

    #[test]
    fn app_theme_uses_the_host_palette() {
        let palette = SmolwebPalette {
            bg: "#102030".into(),
            fg: "#fafafa".into(),
            link: "#33ccff".into(),
            quote: "#99aabb".into(),
            pre_bg: "#0a1622".into(),
        };
        let css = stylesheet(SmolwebTheme::App(palette), "gemini://x.test/");
        // The host colours appear verbatim, and the site hue is ignored.
        assert!(css.contains("#102030"), "uses the host background");
        assert!(css.contains("color:#33ccff"), "uses the host link colour");
        assert!(!css.contains("hsl("), "App does not derive a per-site hue");
    }
}
