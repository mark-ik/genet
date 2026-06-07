/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stub `FontMetricsProvider` for the cascade runner.
//!
//! Stylo's cascade requires a `FontMetricsProvider` to resolve font-relative
//! units (`ex`, `cap`, `ic`, `ch`). This stub returns `FontMetrics::default()`
//! for every query, so those units resolve to fallback defaults (e.g. `ex`
//! becomes `0.5em`). The cascade still produces correct computed values for
//! everything that does not depend on real font metrics.
//!
//! Parley is wired in for inline measurement (`text_measure.rs`), but this
//! cascade-time provider is not yet backed by it. Replacing this stub with one
//! that queries parley's font collection (mirroring Blitz's
//! `BlitzFontMetricsProvider` in `blitz-dom/src/font_metrics.rs`) is the
//! remaining step for real font-relative units.

use style::device::servo::FontMetricsProvider;
use style::font_metrics::FontMetrics;
use style::properties::style_structs::Font as FontStyles;
use style::values::computed::font::{GenericFontFamily, QueryFontMetricsFlags};
use style::values::computed::{CSSPixelLength, Length};

#[derive(Clone, Debug)]
pub(crate) struct StubFontMetricsProvider;

impl FontMetricsProvider for StubFontMetricsProvider {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        _font_styles: &FontStyles,
        _font_size: CSSPixelLength,
        _flags: QueryFontMetricsFlags,
    ) -> FontMetrics {
        FontMetrics::default()
    }

    fn base_size_for_generic(&self, _generic: GenericFontFamily) -> Length {
        // Default to 16px (browser-conventional medium font size). A real
        // provider would return the font-family-specific user preference.
        Length::new(16.0)
    }
}
