/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! # graft-engine
//!
//! `inker::SurfaceEngine` implementation backed by the
//! [`wgpu-graft`](https://github.com/mark-ik/wgpu-graft) Servo embedding: an
//! embedded Servo instance renders offscreen and its GL framebuffer is imported
//! into a host `wgpu` texture (Vulkan external memory on Linux, IOSurface/Metal
//! on macOS, ANGLE-D3D11 -> DX12 shared texture on Windows).
//!
//! ## Why a host-seam instead of wrapping a library producer
//!
//! [`scrying-engine`](../scrying-engine) adapts one library trait
//! (`scrying::WebSurfaceProducer`). graft has no equivalent single type: a graft
//! "producer" is a composite of a `servo::Servo` instance, a `servo::WebView`,
//! and `servo_wgpu_interop_adapter::ServoWgpuInteropAdapter`. So this crate
//! defines the [`GraftSurface`] host-seam and the host implements it over those
//! live types. That keeps this crate dependency-light (inker only): pulling
//! Servo in here would make every `cargo` command in the mere workspace resolve
//! the Servo git tree, defeating the per-engine build gating.
//!
//! ## Shape (mirrors scrying-engine)
//!
//! - [`GraftEngine`] — the `inker::SurfaceEngine`, registered as `graft.servo`.
//! - [`GraftProducerFactory`] — host hook that builds a [`GraftSurface`] per
//!   spawn (it owns the wgpu device, the Servo instance, and the interop
//!   adapter — the parts this crate cannot fabricate).
//! - [`GraftProducer`] — adapts a `Box<dyn GraftSurface>` onto
//!   `inker::SurfaceProducer`.
//!
//! See the engine-picker plan
//! (`design_docs/inker_docs/implementation_strategy/2026-06-15_engine_picker_and_pluggability_plan.md`,
//! Phase 5) for how this slots into the multi-engine multiplexer alongside
//! `scrying-engine` and `weld-engine`, and for the feature-gating model.

#![doc(html_root_url = "https://docs.rs/graft-engine/0.0.1")]

pub mod engine;
pub mod producer;

pub use engine::{GRAFT_SERVO_ENGINE_ID, GraftEngine, GraftProducerFactory};
pub use producer::{GraftFrame, GraftProducer, GraftSurface};
