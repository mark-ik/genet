/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Errors raised by `WaylandSubsurfaceBackend`.

use crate::interop::InteropBackend;

/// Errors raised by [`crate::compositor_wayland::WaylandSubsurfaceBackend`]
/// construction or per-frame operations.
#[derive(Debug)]
pub enum BackendError {
    /// The supplied host wgpu context is not running on Vulkan.
    WrongBackend(InteropBackend),
    /// The provided wl_display pointer was null.
    NullDisplay,
    /// The provided wl_surface pointer was null.
    NullSurface,
    /// A Wayland registry global the backend requires was not advertised
    /// by the compositor.
    MissingGlobal(&'static str),
    /// No `(DRM format, modifier)` pair is supported by both Vulkan
    /// (RADV) and the Wayland compositor.
    NoCompatibleFormat,
    /// A Vulkan call failed during dmabuf import setup.
    Dmabuf(String),
    /// A Wayland protocol call failed.
    Wayland(String),
    /// The interop synchronizer could not be constructed.
    SyncInit(String),
    /// A path that hasn't been wired yet — see the named area.
    Unwired(&'static str),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongBackend(b) => {
                write!(f, "WaylandSubsurfaceBackend requires Vulkan, found {b:?}")
            },
            Self::NullDisplay => f.write_str("WaylandSubsurfaceBackend: null wl_display"),
            Self::NullSurface => f.write_str("WaylandSubsurfaceBackend: null wl_surface"),
            Self::MissingGlobal(g) => {
                write!(f, "WaylandSubsurfaceBackend: missing Wayland global: {g}")
            },
            Self::NoCompatibleFormat => f.write_str(
                "WaylandSubsurfaceBackend: no (DRM format, modifier) pair supported by both \
                 the Vulkan device and the Wayland compositor",
            ),
            Self::Dmabuf(m) => write!(f, "WaylandSubsurfaceBackend: dmabuf setup failed: {m}"),
            Self::Wayland(m) => write!(f, "WaylandSubsurfaceBackend: wayland call failed: {m}"),
            Self::SyncInit(m) => write!(f, "WaylandSubsurfaceBackend: sync init failed: {m}"),
            Self::Unwired(area) => {
                write!(f, "WaylandSubsurfaceBackend: not yet wired: {area}")
            },
        }
    }
}

impl std::error::Error for BackendError {}
