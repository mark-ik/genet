//! Perceptual syntax-highlight palette: a contrast-gated colour per highlight
//! role, derived from the same [`Seeds`] the base [`Palette`] uses.
//!
//! A host's lexer (the `illume` text lexer, say) emits its own token kinds; the
//! host maps each onto a [`SyntaxRole`] here, and the colour is derived. The role
//! set is small and canonical on purpose, so any highlighter fits and the
//! derivation stays coherent: accents are OKLCH hue-steps fanned off the brand
//! primary at a shared chroma, each nudged in lightness until it clears WCAG
//! contrast against the surface; muted roles ride the base palette's dim text;
//! emphasis carries no own hue (the host applies weight / italic). Reseed the
//! theme and the whole syntax palette rotates with the brand.

use crate::oklch::Oklch;
use crate::{contrast, derive_palette, Palette, Seeds, Srgb};

/// A canonical highlight role. A host maps its lexer's finer token kinds onto
/// these, and [`derive_syntax_palette`] gives each a themed colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SyntaxRole {
    // Prose / document structure.
    Heading,
    Emphasis,
    Strong,
    Link,
    Quote,
    Verbatim,
    // Code tokens.
    Keyword,
    Type,
    Function,
    String,
    Number,
    Comment,
    Punctuation,
    // Inline entities (any prose, the omnibar included).
    Url,
    Mention,
    Tag,
}

impl SyntaxRole {
    /// Every role, in declaration order.
    pub const ALL: [SyntaxRole; 16] = [
        Self::Heading,
        Self::Emphasis,
        Self::Strong,
        Self::Link,
        Self::Quote,
        Self::Verbatim,
        Self::Keyword,
        Self::Type,
        Self::Function,
        Self::String,
        Self::Number,
        Self::Comment,
        Self::Punctuation,
        Self::Url,
        Self::Mention,
        Self::Tag,
    ];
}

/// A themed colour per [`SyntaxRole`], plus the surface it was gated against (so a
/// host can re-check or blend). Look a role up with [`SyntaxPalette::role`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyntaxPalette {
    pub surface: Srgb,
    pub heading: Srgb,
    pub emphasis: Srgb,
    pub strong: Srgb,
    pub link: Srgb,
    pub quote: Srgb,
    pub verbatim: Srgb,
    pub keyword: Srgb,
    pub type_: Srgb,
    pub function: Srgb,
    pub string: Srgb,
    pub number: Srgb,
    pub comment: Srgb,
    pub punctuation: Srgb,
    pub url: Srgb,
    pub mention: Srgb,
    pub tag: Srgb,
}

impl SyntaxPalette {
    /// The colour for `role`.
    pub fn role(&self, role: SyntaxRole) -> Srgb {
        match role {
            SyntaxRole::Heading => self.heading,
            SyntaxRole::Emphasis => self.emphasis,
            SyntaxRole::Strong => self.strong,
            SyntaxRole::Link => self.link,
            SyntaxRole::Quote => self.quote,
            SyntaxRole::Verbatim => self.verbatim,
            SyntaxRole::Keyword => self.keyword,
            SyntaxRole::Type => self.type_,
            SyntaxRole::Function => self.function,
            SyntaxRole::String => self.string,
            SyntaxRole::Number => self.number,
            SyntaxRole::Comment => self.comment,
            SyntaxRole::Punctuation => self.punctuation,
            SyntaxRole::Url => self.url,
            SyntaxRole::Mention => self.mention,
            SyntaxRole::Tag => self.tag,
        }
    }
}

/// WCAG contrast floor a syntax colour must clear against the surface.
const MIN_CONTRAST: f64 = 4.5;
/// Shared chroma for the fanned accents (readable saturation, not neon).
const ACCENT_C: f64 = 0.13;

/// Nudge `col`'s lightness toward the text end until it clears [`MIN_CONTRAST`]
/// against `surface` (or it hits the lightness rail), then return sRGB. Measures
/// contrast on the post-gamut-clamp colour, so out-of-gamut accents still gate.
fn gate(mut col: Oklch, surface: Srgb, dark: bool) -> Srgb {
    for _ in 0..40 {
        if contrast(col.to_srgb(), surface) >= MIN_CONTRAST {
            break;
        }
        col = if dark { col.lighten(0.02) } else { col.darken(0.02) };
        if !(0.04..=0.96).contains(&col.l) {
            break;
        }
    }
    col.to_srgb()
}

/// Derive a contrast-gated [`SyntaxPalette`] from the seeds. Accents fan off the
/// brand primary's hue at [`ACCENT_C`] and a per-mode base lightness, each gated
/// against the derived surface; muted roles ride the base palette's dim text;
/// emphasis / strong carry no own hue (the host applies weight).
pub fn derive_syntax_palette(seeds: &Seeds) -> SyntaxPalette {
    let base: Palette = derive_palette(seeds);
    let surface = base.surface;
    let dark = seeds.dark;
    let base_l = if dark { 0.74 } else { 0.46 };
    let primary_h = Oklch::from_srgb(seeds.primary).h;

    // An accent `offset` degrees off the primary hue: shared chroma + base
    // lightness, gated for contrast against the surface.
    let accent = |offset: f64| -> Srgb {
        let col = Oklch {
            l: base_l,
            c: ACCENT_C,
            h: primary_h,
        }
        .rotate_hue(offset);
        gate(col, surface, dark)
    };

    SyntaxPalette {
        surface,
        // Structure: heading is the brand primary, prominent; emphasis / strong
        // ride the text tiers (weight + italic carry them); quote is dimmed.
        heading: gate(
            Oklch::from_srgb(seeds.primary)
                .with_c(ACCENT_C * 1.2)
                .with_l(base_l),
            surface,
            dark,
        ),
        emphasis: base.text,
        strong: base.text_header,
        link: accent(-40.0),
        quote: base.text_dim,
        verbatim: accent(180.0),
        // Code accents, fanned around the wheel so kinds stay distinguishable.
        keyword: accent(0.0),
        function: accent(40.0),
        type_: accent(80.0),
        string: accent(150.0),
        number: accent(210.0),
        comment: base.text_dim,
        punctuation: base.text_dim,
        // Inline entities: links / urls share the navigational hue; mention + tag
        // take their own.
        url: accent(-40.0),
        mention: accent(115.0),
        tag: accent(245.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeds() -> Seeds {
        Seeds {
            primary: Srgb::rgb(0x33, 0x66, 0xC8),
            secondary: Srgb::rgb(0x2E, 0x9D, 0xA6),
            tertiary: Srgb::rgb(0xE0, 0xA8, 0x46),
            neutral: Srgb::rgb(0x10, 0x14, 0x22),
            text_header: None,
            text_body: None,
            success: Srgb::rgb(0x4F, 0xB3, 0x6E),
            danger: Srgb::rgb(0xD5, 0x4E, 0x4E),
            dark: true,
        }
    }

    #[test]
    fn fanned_accents_clear_contrast() {
        let pal = derive_syntax_palette(&seeds());
        for role in [
            SyntaxRole::Heading,
            SyntaxRole::Keyword,
            SyntaxRole::Type,
            SyntaxRole::Function,
            SyntaxRole::String,
            SyntaxRole::Number,
            SyntaxRole::Link,
            SyntaxRole::Url,
            SyntaxRole::Mention,
            SyntaxRole::Tag,
            SyntaxRole::Verbatim,
        ] {
            let c = contrast(pal.role(role), pal.surface);
            assert!(c >= 4.0, "{role:?} contrast {c:.2} too low against surface");
        }
    }

    #[test]
    fn accents_are_distinct() {
        let pal = derive_syntax_palette(&seeds());
        assert_ne!(pal.keyword, pal.string);
        assert_ne!(pal.string, pal.number);
        assert_ne!(pal.keyword, pal.function);
        assert_ne!(pal.mention, pal.tag);
    }

    #[test]
    fn role_lookup_matches_fields() {
        let pal = derive_syntax_palette(&seeds());
        assert_eq!(pal.role(SyntaxRole::Keyword), pal.keyword);
        assert_eq!(pal.role(SyntaxRole::Tag), pal.tag);
        assert_eq!(pal.role(SyntaxRole::Type), pal.type_);
    }

    #[test]
    fn light_mode_also_gates() {
        let mut s = seeds();
        s.dark = false;
        let pal = derive_syntax_palette(&s);
        assert!(contrast(pal.keyword, pal.surface) >= 4.0);
        assert!(contrast(pal.string, pal.surface) >= 4.0);
    }

    #[test]
    fn all_covers_every_role() {
        // ALL and `role` agree, so a host can build a full lookup table.
        let pal = derive_syntax_palette(&seeds());
        for role in SyntaxRole::ALL {
            let _ = pal.role(role);
        }
        assert_eq!(SyntaxRole::ALL.len(), 16);
    }
}
