/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Per-platform default-compositor factory.
//!
//! Embedders typically want "give me the right OS-handoff backend
//! for this platform, given my window handle." This module provides
//! that one-stop construction:
//!
//! ```ignore
//! use paint::{HostWgpuContext, default_compositor_for_window};
//!
//! let host = HostWgpuContext::new(device, queue);
//! let compositor = default_compositor_for_window(
//!     host,
//!     display_handle,
//!     window_handle,
//! )?;
//! paint.install_compositor(compositor);
//! ```
//!
//! The factory dispatches on `cfg`:
//!
//! - `target_os = "windows"` → [`crate::WindowsDxgiBackend`] wrapped
//!   in [`crate::ServoCompositor`].
//! - `target_vendor = "apple"` → [`crate::MacosCALayerBackend`].
//! - `target_os = "linux"` → [`crate::WaylandSubsurfaceBackend`].
//! - otherwise → [`crate::WgpuMasterCaptureBackend`] (no OS handoff
//!   available; embedder reads composite back via
//!   `Paint::composite_texture`).
//!
//! Each per-platform backend's construction can fail (wrong backend
//! detected on `host`, null window handle, OS API failure). The
//! factory propagates those errors as `BoxedFactoryError`. Embedders
//! that want "platform if available, else fall back to capture"
//! should call [`default_compositor_for_window_or_capture`] instead
//! — that variant logs the error and returns
//! `WgpuMasterCaptureBackend` as the fallback.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use crate::compositor::{PaintCompositor, WgpuMasterCaptureBackend};
use crate::interop::HostWgpuContext;

/// Type-erased factory error. Per-platform backends return their own
/// `BackendError` types; this wraps them so the cfg-dispatched
/// factory has one return shape.
pub type BoxedFactoryError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Construct the platform-default OS-handoff compositor.
///
/// Returns `Err` if the per-platform backend's construction fails
/// (wrong wgpu backend, null handle, OS API error, etc.). On
/// platforms with no OS-handoff backend, returns
/// [`WgpuMasterCaptureBackend`] (never errors — that's a fallback
/// in its own right).
///
/// `_display` is required by Wayland (needs `wl_display`) and
/// ignored on Windows / macOS — pass it from the embedder
/// regardless to keep call sites portable.
pub fn default_compositor_for_window(
    host: HostWgpuContext,
    _display: RawDisplayHandle,
    window: RawWindowHandle,
) -> Result<Box<dyn PaintCompositor>, BoxedFactoryError> {
    #[cfg(target_os = "windows")]
    {
        let RawWindowHandle::Win32(win32) = window else {
            return Err(format!(
                "default_compositor_for_window (windows): expected Win32 \
                 RawWindowHandle, got {window:?}"
            )
            .into());
        };
        let hwnd = windows::Win32::Foundation::HWND(win32.hwnd.get() as *mut _);
        let backend = crate::WindowsDxgiBackend::new(&host, hwnd)
            .map_err(|e| Box::new(e) as BoxedFactoryError)?;
        let compositor = crate::ServoCompositor::new(host, backend);
        return Ok(Box::new(compositor));
    }

    #[cfg(target_vendor = "apple")]
    {
        let layer_ptr: *mut std::ffi::c_void = match window {
            RawWindowHandle::AppKit(view) => view.ns_view.as_ptr() as *mut _,
            RawWindowHandle::UiKit(view) => view.ui_view.as_ptr() as *mut _,
            other => {
                return Err(format!(
                    "default_compositor_for_window (apple): expected AppKit / \
                     UiKit RawWindowHandle, got {other:?}"
                )
                .into());
            },
        };
        // SAFETY: layer_ptr is non-null (raw-window-handle's NonNull
        // pointers are non-null by construction); embedder owns the
        // underlying NSView/UIView lifetime.
        let backend = unsafe { crate::MacosCALayerBackend::new(&host, layer_ptr) }
            .map_err(|e| Box::new(e) as BoxedFactoryError)?;
        let compositor = crate::ServoCompositor::new(host, backend);
        return Ok(Box::new(compositor));
    }

    #[cfg(target_os = "linux")]
    {
        let RawWindowHandle::Wayland(wl_window) = window else {
            return Err(format!(
                "default_compositor_for_window (linux): expected Wayland \
                 RawWindowHandle, got {window:?}"
            )
            .into());
        };
        let RawDisplayHandle::Wayland(wl_display) = _display else {
            return Err(format!(
                "default_compositor_for_window (linux): expected Wayland \
                 RawDisplayHandle, got {_display:?}"
            )
            .into());
        };
        // SAFETY: pointers are non-null; embedder owns the wl_display +
        // wl_surface lifetimes.
        let backend = unsafe {
            crate::WaylandSubsurfaceBackend::new(
                &host,
                wl_display.display.as_ptr() as *mut _,
                wl_window.surface.as_ptr() as *mut _,
            )
        }
        .map_err(|e| Box::new(e) as BoxedFactoryError)?;
        let compositor = crate::ServoCompositor::new(host, backend);
        return Ok(Box::new(compositor));
    }

    #[cfg(not(any(target_os = "windows", target_vendor = "apple", target_os = "linux")))]
    {
        let _ = (host, window);
        Ok(Box::new(WgpuMasterCaptureBackend::new()))
    }
}

/// Same as [`default_compositor_for_window`] but logs construction
/// errors and falls back to [`WgpuMasterCaptureBackend`] instead of
/// surfacing them. Useful for embedders that just want pixels —
/// "platform handoff if it works, capture path otherwise."
pub fn default_compositor_for_window_or_capture(
    host: HostWgpuContext,
    display: RawDisplayHandle,
    window: RawWindowHandle,
) -> Box<dyn PaintCompositor> {
    // We need to keep `host` available for the fallback construction.
    // Clone it before handing it to the strict factory; HostWgpuContext
    // is `Clone` (wgpu Device/Queue/Adapter/Instance are all Arc-shared
    // so the clone is cheap).
    let host_for_fallback = host.clone();
    match default_compositor_for_window(host, display, window) {
        Ok(boxed) => boxed,
        Err(err) => {
            log::warn!(
                "[paint] default_compositor_for_window failed; falling back to \
                 WgpuMasterCaptureBackend: {err}"
            );
            let _ = host_for_fallback; // keep available for symmetry / future
            Box::new(WgpuMasterCaptureBackend::new())
        },
    }
}
