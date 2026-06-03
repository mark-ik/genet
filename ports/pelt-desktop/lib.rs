/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Desktop host contracts for Pelt.
//!
//! This crate is the destination for winit windows, input translation, native
//! dialogs, filesystem integration, and platform event-loop glue. It stays
//! above `pelt-core` and below the UI chrome crate.

mod profile;
mod static_viewer;

#[cfg(feature = "macos-present")]
mod smoke_macos;
#[cfg(feature = "netrender")]
mod smoke_netrender;
#[cfg(feature = "netrender")]
mod smoke_webgl;
#[cfg(feature = "windows-present")]
mod smoke_windows;
#[cfg(feature = "linux-present")]
mod smoke_wayland;

pub use profile::{DesktopHostProfile, WindowingMode};
#[cfg(feature = "macos-present")]
pub use smoke_macos::{
    MacosCALayerPresentSmokeConfig, MacosCALayerPresentSmokeOutcome,
    run_macos_calayer_present_smoke,
};
#[cfg(feature = "netrender")]
pub use smoke_netrender::{NetrenderSmokeOutcome, run_netrender_smoke};
#[cfg(feature = "netrender")]
pub use smoke_webgl::{WebGlWgpuSmokeOutcome, run_webgl_wgpu_smoke};
#[cfg(feature = "windows-present")]
pub use smoke_windows::{
    WindowsDxgiPresentSmokeConfig, WindowsDxgiPresentSmokeOutcome, run_windows_dxgi_present_smoke,
};
#[cfg(feature = "linux-present")]
pub use smoke_wayland::{
    run_wayland_subsurface_present_smoke, WaylandPresentSmokeConfig, WaylandPresentSmokeOutcome,
};
pub use static_viewer::{StaticViewerConfig, StaticViewerOutcome, run_static_viewer};
