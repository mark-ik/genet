/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

//! Profile-neutral layout engine for serval.
//!
//! Consumes any `LayoutDom`-shaped DOM (initially [`serval-static-dom`]; later
//! a scripted-DOM adapter) and emits `ServalDisplayList` for the paint stage.
//! The lift from dead-on-disk `components/layout/` lands here batch-by-batch
//! per `docs/2026-05-16_serval_layout_lift_plan.md` (P2.3).

mod adapter;
// `adapter_stylo.rs` exists as an in-progress draft of the Stylo trait
// impls; it is intentionally **not** mod-declared here yet because its
// method signatures haven't been reconciled against the actual stylo
// trait surface. See the file's header for next-session strategy.
mod cell;

pub use adapter::NodeRef;
pub use cell::ArcRefCell;
