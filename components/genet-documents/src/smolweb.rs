/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Retained smolweb documents through Genet's engine-native document path.
//!
//! Nematic parses protocol content into an [`inker::EngineDocument`].
//! `document-canvas` owns layout, visible-band derivation, link regions, and
//! PaintList lowering. This module retains that packet plus viewport scroll
//! and exposes the existing session API to Pelt and Mere.

use document_canvas::{
    ColorVocabulary, DocumentStyleSheet, InteractionKind, LaidOutDocument, Viewport,
    layout_document, netrender_backend::scene_from_packet,
};
use genet_layout::ScrollKey;
use inker::{Engine, EngineDocument, EngineInput};
use netrender::Scene;
use pelt_core::ResourceFetcher;

/// How an engine-native smolweb document is colored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SmolwebTheme {
    /// A stable palette derived from the capsule host.
    Site,
    /// A neutral light palette.
    Plain,
    /// A warm fixed light palette.
    Light,
    /// A fixed dark palette.
    Dark,
    /// Colors supplied by the application host.
    App(SmolwebPalette),
    /// Host-resolved system theme. Light is the fallback.
    System,
}

impl Default for SmolwebTheme {
    fn default() -> Self {
        Self::Site
    }
}

/// Compatibility palette used by current Pelt and Mere hosts.
///
/// New engine-native callers may configure [`DocumentStyleSheet`] directly
/// through [`SmolwebDocument::from_document`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmolwebPalette {
    pub bg: String,
    pub fg: String,
    pub link: String,
    pub quote: String,
    pub pre_bg: String,
}

/// A retained engine document, its document-canvas layout, and host viewport.
pub struct SmolwebDocument {
    document: EngineDocument,
    style: DocumentStyleSheet,
    background: [f32; 4],
    layout: Option<LaidOutDocument>,
    size: (u32, u32),
    scroll_y: f32,
}

impl SmolwebDocument {
    /// Fetch `url` through the host fetcher, then lower and retain the body.
    pub fn load(
        fetcher: &impl ResourceFetcher,
        url: &str,
        theme: SmolwebTheme,
    ) -> Result<Self, String> {
        let bytes = fetcher
            .fetch(url)
            .ok_or_else(|| format!("could not load {url}"))?;
        Ok(Self::parse(url, &String::from_utf8_lossy(&bytes), theme))
    }

    /// Lower already-fetched content through the matching Nematic engine.
    pub fn parse(url: &str, body: &str, theme: SmolwebTheme) -> Self {
        let document = lower(url, body);
        let (style, background) = style_for_theme(&theme, url);
        Self::from_document(document, style, background)
    }

    /// Retain an already-lowered document with an explicit host style.
    pub fn from_document(
        document: EngineDocument,
        style: DocumentStyleSheet,
        background: [f32; 4],
    ) -> Self {
        Self {
            document,
            style,
            background,
            layout: None,
            size: (0, 0),
            scroll_y: 0.0,
        }
    }

    /// The portable document retained by this session.
    pub fn document(&self) -> &EngineDocument {
        &self.document
    }

    fn ensure_layout(&mut self, width: u32, height: u32) {
        let size = (width.max(1), height.max(1));
        if self.layout.is_some() && self.size == size {
            return;
        }
        self.layout = Some(layout_document(
            &self.document,
            Viewport::new(size.0 as f32, size.1 as f32),
            &self.style,
        ));
        self.size = size;
        self.scroll_y = self.scroll_y.min(self.max_scroll());
    }

    fn max_scroll(&self) -> f32 {
        let Some(layout) = &self.layout else {
            return 0.0;
        };
        (layout.packet.content_bounds.size.height - self.size.1 as f32).max(0.0)
    }

    /// Paint the visible document band at the retained scroll offset.
    pub fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.ensure_layout(width, height);
        let layout = self.layout.as_ref().expect("layout built above");
        let packet = layout.packet.window(self.scroll_y, self.size.1 as f32);
        let mut scene = scene_from_packet(&packet, &layout.fonts, &self.style.colors);
        scene.push_rect(
            0.0,
            0.0,
            self.size.0 as f32,
            self.size.1 as f32,
            self.background,
        );
        let background = scene.ops.pop().expect("push_rect appended an op");
        scene.ops.insert(0, background);
        scene
    }

    /// Move the single host-owned document viewport.
    pub fn scroll_by(&mut self, _dx: f32, dy: f32) -> bool {
        if self.layout.is_none() {
            return false;
        }
        let before = self.scroll_y;
        self.scroll_y = (self.scroll_y + dy).clamp(0.0, self.max_scroll());
        self.scroll_y != before
    }

    /// document-canvas has one viewport scroller, so point routing delegates
    /// to [`scroll_by`](Self::scroll_by).
    pub fn scroll_at(&mut self, _x: f32, _y: f32, dx: f32, dy: f32) -> bool {
        self.scroll_by(dx, dy)
    }

    /// Apply the established Genet keyboard-scroll vocabulary.
    pub fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        if self.layout.is_none() {
            return false;
        }
        let before = self.scroll_y;
        let page = self.size.1 as f32 * 0.9;
        self.scroll_y = match key {
            ScrollKey::Up => self.scroll_y - 40.0,
            ScrollKey::Down => self.scroll_y + 40.0,
            ScrollKey::PageUp => self.scroll_y - page,
            ScrollKey::PageDown => self.scroll_y + page,
            ScrollKey::Home => 0.0,
            ScrollKey::End => self.max_scroll(),
            ScrollKey::Left | ScrollKey::Right => self.scroll_y,
        }
        .clamp(0.0, self.max_scroll());
        self.scroll_y != before
    }

    /// Jump to an absolute full-document offset.
    pub fn scroll_to(&mut self, y: f32) {
        if self.layout.is_some() {
            self.scroll_y = y.clamp(0.0, self.max_scroll());
        }
    }

    /// Full laid-out content height, floored to the viewport height.
    pub fn content_height(&mut self, width: u32, height: u32) -> u32 {
        self.ensure_layout(width, height);
        self.layout
            .as_ref()
            .expect("layout built above")
            .packet
            .content_bounds
            .size
            .height
            .ceil()
            .max(height.max(1) as f32) as u32
    }

    /// Full-document link rectangles as `[x0, y0, x1, y1]`.
    pub fn links(&self) -> Vec<(String, [f32; 4])> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        layout
            .packet
            .interactions
            .iter()
            .map(|region| {
                let url = match &region.kind {
                    InteractionKind::Link { url } => url.clone(),
                };
                let rect = region.bounds;
                (
                    url,
                    [
                        rect.origin.x,
                        rect.origin.y,
                        rect.origin.x + rect.size.width,
                        rect.origin.y + rect.size.height,
                    ],
                )
            })
            .collect()
    }

    /// Resolve a viewport-local click through the retained full-document packet.
    pub fn click_at(&mut self, x: f32, y: f32, width: u32, height: u32) -> Option<String> {
        self.ensure_layout(width, height);
        self.layout
            .as_ref()?
            .packet
            .link_at(x, y + self.scroll_y)
            .map(str::to_string)
    }
}

fn lower(url: &str, body: &str) -> EngineDocument {
    let scheme = url
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or("");
    let engine: Box<dyn Engine> = match scheme {
        "gopher" => Box::new(nematic::GopherEngine::new()),
        "nex" => Box::new(nematic::NexEngine::new()),
        "finger" => Box::new(nematic::FingerEngine::new()),
        "spartan" => Box::new(nematic::SpartanEngine::new()),
        "titan" => Box::new(nematic::TitanEngine::new()),
        "misfin" => Box::new(nematic::MisfinEngine::new()),
        "guppy" => Box::new(nematic::GuppyEngine::new()),
        "scroll" => Box::new(nematic::ScrollEngine::new()),
        _ if looks_like_feed(body) => Box::new(nematic::FeedEngine::new()),
        _ => Box::new(nematic::GemtextEngine::new()),
    };
    let input = EngineInput::new(url, body);
    engine.render(&input).unwrap_or_else(|_| {
        nematic::GemtextEngine::new()
            .render(&input)
            .expect("gemtext lowering is infallible")
    })
}

fn looks_like_feed(body: &str) -> bool {
    let body = body.trim_start();
    body.starts_with("<?xml") || body.starts_with("<rss") || body.starts_with("<feed")
}

fn style_for_theme(theme: &SmolwebTheme, url: &str) -> (DocumentStyleSheet, [f32; 4]) {
    let palette = match theme {
        SmolwebTheme::Site => site_palette(url),
        SmolwebTheme::Plain => fixed_palette("#ffffff", "#1a1a1a", "#0b57d0", "#555555", "#f4f4f4"),
        SmolwebTheme::Light | SmolwebTheme::System => {
            fixed_palette("#fbfaf7", "#23211c", "#1a6e57", "#5b574e", "#f0eee8")
        },
        SmolwebTheme::Dark => fixed_palette("#16181c", "#e6e3dc", "#7db4ff", "#a8a49a", "#21242a"),
        SmolwebTheme::App(palette) => palette.clone(),
    };
    let defaults = ColorVocabulary::default();
    let background = parse_color(&palette.bg).unwrap_or([1.0, 1.0, 1.0, 1.0]);
    let foreground = parse_color(&palette.fg).unwrap_or(defaults.body_text);
    let link = parse_color(&palette.link).unwrap_or(defaults.link_text);
    let quote = parse_color(&palette.quote).unwrap_or(defaults.badge_text);
    let pre = parse_color(&palette.pre_bg).unwrap_or(defaults.placeholder_image);
    let mut style = DocumentStyleSheet::default();
    style.body_font_family = "serif".into();
    style.line_height_ratio = 1.5;
    style.horizontal_padding = 32.0;
    style.vertical_padding = 24.0;
    style.colors = ColorVocabulary {
        body_text: foreground,
        heading_text: foreground,
        link_text: link,
        code_text: foreground,
        badge_text: quote,
        rule: quote,
        placeholder_text: foreground,
        placeholder_image: pre,
    };
    (style, background)
}

fn fixed_palette(bg: &str, fg: &str, link: &str, quote: &str, pre_bg: &str) -> SmolwebPalette {
    SmolwebPalette {
        bg: bg.into(),
        fg: fg.into(),
        link: link.into(),
        quote: quote.into(),
        pre_bg: pre_bg.into(),
    }
}

fn site_palette(url: &str) -> SmolwebPalette {
    let hue = hue_from_host(url);
    let css = |saturation, lightness| {
        let [r, g, b, _] = hsl(hue as f32, saturation, lightness);
        format!(
            "rgb({}, {}, {})",
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8
        )
    };
    SmolwebPalette {
        bg: css(0.30, 0.97),
        fg: css(0.25, 0.15),
        link: css(0.70, 0.36),
        quote: css(0.18, 0.40),
        pre_bg: css(0.28, 0.93),
    }
}

fn hue_from_host(url: &str) -> u16 {
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    let hash = host.bytes().fold(5381_u32, |hash, byte| {
        hash.wrapping_mul(33).wrapping_add(u32::from(byte))
    });
    (hash % 360) as u16
}

fn hsl(hue: f32, saturation: f32, lightness: f32) -> [f32; 4] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = (hue.rem_euclid(360.0)) / 60.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match sector as u8 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    [r + m, g + m, b + m, 1.0]
}

fn parse_color(value: &str) -> Option<[f32; 4]> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let (r, g, b) = match hex.len() {
            3 => {
                let mut chars = hex.chars();
                let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
                (
                    expand(chars.next()?)?,
                    expand(chars.next()?)?,
                    expand(chars.next()?)?,
                )
            },
            6 => (
                u8::from_str_radix(&hex[0..2], 16).ok()?,
                u8::from_str_radix(&hex[2..4], 16).ok()?,
                u8::from_str_radix(&hex[4..6], 16).ok()?,
            ),
            _ => return None,
        };
        return Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]);
    }
    let body = value.strip_prefix("rgb(")?.strip_suffix(')')?;
    let channels: Vec<u8> = body
        .split(',')
        .map(|channel| channel.trim().parse())
        .collect::<Result<_, _>>()
        .ok()?;
    (channels.len() == 3).then(|| {
        [
            channels[0] as f32 / 255.0,
            channels[1] as f32 / 255.0,
            channels[2] as f32 / 255.0,
            1.0,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemtext_uses_engine_document_and_paints_text() {
        let mut doc = SmolwebDocument::parse(
            "gemini://x.test/",
            "# Hello\n\nWorld.\n",
            SmolwebTheme::Site,
        );
        assert_eq!(
            doc.document().provenance.source_kind.as_deref(),
            Some(nematic::ENGINE_GEMTEXT)
        );
        let scene = doc.frame(800, 600);
        assert!(
            scene
                .ops
                .iter()
                .any(|op| matches!(op, netrender::SceneOp::GlyphRun(_)))
        );
        assert!(matches!(
            scene.ops.first(),
            Some(netrender::SceneOp::Rect(_))
        ));
    }

    #[test]
    fn long_document_scrolls_and_windows() {
        let body: String = (0..200).map(|i| format!("Line {i}\n\n")).collect();
        let mut doc = SmolwebDocument::parse("gemini://x.test/", &body, SmolwebTheme::Plain);
        assert!(doc.content_height(400, 300) > 300);
        assert!(!doc.scroll_by(0.0, -50.0));
        assert!(doc.scroll_by(0.0, 240.0));
        assert_eq!(doc.frame(400, 300).viewport_height, 300);
    }

    #[test]
    fn links_and_clicks_use_full_document_coordinates() {
        let mut doc = SmolwebDocument::parse(
            "gemini://x.test/",
            "=> gemini://x.test/page A link\n",
            SmolwebTheme::Plain,
        );
        assert!(doc.links().is_empty());
        let _ = doc.frame(400, 300);
        let (url, [x0, y0, x1, y1]) = doc.links().into_iter().next().expect("link region");
        assert_eq!(url, "gemini://x.test/page");
        assert!(x1 > x0 && y1 > y0);
        assert_eq!(
            doc.click_at((x0 + x1) / 2.0, (y0 + y1) / 2.0, 400, 300)
                .as_deref(),
            Some("gemini://x.test/page")
        );
    }

    #[test]
    fn schemes_select_their_nematic_engines() {
        let gopher = SmolwebDocument::parse(
            "gopher://x.test/",
            "1Files\t/files\tx.test\t70\r\n",
            SmolwebTheme::Plain,
        );
        assert_eq!(
            gopher.document().provenance.source_kind.as_deref(),
            Some(nematic::ENGINE_GOPHER)
        );

        let feed = SmolwebDocument::parse(
            "gemini://x.test/feed",
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Log</title></channel></rss>",
            SmolwebTheme::Dark,
        );
        assert_eq!(
            feed.document().provenance.source_kind.as_deref(),
            Some(nematic::ENGINE_FEED)
        );
    }

    #[test]
    fn app_rgb_palette_maps_into_document_style() {
        let theme = SmolwebTheme::App(SmolwebPalette {
            bg: "rgb(16, 32, 48)".into(),
            fg: "rgb(250, 250, 250)".into(),
            link: "rgb(51, 204, 255)".into(),
            quote: "rgb(153, 170, 187)".into(),
            pre_bg: "rgb(10, 22, 34)".into(),
        });
        let (style, background) = style_for_theme(&theme, "gemini://x.test/");
        assert_eq!(background, [16.0 / 255.0, 32.0 / 255.0, 48.0 / 255.0, 1.0]);
        assert_eq!(
            style.colors.link_text,
            [51.0 / 255.0, 204.0 / 255.0, 1.0, 1.0]
        );
    }
}
