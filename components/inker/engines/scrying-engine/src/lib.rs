/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # scrying-engine
//!
//! `inker::SurfaceEngine` implementation backed by the
//! [`scrying`](https://crates.io/crates/scrying) system-WebView producer
//! (WebView2 on Windows, WKWebView on macOS, WPE/WebKitGTK on Linux).
//!
//! The crate ships:
//!
//! - [`ScryingTileEngine`] — the `inker::SurfaceEngine` factory, registered
//!   with [`inker::SurfaceEngineRegistry`] as `scrying.web`.
//! - [`ScryingProducer`] — adapter mapping a
//!   `Box<dyn scrying::WebSurfaceProducer>` onto `inker::SurfaceProducer`.
//! - [`ProducerFactory`] — the host-side hook that owns the parts the engine
//!   can't fabricate (parent HWND / NSView, wgpu device, fence handle) and
//!   builds a concrete scrying producer per spawn.
//!
//! See `design_docs/mere_docs/implementation_strategy/2026-06-10_scrying_tile_plan.md`
//! for the full integration plan. (Note: meerkat's shipped X1 host pool binds
//! the platform producer concretely and does not currently route through this
//! `SurfaceEngine` registry seam; folding it in is the inker engine-picker
//! plan's Phase 0.)

#![doc(html_root_url = "https://docs.rs/scrying-engine/0.0.1")]

pub mod engine;
pub mod producer;
pub mod translation;

pub use engine::{ProducerFactory, SCRYING_WEB_ENGINE_ID, ScryingTileEngine};
pub use producer::ScryingProducer;
