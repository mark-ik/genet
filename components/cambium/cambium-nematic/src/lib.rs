/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cambium-native presentation for Nematic smolweb content.
//!
//! Nematic retains `EngineDocument` lowering, while Errand owns the protocol
//! ASTs consumed here. This crate projects those ASTs into reactive Cambium
//! views. Genet's retained document sessions remain in `genet-documents`.

pub mod views;

pub use views::{SmolwebPalette, SmolwebTheme, SmolwebView, stylesheet};
