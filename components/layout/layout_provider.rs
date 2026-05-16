/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Single entry point for the concrete Layout DOM provider types used by
//! `servo-layout`. Localising these re-exports here is the first step of P2
//! (remove `script` from `servo-layout`). Once layout is parameterised over
//! `LayoutDomTypeBundle` and the storage trait, this module collapses to imports
//! from `layout_api` / `shared/layout` and the `script` dependency can drop.
//!
//! See [`docs/2026-05-13_p2_layout_dom_provider_design.md`].

pub(crate) use script::layout_dom::{
    ServoDangerousStyleDocument, ServoDangerousStyleElement, ServoLayoutElement, ServoLayoutNode,
};
