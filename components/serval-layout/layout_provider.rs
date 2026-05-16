/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Bridge from `serval-layout`'s internal type expectations to the
//! profile-neutral `layout_dom_api::LayoutDom` surface.
//!
//! Replaces the dead-on-disk `layout_provider.rs` stub that re-exported
//! `script::layout_dom::*`. Where layout used to name `ServoLayoutNode`,
//! `ServoLayoutElement`, etc. directly, callers will eventually take a
//! `LayoutNodeRef<'a, D>` / `LayoutElementRef<'a, D>` parameterized over
//! `D: LayoutDom`.
//!
//! P2.3 work in progress: file-bulk-port is done; the generic refactor
//! that propagates `D` through layout's internal types is the next pass.
