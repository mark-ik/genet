/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cambium-native presentation for Nematic smolweb content.
//!
//! Nematic retains parsing and `EngineDocument` lowering. This crate owns the
//! reactive view projection and, behind the `document` feature, a Genet-backed
//! retained document adapter. Hosts fetch bytes and pass the address and body
//! to that adapter; transport stays above this layer.

#[cfg(feature = "document")]
mod document;
pub mod views;

#[cfg(feature = "document")]
pub use document::SmolwebDocument;
pub use views::{SmolwebPalette, SmolwebTheme, SmolwebView, stylesheet};

#[cfg(feature = "document")]
pub(crate) const STRUCTURAL_SHEET: &[&str] = &[
    "html, body, main, div, section, article, nav, header, footer, h1, h2, h3, h4, h5, h6, p, pre, ul, ol, li { display: block; }",
    "a { display: inline; }",
    "body { margin: 0; }",
];
