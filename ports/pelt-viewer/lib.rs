/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Xilem-based viewer for Pelt.
//!
//! Renders serval HTML content into a Masonry window: the engine
//! pipeline (parse → cascade → layout → emit) produces a paint list,
//! netrender rasterizes it to a texture, the pixels are read back as a
//! `peniko::ImageData`, and Xilem displays them in an `image_view`
//! beneath a navigation bar. Adapted from `wgpu-graft/demo-servo-xilem`
//! (Servo → serval). See `docs/2026-05-20_*` plans.

mod app;
// Netfetcher-backed ResourceFetcher — only with the `netfetch` feature (keeps the
// async network stack out of serval's default build).
#[cfg(feature = "netfetch")]
mod net_fetcher;
mod render;

pub use app::run;
pub use render::build_scene;
#[cfg(feature = "netfetch")]
pub use net_fetcher::NetResourceFetcher;
