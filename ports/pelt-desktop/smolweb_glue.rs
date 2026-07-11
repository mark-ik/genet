/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Windowed-shell glue for the smolweb lane: the [`SmolwebDocument`]
//! (a `serval-documents` type since the session-engines split) as pelt's
//! `ViewerContent` / `BrowsableContent`, plus the standalone viewer entry.
//! Pelt impls its own local traits for the foreign type.

use netrender::Scene;
use serval_documents::{LocalFetcher, SmolwebDocument, SmolwebTheme};
use serval_layout::ScrollKey;

/// The smolweb document as windowed [`ViewerContent`](crate::static_viewer::windowed::ViewerContent),
/// so it plugs into the shared winit shell like the static document. v1 is read-only:
/// no scroll yet, and in-window link navigation is the chrome/tile lanes' job (the
/// bare viewer has no history), so a click is a no-op here.
#[cfg(feature = "viewer")]
impl crate::static_viewer::windowed::ViewerContent for SmolwebDocument {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        SmolwebDocument::frame(self, width, height)
    }
    fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_by(self, dx, dy)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_at(self, x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        SmolwebDocument::scroll_for_key(self, key)
    }
    fn click_at(&mut self, _x: f32, _y: f32) -> bool {
        // The bare viewer has no history; navigation is the chrome browser's job
        // (see the `BrowsableContent` impl below), so a click is a no-op here.
        false
    }
}

/// The smolweb document as [`BrowsableContent`](crate::chrome_viewer::windowed::BrowsableContent),
/// so it hosts in the shared chrome browser (omnibar + back/forward + navigation), the
/// same shell the HTML viewer uses. A link click resolves to its `on_navigate` URL,
/// which the shell loads.
#[cfg(all(feature = "viewer", feature = "chrome"))]
impl crate::chrome_viewer::windowed::BrowsableContent for SmolwebDocument {
    fn load(url: &str) -> Result<Self, String> {
        SmolwebDocument::load(&LocalFetcher, url, SmolwebTheme::default())
    }
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        SmolwebDocument::frame(self, width, height)
    }
    fn scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        SmolwebDocument::scroll_at(self, x, y, dx, dy)
    }
    fn scroll_for_key(&mut self, key: ScrollKey) -> bool {
        SmolwebDocument::scroll_for_key(self, key)
    }
    fn click_at(
        &mut self,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
    ) -> crate::chrome_viewer::windowed::ContentClick {
        use crate::chrome_viewer::windowed::ContentClick;
        match SmolwebDocument::click_at(self, x, y, width, height) {
            Some(url) => ContentClick::Navigate(url),
            None => ContentClick::None,
        }
    }
}

/// Open a window and present the smolweb capsule at `config.url`, themed per-site by
/// default (the Lagrange look). The smolweb twin of
/// [`run_static_viewer`](crate::run_static_viewer); a bad URL fails fast before the
/// window opens.
#[cfg(feature = "viewer")]
pub fn run_smolweb_viewer(
    config: crate::StaticViewerConfig,
) -> Result<crate::StaticViewerOutcome, String> {
    let doc = SmolwebDocument::load(&LocalFetcher, &config.url, SmolwebTheme::default())?;
    crate::static_viewer::run_headed_with(config, doc)
}
