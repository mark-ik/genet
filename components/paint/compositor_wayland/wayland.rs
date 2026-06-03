/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Wayland connection + globals binding for the C4 backend.
//!
//! Wraps a `wayland_backend::client::Backend` over the embedder's
//! `wl_display` pointer (via `Backend::from_foreign_display`, then
//! `Connection::from_backend`), runs `registry_queue_init` to bind the
//! required globals (`wl_compositor`, `wl_subcompositor`,
//! `zwp_linux_dmabuf_v1`, `wp_viewporter`) and the optional
//! `wp_alpha_modifier_v1`, drains the dmabuf format/modifier
//! advertisements, and dispatches per-frame events (notably
//! `wl_buffer.release`).
//!
//! ## API drift notes
//!
//! `wayland-client 0.31.x` does NOT expose `Connection::from_ptr`.
//! The canonical "adopt an embedder's wl_display" path is:
//!   1. `wayland_backend::sys::client::Backend::from_foreign_display(*mut wl_display)`
//!      (re-exported as `wayland_client::backend::Backend`; requires the
//!      `system` cargo feature on `wayland-client`).
//!   2. `Connection::from_backend(backend)`.
//!
//! Similarly, `*mut wl_surface` becomes a `WlSurface` proxy via:
//!   1. `ObjectId::from_ptr(WlSurface::interface(), ptr.cast())` (2-arg
//!      signature in wayland-backend 0.3.15).
//!   2. `WlSurface::from_id(&connection, id)`.

#![allow(unsafe_op_in_unsafe_fn)]

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use wayland_protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_v1::WpAlphaModifierV1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1;
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::compositor_wayland::dmabuf::{BufferSlotUserData, WaylandAdvertised};
use crate::compositor_wayland::errors::BackendError;

/// Bound Wayland globals required by the backend.
pub struct WaylandGlobals {
    pub compositor: WlCompositor,
    pub subcompositor: WlSubcompositor,
    pub dmabuf: ZwpLinuxDmabufV1,
    pub viewporter: WpViewporter,
    pub alpha_modifier: Option<WpAlphaModifierV1>,
}

/// Lives in `WaylandSubsurfaceBackend`. Holds the connection + event
/// queue + bound globals + the modifier negotiation result.
pub struct WaylandState {
    pub connection: Connection,
    pub event_queue: EventQueue<DispatchState>,
    pub queue_handle: QueueHandle<DispatchState>,
    pub globals: WaylandGlobals,
    pub parent_surface: WlSurface,
    pub advertised: WaylandAdvertised,
    pub dispatch_state: DispatchState,
}

/// Dispatch user-data state held by the event queue. Keeps the
/// list of advertised `(format, modifier)` pairs current and provides
/// the buffer-release routing.
pub struct DispatchState {
    pub advertised: Arc<Mutex<WaylandAdvertised>>,
}

impl WaylandState {
    /// Build a state from raw `wl_display` + `wl_surface` pointers
    /// owned by the embedder. The connection borrows the display; the
    /// caller retains lifetime responsibility.
    ///
    /// # Safety
    ///
    /// `display` and `parent_surface` must point to live Wayland
    /// objects whose lifetime exceeds the returned state.
    pub unsafe fn new(
        display: *mut c_void,
        parent_surface_ptr: *mut c_void,
    ) -> Result<Self, BackendError> {
        if display.is_null() {
            return Err(BackendError::NullDisplay);
        }
        if parent_surface_ptr.is_null() {
            return Err(BackendError::NullSurface);
        }

        // Adopt the embedder's wl_display via wayland-backend's
        // libwayland-sys path. `from_foreign_display` is "guest" mode:
        // the backend will not close the underlying display on drop.
        let backend = Backend::from_foreign_display(display.cast());
        let connection = Connection::from_backend(backend);

        let advertised = Arc::new(Mutex::new(WaylandAdvertised::new()));
        let dispatch_state = DispatchState {
            advertised: advertised.clone(),
        };

        let (globals, event_queue) = registry_queue_init::<DispatchState>(&connection)
            .map_err(|e| BackendError::Wayland(format!("registry_queue_init: {e}")))?;
        let queue_handle = event_queue.handle();

        let compositor: WlCompositor = globals
            .bind(&queue_handle, 4..=6, ())
            .map_err(|e| BackendError::Wayland(format!("bind wl_compositor: {e}")))?;
        let subcompositor: WlSubcompositor = globals
            .bind(&queue_handle, 1..=1, ())
            .map_err(|e| BackendError::Wayland(format!("bind wl_subcompositor: {e}")))?;
        let dmabuf: ZwpLinuxDmabufV1 = globals
            .bind(&queue_handle, 3..=4, ())
            .map_err(|_| BackendError::MissingGlobal("zwp_linux_dmabuf_v1"))?;
        let viewporter: WpViewporter = globals
            .bind(&queue_handle, 1..=1, ())
            .map_err(|_| BackendError::MissingGlobal("wp_viewporter"))?;
        let alpha_modifier: Option<WpAlphaModifierV1> =
            globals.bind(&queue_handle, 1..=1, ()).ok();

        let bound_globals = WaylandGlobals {
            compositor,
            subcompositor,
            dmabuf,
            viewporter,
            alpha_modifier,
        };

        // Adopt the embedder's parent wl_surface. The raw-window-handle
        // pointer is a `*mut wl_proxy` (same opaque libwayland-client
        // type that backs every protocol object).
        let parent_surface = wayland_client_adopt_surface(&connection, parent_surface_ptr)
            .map_err(|e| BackendError::Wayland(format!("adopt parent wl_surface: {e}")))?;

        let mut state = Self {
            connection,
            event_queue,
            queue_handle,
            globals: bound_globals,
            parent_surface,
            advertised: WaylandAdvertised::new(),
            dispatch_state,
        };

        // Drive a roundtrip so the dmabuf format / modifier events arrive
        // before any caller asks for a chosen modifier.
        state
            .event_queue
            .roundtrip(&mut state.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("roundtrip(initial): {e}")))?;
        state.advertised = state.dispatch_state.advertised.lock().expect("advertised mutex poisoned").clone();

        Ok(state)
    }

    /// Drain any pending events (notably `wl_buffer.release`) without
    /// blocking. Called at the top of `present_master` / `present`.
    pub fn dispatch_pending(&mut self) -> Result<(), BackendError> {
        self.event_queue
            .dispatch_pending(&mut self.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("dispatch_pending: {e}")))?;
        Ok(())
    }

    /// Block until at least one event is dispatched. Called when the
    /// buffer pool is starved.
    pub fn roundtrip(&mut self) -> Result<(), BackendError> {
        self.event_queue
            .roundtrip(&mut self.dispatch_state)
            .map_err(|e| BackendError::Wayland(format!("roundtrip: {e}")))?;
        Ok(())
    }

    /// Flush queued protocol messages to the compositor.
    pub fn flush(&mut self) -> Result<(), BackendError> {
        self.connection
            .flush()
            .map_err(|e| BackendError::Wayland(format!("flush: {e}")))?;
        Ok(())
    }
}

/// Convert a raw `*mut wl_surface` (from raw-window-handle) into a
/// `wayland-client` `WlSurface` proxy. Uses `ObjectId::from_ptr` against
/// `WlSurface::interface()` followed by `Proxy::from_id`. The 2-arg
/// `from_ptr` signature is the wayland-backend 0.3.x shape (in 0.2.x the
/// interface table lookup was done via the backend instead).
///
/// # Safety
///
/// `raw` must be a valid `*mut wl_proxy` pointing to a live `wl_surface`
/// whose lifetime exceeds the returned proxy. The pointer is assumed to
/// have been produced by libwayland-client (typically via winit /
/// raw-window-handle).
unsafe fn wayland_client_adopt_surface(
    connection: &Connection,
    raw: *mut c_void,
) -> Result<WlSurface, String> {
    let id = ObjectId::from_ptr(WlSurface::interface(), raw.cast())
        .map_err(|e| format!("ObjectId::from_ptr: {e:?}"))?;
    let surface = WlSurface::from_id(connection, id)
        .map_err(|e| format!("WlSurface::from_id: {e:?}"))?;
    Ok(surface)
}

// ---- Dispatch impls --------------------------------------------------
// wayland-client requires Dispatch impls for every proxy whose events
// we want to handle. The default-no-op impls cover the globals we just
// bind-and-use; the meaningful ones are WlBuffer (release-event
// routing) and ZwpLinuxDmabufV1 (format/modifier advertisement).

macro_rules! noop_dispatch {
    ($proxy:ty) => {
        impl Dispatch<$proxy, ()> for DispatchState {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

noop_dispatch!(WlCompositor);
noop_dispatch!(WlSubcompositor);
noop_dispatch!(WpViewporter);
noop_dispatch!(WpAlphaModifierV1);
noop_dispatch!(WlSurface);
// ZwpLinuxBufferParamsV1 events (created/failed) are irrelevant when
// using create_immed; provide a no-op impl to satisfy the queue_handle
// trait bound on create_params.
noop_dispatch!(ZwpLinuxBufferParamsV1);

impl Dispatch<WlRegistry, GlobalListContents> for DispatchState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for DispatchState {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: <ZwpLinuxDmabufV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::Event;
        match event {
            Event::Format { format: _ } => {
                // v1 Format event is per-spec deprecated for modifier-
                // capable compositors; ignore and rely on Modifier.
            },
            Event::Modifier {
                format,
                modifier_hi,
                modifier_lo,
            } => {
                let modifier = ((modifier_hi as u64) << 32) | (modifier_lo as u64);
                state
                    .advertised
                    .lock()
                    .expect("advertised mutex poisoned")
                    .push((format, modifier));
            },
            _ => {},
        }
    }
}

impl Dispatch<WlBuffer, BufferSlotUserData> for DispatchState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        event: <WlBuffer as Proxy>::Event,
        user_data: &BufferSlotUserData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_buffer::Event;
        if matches!(event, Event::Release) {
            let mut g = user_data.in_flight.lock().expect("in_flight mutex poisoned");
            *g = false;
        }
    }
}
