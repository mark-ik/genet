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

// The interop module is the only place where this crate touches
// platform-native APIs (D3D12 fence creation, wgpu-hal `as_hal`
// access). The crate-level `deny(unsafe_code)` is preserved
// everywhere else.
#[allow(unsafe_code)]
pub mod interop;

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

#[allow(deprecated)]
pub use crate::compositor::{
    OsCompositorBackend, PaintCompositor, ServoCompositor, StubCompositor,
    WgpuMasterCaptureBackend,
};
#[cfg(target_os = "windows")]
pub use crate::compositor_dxgi::{BackendError as WindowsDxgiBackendError, WindowsDxgiBackend};
#[cfg(target_vendor = "apple")]
pub use crate::compositor_calayer::{
    BackendError as MacosCALayerBackendError, MacosCALayerBackend,
};
#[cfg(target_os = "linux")]
pub use crate::compositor_wayland::{
    BackendError as WaylandSubsurfaceBackendError, WaylandSubsurfaceBackend,
};
pub use crate::interop::{HostWgpuContext, InteropBackend, InteropError, SyncMechanism};
#[cfg(target_os = "windows")]
pub use crate::interop::Dx12FenceSynchronizer;
pub use crate::netrender_painter::{Paint, WebRenderDebugOption};
pub use crate::translator::translate_display_list;

mod compositor;
#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
mod compositor_calayer;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod compositor_dxgi;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod compositor_wayland;
mod netrender_painter;
mod translator;

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
