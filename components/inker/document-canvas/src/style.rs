/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Color + inline-style primitives for document layout.
//!
//! [`ColorVocabulary`] is the theme palette downstream renderers read;
//! [`InlineStyle`] is the per-span brush parley layout carries. The
//! per-role typography + spacing knobs live in
//! [`DocumentStyleSheet`](crate::style_sheet::DocumentStyleSheet), which
//! resolves [`ColorVocabulary`] entries via its color tokens.

use serde::{Deserialize, Serialize};

/// Theme primitives used by downstream renderers. All colors are
/// premultiplied RGBA in 0..=1 space (the format `netrender::Scene`
/// expects directly).
///
/// The defaults are a "near-black on transparent" palette suitable for
/// light themes; callers tune for their host. Per-style brushes
/// (link-text, code-text, etc.) are stored separately so a single field
/// change re-themes one block kind.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorVocabulary {
    pub body_text: [f32; 4],
    pub heading_text: [f32; 4],
    pub link_text: [f32; 4],
    pub code_text: [f32; 4],
    pub badge_text: [f32; 4],
    pub rule: [f32; 4],
    /// Tint for placeholder rects emitted when a renderer can't (yet)
    /// emit real glyphs — e.g. when no font resolver is registered for a
    /// given family.
    pub placeholder_text: [f32; 4],
    /// Tint for image placeholder strips (before the host fetches the
    /// real image bytes).
    pub placeholder_image: [f32; 4],
}

impl Default for ColorVocabulary {
    fn default() -> Self {
        Self {
            body_text: [0.05, 0.05, 0.08, 1.0],
            heading_text: [0.02, 0.02, 0.05, 1.0],
            link_text: [0.10, 0.30, 0.70, 1.0],
            code_text: [0.20, 0.05, 0.10, 1.0],
            badge_text: [0.30, 0.30, 0.40, 1.0],
            rule: [0.40, 0.40, 0.40, 1.0],
            placeholder_text: [0.05, 0.05, 0.08, 0.10],
            placeholder_image: [0.50, 0.50, 0.60, 0.20],
        }
    }
}

/// Per-inline-span style attributes. Computed during inline-flattening and
/// applied to parley as ranged style properties. Used as parley's `Brush`
/// type — must satisfy `Clone + PartialEq + Default + Debug`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InlineStyle {
    pub italic: bool,
    pub bold: bool,
    pub monospace: bool,
    pub link: bool,
}

impl InlineStyle {
    pub const NORMAL: Self = Self {
        italic: false,
        bold: false,
        monospace: false,
        link: false,
    };

    pub fn with_italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn with_bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn with_monospace(mut self) -> Self {
        self.monospace = true;
        self
    }

    pub fn with_link(mut self) -> Self {
        self.link = true;
        self
    }
}
