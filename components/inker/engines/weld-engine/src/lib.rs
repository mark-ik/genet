/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # weld-engine
//!
//! `inker::SurfaceEngine` implementation backed by the
//! [`wgpu-weld`](https://github.com/mark-ik/wgpu-weld) CEF embedding: a bundled
//! Chromium (via CEF) renders in accelerated off-screen-rendering (OSR) mode and
//! its `OnAcceleratedPaint` output is imported into a host `wgpu` texture
//! (D3D12-shared on Windows, IOSurface/Metal on macOS, DMABUF/Vulkan on Linux).
//!
//! ## Why a host-seam instead of wrapping `welding::CefSurfaceProducer`
//!
//! weld imports frames internally and exposes a wgpu `ImportedTexture`, which
//! does not fit inker's raw-`NativeTextureHandle` `SurfaceFrame` (and inker is
//! wgpu-free). So, like [`graft-engine`](../graft-engine), this crate defines the
//! [`WeldSurface`] host-seam (yielding the raw shared handle + the import-once
//! `is_new` bit) and the host implements it over `welding`'s `CefSurfaceProducer`
//! and `CefRuntime`. Keeping this crate inker-only means the CEF / Chromium
//! distribution never resolves into the mere workspace.
//!
//! ## The subprocess tax (host responsibility, cannot live here)
//!
//! CEF spawns its renderer/GPU/utility processes by re-executing the host
//! binary. The host MUST call `welding::CefRuntime::execute_process_from` at the
//! very start of `main()` (before winit/wgpu) and exit if it returns an exit
//! code. This crate cannot do that — it is the host's `main()` that re-executes.
//! See [`WeldSurface`].
//!
//! ## Shape (mirrors graft-engine / scrying-engine)
//!
//! - [`WeldEngine`] — the `inker::SurfaceEngine`, registered as `weld.chromium`.
//! - [`WeldProducerFactory`] — host hook building a [`WeldSurface`] per spawn.
//! - [`WeldProducer`] — adapts a `Box<dyn WeldSurface>` onto
//!   `inker::SurfaceProducer`.

#![doc(html_root_url = "https://docs.rs/weld-engine/0.0.1")]

pub mod engine;
pub mod producer;

pub use engine::{WELD_CHROMIUM_ENGINE_ID, WeldEngine, WeldProducerFactory};
pub use producer::{WeldFrame, WeldProducer, WeldSurface};
