/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#![deny(unsafe_code)]

use std::sync::Arc;

use genet_static_dom::StaticDocument;
use layout_api::{LayoutHostServices, NoOpLayoutHostServices};
use servo_url::ServoUrl;

/// The first low-profile Genet build target.
///
/// This crate is intentionally thin while the layout input contract is being
/// extracted. Its main job today is to witness a script-free package graph.
#[derive(Clone, Debug)]
pub struct StaticHtmlProfile {
    /// The document URL or synthetic base URL used for resolving relative URLs.
    pub base_url: ServoUrl,
}

impl StaticHtmlProfile {
    /// Create a static HTML profile rooted at `base_url`.
    pub fn new(base_url: ServoUrl) -> Self {
        Self { base_url }
    }

    /// Return the default host services for the static profile.
    pub fn host_services(&self) -> Arc<dyn LayoutHostServices> {
        Arc::new(NoOpLayoutHostServices)
    }

    /// Parse an HTML string into the profile's script-free static DOM.
    pub fn parse_document(&self, html: &str) -> StaticDocument {
        StaticDocument::parse(html)
    }
}
