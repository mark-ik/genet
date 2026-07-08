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

#[cfg(feature = "tile-surface")]
mod document;

// Dependency-free link resolution, shared by `document`, `scripted`, and the chrome
// viewer. Lives outside `document` so the headless `scripted` profile can use it
// without that module's render/`data-url` stack (mirrors `STRUCTURAL_SHEET`'s gate).
#[cfg(any(feature = "tile-surface", feature = "scripted"))]
mod href;

#[cfg(any(feature = "netfetch", feature = "smolweb"))]
mod net_fetch;

#[cfg(feature = "smolweb")]
mod smolweb;
#[cfg(feature = "smolweb")]
pub use smolweb::SmolwebDocument;
// Re-exported so a host that builds a `SmolwebDocument` can name its theme (and, for
// the App theme, supply a palette) without depending on `smolweb-views` directly.
#[cfg(all(feature = "smolweb", feature = "viewer", feature = "chrome"))]
pub use chrome_viewer::run_smolweb_browser;
#[cfg(all(feature = "smolweb", feature = "viewer"))]
pub use smolweb::run_smolweb_viewer;
#[cfg(feature = "smolweb")]
pub use smolweb_views::{SmolwebPalette, SmolwebTheme};

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

#[cfg(feature = "tile-surface")]
mod tile_surface;

#[cfg(feature = "tile-surface")]
mod tile_shell;

#[cfg(feature = "tiles")]
mod tile_viewer;

/// Structural display defaults the viewer + scripted profiles layer over serval's UA
/// cascade, so a plain HTML document lays out as a stack of blocks rather than one
/// inline run, and document metadata stays unpainted. Shared by the static
/// ([`document`]) and scripted ([`scripted`]) loaders so the two cannot drift. (V1; a
/// fuller UA sheet is a follow-up.)
#[cfg(any(feature = "tile-surface", feature = "scripted"))]
pub(crate) const STRUCTURAL_SHEET: &[&str] = &[
    "html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd, \
     section, article, header, footer, nav, main, aside, figure, figcaption, \
     blockquote, pre, table, thead, tbody, tr, hr, form, fieldset { display: block; }",
    "head, style, script, title, meta, link, base { display: none; }",
    "body { padding: 8px; }",
];

#[cfg(feature = "png-reftest")]
mod smoke_chisel;
#[cfg(feature = "macos-present")]
mod smoke_macos;
#[cfg(feature = "netrender")]
mod smoke_netrender;
#[cfg(feature = "linux-present")]
mod smoke_wayland;
#[cfg(feature = "netrender")]
mod smoke_webgl;
#[cfg(feature = "windows-present")]
mod smoke_windows;

#[cfg(feature = "tile-surface")]
pub use document::{ClickOutcome, LoadedDocument, LocalFetcher};
#[cfg(feature = "viewer")]
pub use headless::{
    DEFAULT_HEIGHT, DEFAULT_WIDTH, Outcome, ReftestResult, render_snapshot, run_reftests,
};
#[cfg(feature = "png-reftest")]
pub use headless::{Fuzz, png_within_fuzz, render_png, render_png_scrolled};
#[cfg(any(feature = "tile-surface", feature = "scripted"))]
pub use href::resolve_href;
pub use profile::{DesktopHostProfile, WindowingMode};
#[cfg(feature = "png-reftest")]
pub use smoke_chisel::{ChiselSmokeOutcome, run_chisel_smoke};
#[cfg(feature = "macos-present")]
pub use smoke_macos::{
    MacosCALayerPresentSmokeConfig, MacosCALayerPresentSmokeOutcome,
    run_macos_calayer_present_smoke,
};
#[cfg(feature = "netrender")]
pub use smoke_netrender::{NetrenderSmokeOutcome, run_netrender_smoke};
#[cfg(feature = "linux-present")]
pub use smoke_wayland::{
    WaylandPresentSmokeConfig, WaylandPresentSmokeOutcome, run_wayland_subsurface_present_smoke,
};
#[cfg(feature = "netrender")]
pub use smoke_webgl::{WebGlWgpuSmokeOutcome, run_webgl_wgpu_smoke};
#[cfg(feature = "windows-present")]
pub use smoke_windows::{
    WindowsDxgiPresentSmokeConfig, WindowsDxgiPresentSmokeOutcome, run_windows_dxgi_present_smoke,
};
pub use static_viewer::{StaticViewerConfig, StaticViewerOutcome, run_static_viewer};
// `ScriptResourceFetcher` is `serval_scripted::ResourceFetcher` (the external-script
// byte seam `ScriptedDocument::from_body` takes), distinct from `pelt_core::
// ResourceFetcher` (the shell-level fetch contract); re-exported so a host can impl
// it without a direct `serval-scripted` dep.
#[cfg(feature = "scripted")]
pub use scripted::{ScriptResourceFetcher, ScriptedDocument, ScriptedEngine};
// The host installs a cookie store on a scripted document (e.g. meerkat's session jar)
// for `document.cookie`; re-export the seam so the host can name it without a direct
// `script-runtime-api` dep. (Render ladder 2c.)
#[cfg(feature = "scripted")]
pub use script_runtime_api::CookieProvider;
// The headless-scripted-DOM scrape (`ScriptedDocument::extract`) returns these; re-export
// so the host names the post-JS extract without a direct `serval-extract` dep. (Phase 4.)
#[cfg(feature = "chrome")]
pub use chrome::{Chrome, ChromeIntent, ChromeState, StripSide};
#[cfg(all(feature = "viewer", feature = "chrome"))]
pub use chrome_viewer::run_chrome_viewer;
#[cfg(all(feature = "viewer", feature = "scripted"))]
pub use scripted_viewer::run_scripted_viewer;
#[cfg(feature = "scripted")]
pub use serval_extract::{Heading, Link, Metadata, PageExtract};
#[cfg(feature = "tile-surface")]
pub use tile_shell::TileShell;
#[cfg(feature = "tile-surface")]
pub use tile_surface::{DividerHit, TileFrame, TileLayer, TileSurface};
#[cfg(feature = "tiles")]
pub use tile_viewer::run_tile_viewer;
