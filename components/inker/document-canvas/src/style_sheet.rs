/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Per-role document style sheet.
//!
//! [`DocumentStyleSheet`] is the typography + spacing authority for document
//! layout: a table keyed by semantic block role. Each role carries a full
//! [`BlockStyle`] descriptor (font family, weight, size, italic, color token,
//! wrap policy, spacing); a single [`DocumentStyleSheet::resolve`] call turns a
//! [`BlockRole`] into a [`ResolvedBlockStyle`] the layout pass consumes. This is
//! the seam that lets every document engine (smolweb, markdown, gopher, feed,
//! knot) be styled per block-kind and, later, user-customised — the
//! "customizable like Geopard" capability, generalised past Gemtext.
//!
//! `default()` is the built-in look (14px body, system-ui / monospace
//! families, the near-black light palette). Colors are named via
//! [`ColorToken`] and resolved against the sheet's [`ColorVocabulary`]; a
//! later phase sources that palette from the live theme. See
//! `design_docs/inker_docs/implementation_strategy/2026-06-21_document_style_sheet_plan.md`.

use serde::{Deserialize, Serialize};

use crate::style::ColorVocabulary;

/// Where a role's font comes from. `Inherit*` defer to the sheet's global
/// family names; `Explicit` names a face directly (for user overrides).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontChoice {
    /// The sheet's `body_font_family`.
    InheritBody,
    /// The sheet's `mono_font_family`. Also marks the role monospace.
    InheritMono,
    /// A named face, resolved by the host's font system.
    Explicit(String),
}

/// How a role's font size is expressed. `ScaleOfBody` multiplies the sheet's
/// `body_font_size`; `Absolute` is a fixed pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SizeSpec {
    Absolute(f32),
    ScaleOfBody(f32),
}

/// Whether a role's text wraps to the available width. `NoWrap` is a P4
/// affordance (the renderer horizontally scrolls); the default sheet wraps
/// every role, matching today's behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WrapPolicy {
    Wrap,
    NoWrap,
}

/// A semantic color slot, resolved against the sheet's [`ColorVocabulary`]
/// (and, in a later phase, the live theme). Naming a token rather than a
/// literal RGBA means a single palette change re-themes every role that uses
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorToken {
    BodyText,
    HeadingText,
    LinkText,
    CodeText,
    BadgeText,
    Rule,
}

/// How to adorn inline links. `SchemeArrow` is the Geopard-style prefix: an
/// arrow chosen by whether the link leaves the document's own protocol. The
/// prefix renders as part of the link (link-colored, inside the hit region).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkAdornment {
    /// No prefix; the link text renders as-is.
    None,
    /// `⇒ ` (U+21D2) for in-protocol / relative links, `⇗ ` (U+21D7) for
    /// links that leave the document's protocol.
    SchemeArrow,
}

impl LinkAdornment {
    /// The prefix string for a link, or `None` if no adornment applies.
    pub fn prefix_for(self, url: &str, base_scheme: Option<&str>) -> Option<&'static str> {
        match self {
            LinkAdornment::None => None,
            LinkAdornment::SchemeArrow => Some(if link_is_external(url, base_scheme) {
                "\u{21d7} " // ⇗ leaves the document's protocol
            } else {
                "\u{21d2} " // ⇒ stays in-protocol (or relative)
            }),
        }
    }
}

/// The scheme of a URL — the part before the first `:`, if it is a valid
/// scheme. Relative URLs (no scheme) return `None`. Used to derive a
/// document's base scheme from its address and to classify its links.
pub fn url_scheme(url: &str) -> Option<&str> {
    let idx = url.find(':')?;
    let scheme = &url[..idx];
    let valid_start = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic());
    let valid_rest = scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'));
    if valid_start && valid_rest {
        Some(scheme)
    } else {
        None
    }
}

/// Whether a link leaves the document's protocol. Relative and same-scheme
/// links are in-protocol; a different scheme is external. With no known base
/// scheme, an absolute link is treated as external.
fn link_is_external(url: &str, base_scheme: Option<&str>) -> bool {
    match (url_scheme(url), base_scheme) {
        (None, _) => false,
        (Some(s), Some(b)) => !s.eq_ignore_ascii_case(b),
        (Some(_), None) => true,
    }
}

/// Which role to resolve. Carries the heading level inline, since heading
/// size is intrinsically per-level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRole {
    Body,
    Heading(u8),
    Code,
    Metadata,
    Badge,
}

/// Style descriptor for a single block role.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockStyle {
    pub family: FontChoice,
    /// CSS-style weight (400 = normal, 700 = bold). Layout treats `>= 700` as
    /// bold for parity with the current `bool` flag.
    pub weight: u16,
    pub size: SizeSpec,
    pub italic: bool,
    pub color: ColorToken,
    pub wrap: WrapPolicy,
    /// Vertical space (px) above the block.
    pub spacing_above: f32,
    /// Vertical space (px) below the block.
    pub spacing_below: f32,
}

/// Heading style. Distinct from [`BlockStyle`] because heading size is a
/// per-level array, not a single [`SizeSpec`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeadingStyle {
    pub family: FontChoice,
    pub weight: u16,
    /// Font size per level (index 0 = h1, …, index 5 = h6).
    pub sizes: [f32; 6],
    pub italic: bool,
    pub color: ColorToken,
    pub wrap: WrapPolicy,
    pub spacing_above: f32,
    pub spacing_below: f32,
}

/// The per-role descriptors. Named fields rather than a map: typed,
/// serde-clean, and there is a fixed, small set of text-bearing roles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RoleStyles {
    pub body: BlockStyle,
    pub heading: HeadingStyle,
    pub code: BlockStyle,
    pub metadata: BlockStyle,
    pub badge: BlockStyle,
}

/// A role resolved against a sheet: exactly the fields the layout pass needs
/// to build a `TextBaseStyle` plus its block spacing, color, and wrap policy.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBlockStyle {
    pub font_size: f32,
    pub font_family: String,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    pub line_height_ratio: f32,
    pub spacing_above: f32,
    pub spacing_below: f32,
    /// Premultiplied RGBA in 0..=1 (the `netrender::Scene` format). The
    /// block's base text color; the layout pass bakes it onto each glyph run.
    pub color: [f32; 4],
    /// Wrap policy: `Wrap` constrains the block to the content width; `NoWrap`
    /// lays it out on its natural width and overflows for the host to scroll
    /// (e.g. code blocks). The default sheet wraps every role.
    pub wrap: WrapPolicy,
}

/// The document style sheet: global metrics + per-role descriptors.
///
/// [`Self::default`] is the built-in look. The per-block constants that used
/// to be flat fields (paragraph spacing, heading spacing, heading sizes) now
/// live inside the role descriptors in [`RoleStyles`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentStyleSheet {
    /// Base body size (px). `ScaleOfBody` sizes multiply this.
    pub body_font_size: f32,
    /// Body font family name (resolved by the host's font system).
    pub body_font_family: String,
    /// Monospace family name for code blocks + inline code.
    pub mono_font_family: String,
    /// Line-height multiplier shared by all roles (e.g. 1.4 = 140%).
    pub line_height_ratio: f32,
    /// Horizontal indent (px) per nesting level for quotes / lists.
    pub indent_per_level: f32,
    /// Horizontal padding (px) inside the viewport.
    pub horizontal_padding: f32,
    /// Vertical padding (px) at the top + bottom of the viewport.
    pub vertical_padding: f32,
    /// Palette the [`ColorToken`]s resolve against.
    pub colors: ColorVocabulary,
    /// How inline links are adorned (the `⇒` / `⇗` scheme arrows).
    pub link_adornment: LinkAdornment,
    /// Per-role style descriptors.
    pub roles: RoleStyles,
}

impl DocumentStyleSheet {
    /// Block spacing for non-text blocks (image / rule) that currently use the
    /// paragraph spacing. Equals the body role's `spacing_below`.
    pub fn block_spacing(&self) -> f32 {
        self.roles.body.spacing_below
    }

    /// Font size for a heading at `level` (1..=6). Levels outside the range
    /// clamp to h1 / h6.
    pub fn heading_size(&self, level: u8) -> f32 {
        let idx = level.clamp(1, 6) as usize - 1;
        self.roles.heading.sizes[idx]
    }

    /// Line height (px) for a given font size, using the sheet's shared ratio.
    pub fn line_height(&self, font_size: f32) -> f32 {
        font_size * self.line_height_ratio
    }

    /// Resolve a role into the concrete attributes the layout pass needs.
    pub fn resolve(&self, role: BlockRole) -> ResolvedBlockStyle {
        match role {
            BlockRole::Heading(level) => {
                let h = &self.roles.heading;
                ResolvedBlockStyle {
                    font_size: self.heading_size(level),
                    font_family: self.family_name(&h.family),
                    bold: h.weight >= 700,
                    italic: h.italic,
                    monospace: matches!(h.family, FontChoice::InheritMono),
                    line_height_ratio: self.line_height_ratio,
                    spacing_above: h.spacing_above,
                    spacing_below: h.spacing_below,
                    color: self.token_color(h.color),
                    wrap: h.wrap,
                }
            }
            BlockRole::Body => self.resolve_block(&self.roles.body),
            BlockRole::Code => self.resolve_block(&self.roles.code),
            BlockRole::Metadata => self.resolve_block(&self.roles.metadata),
            BlockRole::Badge => self.resolve_block(&self.roles.badge),
        }
    }

    fn resolve_block(&self, s: &BlockStyle) -> ResolvedBlockStyle {
        ResolvedBlockStyle {
            font_size: self.size_px(&s.size),
            font_family: self.family_name(&s.family),
            bold: s.weight >= 700,
            italic: s.italic,
            monospace: matches!(s.family, FontChoice::InheritMono),
            line_height_ratio: self.line_height_ratio,
            spacing_above: s.spacing_above,
            spacing_below: s.spacing_below,
            color: self.token_color(s.color),
            wrap: s.wrap,
        }
    }

    fn size_px(&self, size: &SizeSpec) -> f32 {
        match size {
            SizeSpec::Absolute(px) => *px,
            SizeSpec::ScaleOfBody(factor) => self.body_font_size * factor,
        }
    }

    fn family_name(&self, choice: &FontChoice) -> String {
        match choice {
            FontChoice::InheritBody => self.body_font_family.clone(),
            FontChoice::InheritMono => self.mono_font_family.clone(),
            FontChoice::Explicit(name) => name.clone(),
        }
    }

    /// Resolve a [`ColorToken`] to premultiplied RGBA against the sheet's
    /// palette. The layout pass reads this for inline link / code colors (the
    /// per-role base color rides on the [`ResolvedBlockStyle`]).
    pub fn token_color(&self, token: ColorToken) -> [f32; 4] {
        match token {
            ColorToken::BodyText => self.colors.body_text,
            ColorToken::HeadingText => self.colors.heading_text,
            ColorToken::LinkText => self.colors.link_text,
            ColorToken::CodeText => self.colors.code_text,
            ColorToken::BadgeText => self.colors.badge_text,
            ColorToken::Rule => self.colors.rule,
        }
    }
}

impl Default for DocumentStyleSheet {
    /// The built-in look: 14px body, system-ui / monospace families, the
    /// near-black light palette. Reproduces the typography the document
    /// renderers carried before the sheet existed.
    fn default() -> Self {
        Self {
            body_font_size: 14.0,
            body_font_family: "system-ui".to_string(),
            mono_font_family: "monospace".to_string(),
            line_height_ratio: 1.4,
            indent_per_level: 24.0,
            horizontal_padding: 16.0,
            vertical_padding: 16.0,
            colors: ColorVocabulary::default(),
            link_adornment: LinkAdornment::SchemeArrow,
            roles: RoleStyles {
                body: BlockStyle {
                    family: FontChoice::InheritBody,
                    weight: 400,
                    size: SizeSpec::ScaleOfBody(1.0),
                    italic: false,
                    color: ColorToken::BodyText,
                    wrap: WrapPolicy::Wrap,
                    spacing_above: 0.0,
                    spacing_below: 12.0,
                },
                heading: HeadingStyle {
                    family: FontChoice::InheritBody,
                    weight: 700,
                    sizes: [28.0, 22.0, 18.0, 16.0, 14.0, 12.0],
                    italic: false,
                    color: ColorToken::HeadingText,
                    wrap: WrapPolicy::Wrap,
                    spacing_above: 16.0,
                    spacing_below: 8.0,
                },
                code: BlockStyle {
                    family: FontChoice::InheritMono,
                    weight: 400,
                    size: SizeSpec::ScaleOfBody(1.0),
                    italic: false,
                    color: ColorToken::CodeText,
                    wrap: WrapPolicy::Wrap,
                    spacing_above: 0.0,
                    spacing_below: 12.0,
                },
                metadata: BlockStyle {
                    family: FontChoice::InheritBody,
                    weight: 400,
                    size: SizeSpec::ScaleOfBody(1.0),
                    italic: false,
                    color: ColorToken::BodyText,
                    wrap: WrapPolicy::Wrap,
                    spacing_above: 0.0,
                    spacing_below: 6.0,
                },
                badge: BlockStyle {
                    family: FontChoice::InheritBody,
                    weight: 400,
                    size: SizeSpec::ScaleOfBody(0.85),
                    italic: true,
                    color: ColorToken::BadgeText,
                    wrap: WrapPolicy::Wrap,
                    spacing_above: 0.0,
                    spacing_below: 6.0,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- The default sheet reproduces the built-in document look ---
    //
    // These assertions pin the exact values the layout renderers carried
    // before the sheet existed (the byte-identical contract for the routing
    // that consumes them in `layout.rs`).

    #[test]
    fn body_role_matches_paragraph_defaults() {
        let sheet = DocumentStyleSheet::default();
        let r = sheet.resolve(BlockRole::Body);
        assert_eq!(r.font_size, 14.0);
        assert_eq!(r.font_family, "system-ui");
        assert!(!r.bold);
        assert!(!r.italic);
        assert!(!r.monospace);
        assert_eq!(r.line_height_ratio, 1.4);
        assert_eq!(r.spacing_above, 0.0);
        assert_eq!(r.spacing_below, 12.0);
        assert_eq!(r.color, ColorVocabulary::default().body_text);
        assert_eq!(r.wrap, WrapPolicy::Wrap);
    }

    #[test]
    fn heading_role_matches_heading_defaults_every_level() {
        let sheet = DocumentStyleSheet::default();
        let expected = [28.0, 22.0, 18.0, 16.0, 14.0, 12.0];
        for level in 1..=6u8 {
            let r = sheet.resolve(BlockRole::Heading(level));
            assert_eq!(r.font_size, expected[level as usize - 1], "h{level} size");
            assert_eq!(r.font_family, "system-ui");
            assert!(r.bold, "h{level} bold");
            assert!(!r.italic);
            assert!(!r.monospace);
            assert_eq!(r.spacing_above, 16.0);
            assert_eq!(r.spacing_below, 8.0);
            assert_eq!(r.color, ColorVocabulary::default().heading_text);
        }
    }

    #[test]
    fn heading_level_clamps_to_h1_h6() {
        let sheet = DocumentStyleSheet::default();
        // Level 0 clamps to h1, level 7+ clamps to h6.
        assert_eq!(sheet.resolve(BlockRole::Heading(0)).font_size, 28.0);
        assert_eq!(sheet.resolve(BlockRole::Heading(9)).font_size, 12.0);
        assert_eq!(sheet.heading_size(0), 28.0);
        assert_eq!(sheet.heading_size(99), 12.0);
    }

    #[test]
    fn code_role_matches_code_block_defaults() {
        let sheet = DocumentStyleSheet::default();
        let r = sheet.resolve(BlockRole::Code);
        assert_eq!(r.font_size, 14.0);
        assert_eq!(r.font_family, "monospace");
        assert!(r.monospace);
        assert!(!r.bold);
        assert!(!r.italic);
        assert_eq!(r.spacing_below, 12.0);
        assert_eq!(r.color, ColorVocabulary::default().code_text);
    }

    #[test]
    fn metadata_role_matches_metadata_row_defaults() {
        let sheet = DocumentStyleSheet::default();
        let r = sheet.resolve(BlockRole::Metadata);
        assert_eq!(r.font_size, 14.0);
        assert_eq!(r.font_family, "system-ui");
        assert!(!r.bold);
        assert!(!r.italic);
        assert!(!r.monospace);
        assert_eq!(r.spacing_below, 6.0);
        assert_eq!(r.color, ColorVocabulary::default().body_text);
    }

    #[test]
    fn badge_role_matches_badge_defaults() {
        let sheet = DocumentStyleSheet::default();
        let r = sheet.resolve(BlockRole::Badge);
        // The renderer used `body_font_size * 0.85`; ScaleOfBody must match it
        // bit-for-bit.
        assert_eq!(r.font_size, 14.0 * 0.85);
        assert_eq!(r.font_family, "system-ui");
        assert!(r.italic);
        assert!(!r.bold);
        assert!(!r.monospace);
        assert_eq!(r.spacing_below, 6.0);
        assert_eq!(r.color, ColorVocabulary::default().badge_text);
    }

    #[test]
    fn block_spacing_equals_paragraph_spacing() {
        let sheet = DocumentStyleSheet::default();
        assert_eq!(sheet.block_spacing(), 12.0);
    }

    #[test]
    fn globals_are_built_in_defaults() {
        let sheet = DocumentStyleSheet::default();
        assert_eq!(sheet.body_font_size, 14.0);
        assert_eq!(sheet.body_font_family, "system-ui");
        assert_eq!(sheet.mono_font_family, "monospace");
        assert_eq!(sheet.line_height_ratio, 1.4);
        assert_eq!(sheet.indent_per_level, 24.0);
        assert_eq!(sheet.horizontal_padding, 16.0);
        assert_eq!(sheet.vertical_padding, 16.0);
        assert_eq!(sheet.colors, ColorVocabulary::default());
    }

    #[test]
    fn line_height_uses_the_shared_ratio() {
        let sheet = DocumentStyleSheet::default();
        assert_eq!(sheet.line_height(14.0), 14.0 * 1.4);
    }

    #[test]
    fn explicit_family_and_absolute_size_resolve() {
        let mut sheet = DocumentStyleSheet::default();
        sheet.roles.body.family = FontChoice::Explicit("Iosevka".into());
        sheet.roles.body.size = SizeSpec::Absolute(20.0);
        let r = sheet.resolve(BlockRole::Body);
        assert_eq!(r.font_family, "Iosevka");
        assert_eq!(r.font_size, 20.0);
        assert!(!r.monospace, "explicit family is not implicitly monospace");
    }

    #[test]
    fn sheet_round_trips_through_serde() {
        let sheet = DocumentStyleSheet::default();
        let json = serde_json::to_string(&sheet).expect("serialize");
        let back: DocumentStyleSheet = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(sheet, back);
    }
}
