/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! skrifa-backed `FontMetricsProvider` for the cascade runner.
//!
//! Stylo's cascade requires a `FontMetricsProvider` to resolve font-relative
//! units (`ex`, `cap`, `ic`, `ch`). This resolves the element's font through the
//! same `fontique` stack parley shapes with (so the cascade and text layout
//! agree on which font is picked), then reads its metrics with `skrifa`:
//!
//! - `ex` / `x_height` and `cap` / `cap_height` come from the OS/2 table.
//! - `ch` is the advance of the `0` glyph; `ic` is the advance of the CJK water
//!   ideograph (U+6C34).
//!
//! Metrics are read at `Size::new(1.0)` (fractions of the em) and cached per
//! resolved font, then scaled by the query's `font_size`. The cascade is
//! sequential, so the fontique `Collection` + cache live in a `thread_local`
//! (the provider itself is a zero-size, `Sync` handle, as the trait requires).
//! When no font resolves, this falls back to `FontMetrics::default()` (Stylo's
//! blind fallbacks, e.g. `ex = 0.5em`).

use std::cell::RefCell;

use fontique::{
    Attributes, Collection, CollectionOptions, FontStyle as FontiqueStyle, FontWeight, FontWidth,
    GenericFamily, QueryFamily, QueryFont, QueryStatus, SourceCache,
};
use rustc_hash::FxHashMap;
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};
use style::device::servo::FontMetricsProvider;
use style::font_metrics::FontMetrics;
use style::properties::style_structs::Font as FontStyles;
use style::values::computed::font::{
    FontStyle, GenericFontFamily, QueryFontMetricsFlags, SingleFontFamily,
};
use style::values::computed::{CSSPixelLength, Length};

/// Font metrics as fractions of the em square (read at `Size::new(1.0)`), so a
/// query scales them by its `font_size`.
#[derive(Clone, Copy)]
struct PerEm {
    x_height: Option<f32>,
    cap_height: Option<f32>,
    zero_advance: Option<f32>,
    ic_width: Option<f32>,
    ascent: f32,
}

thread_local! {
    static RESOLVER: RefCell<FontResolver> = RefCell::new(FontResolver::new());
}

/// Owns the fontique `Collection` + per-font metrics cache for this thread.
struct FontResolver {
    collection: Collection,
    source_cache: SourceCache,
    cache: FxHashMap<String, Option<PerEm>>,
}

/// One resolved family entry: an owned name (so `QueryFamily::Named(&str)` can
/// borrow it) or a generic.
enum Family {
    Named(String),
    Generic(GenericFamily),
}

impl FontResolver {
    fn new() -> Self {
        Self {
            collection: Collection::new(CollectionOptions::default()),
            source_cache: SourceCache::default(),
            cache: FxHashMap::default(),
        }
    }

    /// Per-em metrics for `font_styles`' resolved font, cached by the request.
    fn per_em(&mut self, font_styles: &FontStyles) -> Option<PerEm> {
        let families: Vec<Family> = font_styles
            .font_family
            .families
            .iter()
            .map(|f| match f {
                SingleFontFamily::FamilyName(n) => Family::Named(n.name.to_string()),
                SingleFontFamily::Generic(g) => Family::Generic(map_generic(g)),
            })
            .collect();
        let weight = font_styles.font_weight.value();
        let italic = font_styles.font_style != FontStyle::NORMAL;

        let mut key = String::new();
        for f in &families {
            match f {
                Family::Named(s) => key.push_str(s),
                Family::Generic(g) => key.push_str(generic_name(*g)),
            }
            key.push('|');
        }
        key.push_str(&format!("{weight}:{}", italic as u8));
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }

        let query_families: Vec<QueryFamily> = families
            .iter()
            .map(|f| match f {
                Family::Named(s) => QueryFamily::Named(s.as_str()),
                Family::Generic(g) => QueryFamily::Generic(*g),
            })
            .collect();
        let attributes = Attributes::new(
            FontWidth::NORMAL,
            if italic {
                FontiqueStyle::Italic
            } else {
                FontiqueStyle::Normal
            },
            FontWeight::new(weight),
        );

        let mut result = None;
        {
            let mut query = self.collection.query(&mut self.source_cache);
            query.set_families(query_families);
            query.set_attributes(attributes);
            query.matches_with(|font| {
                result = read_per_em(font);
                QueryStatus::Stop
            });
        }
        self.cache.insert(key, result);
        result
    }
}

/// Read a matched font's per-em metrics via skrifa.
fn read_per_em(font: &QueryFont) -> Option<PerEm> {
    let font_ref = FontRef::from_index(font.blob.as_ref(), font.index).ok()?;
    let size = Size::new(1.0);
    let location = LocationRef::default();
    let metrics = font_ref.metrics(size, location);
    let glyphs = font_ref.glyph_metrics(size, location);
    let charmap = font_ref.charmap();
    let advance_of = |c: char| charmap.map(c).and_then(|g| glyphs.advance_width(g));
    Some(PerEm {
        x_height: metrics.x_height,
        cap_height: metrics.cap_height,
        zero_advance: advance_of('0'),
        ic_width: advance_of('\u{6c34}'),
        ascent: metrics.ascent,
    })
}

/// Map a Stylo generic family to fontique's. Internal generics (`-moz-*`,
/// `system-ui`, …) fall back to sans-serif.
fn map_generic(g: &GenericFontFamily) -> GenericFamily {
    match g {
        GenericFontFamily::Serif => GenericFamily::Serif,
        GenericFontFamily::SansSerif => GenericFamily::SansSerif,
        GenericFontFamily::Monospace => GenericFamily::Monospace,
        GenericFontFamily::Cursive => GenericFamily::Cursive,
        GenericFontFamily::Fantasy => GenericFamily::Fantasy,
        _ => GenericFamily::SansSerif,
    }
}

/// A stable cache-key fragment for a generic family.
fn generic_name(g: GenericFamily) -> &'static str {
    match g {
        GenericFamily::Serif => "serif",
        GenericFamily::Monospace => "monospace",
        GenericFamily::Cursive => "cursive",
        GenericFamily::Fantasy => "fantasy",
        _ => "sans-serif",
    }
}

/// Resolves font-relative units through fontique + skrifa. Zero-size handle; the
/// font collection + cache live in a `thread_local` (see module docs).
#[derive(Debug)]
pub(crate) struct SkrifaFontMetricsProvider;

impl FontMetricsProvider for SkrifaFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font_styles: &FontStyles,
        font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        let Some(per_em) = RESOLVER.with(|r| r.borrow_mut().per_em(font_styles)) else {
            return FontMetrics::default();
        };
        let s = font_size.px();
        FontMetrics {
            x_height: per_em.x_height.map(|f| Length::new(f * s)),
            zero_advance_measure: per_em.zero_advance.map(|f| Length::new(f * s)),
            cap_height: per_em.cap_height.map(|f| Length::new(f * s)),
            ic_width: per_em.ic_width.map(|f| Length::new(f * s)),
            ascent: Length::new(per_em.ascent * s),
            ..FontMetrics::default()
        }
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        // Default to 16px (browser-conventional medium font size). A real
        // provider would return the font-family-specific user preference.
        Length::new(16.0)
    }
}
