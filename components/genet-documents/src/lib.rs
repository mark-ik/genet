/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Genet's retained document sessions: the static, scripted, and smolweb
//! content lanes, as inker **session engines** (2026-07-10 session-engines
//! plan).
//!
//! These types began as pelt's convenience lanes; the formalization promotes
//! them to an engine-grade component. Each lane is a retained layout session
//! producing [`netrender::Scene`] frames on demand, with scroll, activation,
//! and (scripted) a tick + quiescence seam. [`engines`] wraps each lane in
//! `inker::SessionEngine<Scene>` so hosts spawn them through the
//! `SessionRegistry` instead of hand-matching engine ids; pelt consumes this
//! component like any other host.

pub mod document;

// Dependency-free link resolution, shared by `document`, the scripted lane,
// and hosts' chrome (moved with the lanes from pelt).
pub mod href;

#[cfg(any(feature = "netfetch", feature = "smolweb"))]
pub(crate) mod net_fetch;

#[cfg(feature = "smolweb")]
pub mod smolweb;
#[cfg(feature = "smolweb")]
pub use smolweb::SmolwebDocument;
// Re-exported so a host that builds a `SmolwebDocument` can name its theme
// (and, for the App theme, supply a palette) without depending on
// `nematic::views` directly.
#[cfg(feature = "smolweb")]
pub use nematic::views::{SmolwebPalette, SmolwebTheme};

#[cfg(feature = "scripted")]
pub use genet_scripted::{
    ResourceFetcher as ScriptResourceFetcher, ScriptedDocument, ScriptedEngine,
};

pub mod engines;

pub use document::{ClickOutcome, LoadedDocument, LocalFetcher};
pub use engines::{StaticSessionEngine, session_click_from_outcome};
#[cfg(feature = "scripted")]
pub use engines::{ScriptedDocumentSession, ScriptedSessionEngine};
#[cfg(feature = "smolweb")]
pub use engines::{SmolwebDocumentSession, SmolwebSessionEngine};
pub use href::resolve_href;
pub use pelt_core::ResourceFetcher;

/// Structural display defaults the lanes layer over genet's UA cascade, so a
/// plain HTML document lays out as a stack of blocks rather than one inline
/// run, and document metadata stays unpainted. (V1; a fuller UA sheet is a
/// follow-up. genet-scripted carries its own copy for the same reason.)
pub(crate) const STRUCTURAL_SHEET: &[&str] = &[
    "html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd, \
     section, article, header, footer, nav, main, aside, figure, figcaption, \
     blockquote, pre, table, thead, tbody, tr, hr, form, fieldset { display: block; }",
    "head, style, script, title, meta, link, base { display: none; }",
    "body { padding: 8px; }",
];

// The scripted document's external-script fetch seam, implemented for the
// shell-level fetcher here (LocalFetcher's owner) so the impl satisfies the
// orphan rule after the move out of pelt.
#[cfg(feature = "scripted")]
impl genet_scripted::ResourceFetcher for document::LocalFetcher {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        pelt_core::ResourceFetcher::fetch(self, url)
    }
}
