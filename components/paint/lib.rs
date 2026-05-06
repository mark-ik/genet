/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

//! Servo's rendering subsystem. Post-C2 the WebRender wrapper that
//! lived here is gone; this crate now holds the C3 scaffold for the
//! netrender-driven painter.
//!
//! See [`docs/2026-05-05_serval_netrender_cut_plan.md`](../../docs/2026-05-05_serval_netrender_cut_plan.md)
//! for the cut-plan context. Real `netrender::Scene` translation +
//! `Renderer::render_with_compositor` driving live in the proper C3
//! follow-up; this scaffold is the compile-clean staging point.

use std::cell::Cell;
use std::rc::Rc;

use crossbeam_channel::Sender;
use embedder_traits::{EventLoopWaker, ShutdownState};
use paint_api::{PaintMessage, PaintProxy};
use profile_traits::{mem, time};
use servo_base::generic_channel::RoutedReceiver;
use servo_constellation_traits::EmbedderToConstellationMessage;
#[cfg(feature = "webxr")]
use webxr::WebXrRegistry;

pub use crate::netrender_painter::{Paint, WebRenderDebugOption};

mod netrender_painter;

/// Data used to initialize the `Paint` subsystem.
pub struct InitialPaintState {
    /// A channel to `Paint`.
    pub paint_proxy: PaintProxy,
    /// A port on which messages inbound to `Paint` can be received.
    pub receiver: RoutedReceiver<PaintMessage>,
    /// A channel to the constellation.
    pub embedder_to_constellation_sender: Sender<EmbedderToConstellationMessage>,
    /// A channel to the time profiler thread.
    pub time_profiler_chan: time::ProfilerChan,
    /// A channel to the memory profiler thread.
    pub mem_profiler_chan: mem::ProfilerChan,
    /// A shared state which tracks whether Servo has started or has finished
    /// shutting down.
    pub shutdown_state: Rc<Cell<ShutdownState>>,
    /// An [`EventLoopWaker`] used in order to wake up the embedder when it is
    /// time to paint.
    pub event_loop_waker: Box<dyn EventLoopWaker>,
    /// If WebXR is enabled, a [`WebXrRegistry`] to register WebXR threads.
    #[cfg(feature = "webxr")]
    pub webxr_registry: Box<dyn WebXrRegistry>,
}
