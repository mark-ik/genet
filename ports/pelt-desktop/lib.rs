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

#[cfg(feature = "viewer")]
mod document;

#[cfg(feature = "viewer")]
mod headless;

#[cfg(feature = "scripted")]
mod scripted;

#[cfg(all(feature = "viewer", feature = "scripted"))]
mod scripted_viewer;

#[cfg(feature = "chrome")]
mod chrome;

#[cfg(all(feature = "viewer", feature = "chrome"))]
mod chrome_viewer;

#[cfg(feature = "tiles")]
mod tile_surface;

#[cfg(feature = "tiles")]
mod tile_shell;

#[cfg(feature = "tiles")]
mod tile_viewer;

/// Structural display defaults the viewer + scripted profiles layer over serval's UA
/// cascade, so a plain HTML document lays out as a stack of blocks rather than one
/// inline run, and document metadata stays unpainted. Shared by the static
/// ([`document`]) and scripted ([`scripted`]) loaders so the two cannot drift. (V1; a
/// fuller UA sheet is a follow-up.)
#[cfg(any(feature = "viewer", feature = "scripted"))]
pub(crate) const STRUCTURAL_SHEET: &[&str] = &[
    "html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd, \
     section, article, header, footer, nav, main, aside, figure, figcaption, \
     blockquote, pre, table, thead, tbody, tr, hr, form, fieldset { display: block; }",
    "head, style, script, title, meta, link, base { display: none; }",
    "body { padding: 8px; }",
];

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
#[cfg(feature = "viewer")]
pub use document::{LoadedDocument, LocalFetcher};
#[cfg(feature = "viewer")]
pub use headless::{
    render_snapshot, run_reftests, Outcome, ReftestResult, DEFAULT_HEIGHT, DEFAULT_WIDTH,
};
#[cfg(feature = "scripted")]
pub use scripted::{ScriptedDocument, ScriptedEngine};
#[cfg(all(feature = "viewer", feature = "scripted"))]
pub use scripted_viewer::run_scripted_viewer;
#[cfg(feature = "chrome")]
pub use chrome::{Chrome, ChromeIntent, ChromeState, StripSide};
#[cfg(all(feature = "viewer", feature = "chrome"))]
pub use chrome_viewer::run_chrome_viewer;
#[cfg(feature = "tiles")]
pub use tile_shell::TileShell;
#[cfg(feature = "tiles")]
pub use tile_surface::{DividerHit, TileFrame, TileLayer, TileSurface};
#[cfg(feature = "tiles")]
pub use tile_viewer::run_tile_viewer;
