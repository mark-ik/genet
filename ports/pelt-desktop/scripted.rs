/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pelt adapter for the host-neutral scripted document owner.
//!
//! (The `serval_scripted::ResourceFetcher` impl for `LocalFetcher` moved to
//! `serval-documents` with the lanes — the orphan rule wants it beside the
//! fetcher it is implemented for.)

pub use serval_scripted::{
    ResourceFetcher as ScriptResourceFetcher, ScriptedDocument, ScriptedEngine,
};
