/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Desktop presentation tokens for Pelt's trusted chrome.
//!
//! The page renderer owns content styling. This module owns only the native
//! shell and generates its semantic colour roles from a small, configurable
//! seed set through `tinct`, rather than drifting into unrelated literals.

use tinct::{ModeProfile, Palette, Seeds, Srgb, color_to_hex, derive_palette_with};

/// The built-in presentation modes. Hosts can keep the value in their own
/// settings and pass it back through [`crate::Chrome::set_theme`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeltTheme {
    #[default]
    Dark,
    Light,
    HighContrastDark,
    HighContrastLight,
}

impl PeltTheme {
    fn seeds(self) -> Seeds {
        let dark = matches!(self, Self::Dark | Self::HighContrastDark);
        Seeds {
            primary: Srgb::rgb(0x78, 0xA7, 0xFF),
            secondary: Srgb::rgb(0x74, 0xC7, 0xB8),
            tertiary: Srgb::rgb(0xE9, 0xB9, 0x59),
            neutral: if dark {
                Srgb::rgb(0x1C, 0x1D, 0x25)
            } else {
                Srgb::rgb(0xE6, 0xE7, 0xEC)
            },
            text_header: None,
            text_body: None,
            success: Srgb::rgb(0x66, 0xC2, 0x8A),
            danger: Srgb::rgb(0xDB, 0x6B, 0x70),
            dark,
        }
    }

    fn profile(self) -> ModeProfile {
        match self {
            Self::Dark => ModeProfile::DARK,
            Self::Light => ModeProfile::LIGHT,
            Self::HighContrastDark => ModeProfile::HC_DARK,
            Self::HighContrastLight => ModeProfile::HC_LIGHT,
        }
    }
}

/// The generated CSS base for the trusted browser strip.
pub fn chrome_css(theme: PeltTheme) -> String {
    let palette = derive_palette_with(&theme.seeds(), theme.profile());
    chrome_css_from_palette(&palette)
}

fn chrome_css_from_palette(palette: &Palette) -> String {
    let bg = color_to_hex(palette.bg);
    let surface = color_to_hex(palette.surface);
    let surface_2 = color_to_hex(palette.surface_2);
    let hover = color_to_hex(palette.surface_hover);
    let text = color_to_hex(palette.text);
    let dim = color_to_hex(palette.text_dim);
    let disabled = color_to_hex(palette.text_disabled);
    let primary = color_to_hex(palette.primary);
    format!(
        "div, button, span {{ display: block; }} \
         head, style, script, title, meta, link, base {{ display: none; }} \
         .toolbar {{ display: flex; flex-direction: row; align-items: center; background: {surface}; color: {text}; padding: 6px; font-family: sans-serif; }} \
         button {{ padding: 4px 10px; margin-right: 6px; background: {surface_2}; color: {text}; border: 1px solid {hover}; }} \
         button.disabled {{ color: {disabled}; background: {bg}; }} \
         input {{ flex: 1 1 auto; min-width: 0; padding: 6px 8px; color: {text}; background: {bg}; border: 1px solid {dim}; font-family: sans-serif; }} \
         input:focus {{ border: 1px solid {primary}; }}"
    )
}

/// The WCAG relative luminance of an sRGB colour.
fn relative_luminance(c: Srgb) -> f32 {
    let channel = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// The WCAG contrast ratio between two opaque colours, from 1.0 to 21.0.
fn contrast_ratio(fg: Srgb, bg: Srgb) -> f32 {
    let (a, b) = (relative_luminance(fg), relative_luminance(bg));
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sheet is built with `format!`, where a doubled backslash at end of
    /// line emits a literal backslash instead of continuing the line. That put a
    /// stray one between every rule, and the parser kept only the first: the
    /// strip lost `display: flex` and stacked its buttons, and `button` and
    /// `input` lost their colours and rendered dark on dark. One character to
    /// reintroduce, so assert it rather than trusting it.
    #[test]
    fn the_chrome_stylesheet_has_no_stray_escapes() {
        let css = chrome_css(PeltTheme::default());
        assert!(
            !css.contains('\\'),
            "a stray backslash desyncs the parser and drops every later rule: {css}"
        );
        for rule in [".toolbar", "button", "input"] {
            assert!(css.contains(rule), "{rule} missing from the sheet: {css}");
        }
        assert!(
            css.contains("display: flex"),
            "the toolbar must stay a flex row: {css}"
        );
    }

    /// No black on black. Every colour the chrome paints text with has to be
    /// legible against the surface it actually lands on.
    #[test]
    fn chrome_text_is_legible_against_its_own_surfaces() {
        let theme = PeltTheme::default();
        let palette = derive_palette_with(&theme.seeds(), theme.profile());
        // WCAG 1.4.3 exempts an inactive control from the 4.5:1 text minimum,
        // and pelt's nav buttons are disabled until there is history, so that
        // is the state most screenshots catch. Hold it to the 3:1 non-text bar
        // instead: it may read as unavailable, it may not vanish.
        for (name, fg, bg, floor) in [
            ("toolbar text", palette.text, palette.surface, 4.5),
            ("button label", palette.text, palette.surface_2, 4.5),
            ("omnibar text", palette.text, palette.bg, 4.5),
            ("disabled label", palette.text_disabled, palette.bg, 3.0),
        ] {
            let ratio = contrast_ratio(fg, bg);
            assert!(ratio >= floor, "{name} is {ratio:.2}:1, below {floor}:1");
        }
    }
}
