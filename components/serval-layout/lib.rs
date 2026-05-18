/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

//! Profile-neutral layout engine for serval.
//!
//! Consumes any `LayoutDom`-shaped DOM and produces planes
//! (`StylePlane`, eventually `LayoutPlane`, `FragmentPlane`) per the
//! planes architecture in
//! `docs/2026-05-17_serval_layout_planes_architecture.md`.
//!
//! Probe slice (2026-05-17): minimum end-to-end is wired —
//! `NodeRef` (foreign-trait firewall for Stylo, draft impls in
//! `adapter_stylo.rs` deferred) + `StylePlane` (hand-built today; cascade
//! populates later) + `construct` (DOM → Taffy tree) + `taffy::compute_root_layout`
//! + `FragmentPlane` (per-node rects).

mod adapter;
mod adapter_stylo;
mod cascade;
mod cell;
mod construct;
mod font_metrics;
mod fragment;
mod layout;
mod style;

pub use adapter::NodeRef;
pub use adapter_stylo::StyleNodeRef;
pub use cascade::run_cascade;
pub use cell::ArcRefCell;
pub use construct::{construct, ConstructedTree};
pub use fragment::FragmentPlane;
pub use layout::layout;
pub use style::{StyleEntry, StylePlane};
