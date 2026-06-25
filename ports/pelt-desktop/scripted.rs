/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pelt adapter for the host-neutral scripted document owner.

pub use serval_scripted::{ScriptedDocument, ScriptedEngine};

#[cfg(feature = "tile-surface")]
impl serval_scripted::ResourceFetcher for crate::document::LocalFetcher {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        pelt_core::ResourceFetcher::fetch(self, url)
    }
}
