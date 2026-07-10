/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Font-registration seam between the host and document-canvas's layout.
//!
//! parley needs fonts during *layout* (to shape text into glyph IDs +
//! positions). The host implements [`FontResolver`] to register its font
//! collection with parley's `FontContext` via
//! [`crate::text::LayoutEnvironment::with_resolver`]; layout-time shaping
//! then sees those faces.
//!
//! ## No render-side resolution
//!
//! There is deliberately no render-side method. Each
//! [`GlyphRun`](crate::GlyphRun) records the
//! [`FontFaceId`](crate::FontFaceId) of the face parley *actually* shaped
//! it against, and the real bytes ride in the
//! [`FontTable`](crate::FontTable) sidecar (see [`crate::font_table`]).
//! The paint-list producer ships those exact bytes, so the glyph ids and
//! the face can't desync on fallback.
//!
//! (Earlier a `resolve_font_data` method re-resolved a
//! `(family, weight, style)` label at emit time and shipped *that* face's
//! bytes — the wrong-face-on-fallback bug. It is removed: the bytes now
//! come from parley's own shaping.)

/// Trait the host implements to provide fonts for layout (parley).
pub trait FontResolver: Send + Sync {
    /// Register all fonts this resolver provides with parley's font
    /// context. Called once when the [`crate::text::LayoutEnvironment`] is
    /// constructed via [`crate::text::LayoutEnvironment::with_resolver`].
    ///
    /// Default impl is a no-op, so a host relying on parley's bundled /
    /// system fonts needs no boilerplate.
    fn register_with_parley(&self, _font_cx: &mut parley::FontContext) {}
}

/// A no-op resolver: registers nothing with parley (it falls back to its
/// own bundled / system defaults). Useful as a default when the host
/// hasn't wired a custom font collection — text still renders, since the
/// faces the renderer needs come from parley's shaping, not the resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFontResolver;

impl FontResolver for NoFontResolver {}
