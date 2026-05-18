/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Stub `FontMetricsProvider` for the cascade runner.
//!
//! Stylo's cascade requires a `FontMetricsProvider` to resolve font-relative
//! units (`ex`, `cap`, `ic`, `ch`). For the v1 cascade integration we don't
//! have parley wired in yet (that's step (2) in the roadmap), so this stub
//! returns `FontMetrics::default()` for every query.
//!
//! Effect on rendering: font-relative units resolve to fallback defaults
//! (e.g., `ex` becomes `0.5em`). The cascade still produces useful
//! computed values for everything that doesn't depend on real font metrics.
//!
//! When parley wires in for step (2), this stub is replaced with a real
//! impl that queries parley's font collection for metrics (mirroring Blitz's
//! `BlitzFontMetricsProvider` in `blitz-dom/src/font_metrics.rs`).

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
        // Default to 16px (browser-conventional medium font size). When
        // parley wires in for step (2), this returns size from the
        // font-family-specific user preference.
        Length::new(16.0)
    }
}
