/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Host-shell core contracts for Pelt.
//!
//! Pelt is the shell layer that keeps browser chrome, platform integration,
//! protocol UI, and automation routing distinct from whichever engine profile
//! is hosted underneath it.

use std::fmt;
use std::str::FromStr;

pub mod tile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineProfile {
    /// Current all-up Servo browser engine: JS, DOM, layout, paint, webdriver.
    Browser,
    /// Future script-free static resource / document validation profile.
    Viewer,
    /// Alias for the script-free validation profile.
    Static,
    /// The scripted profile (V4): a live document whose inline `<script>` runs
    /// through `script-runtime-api` on a JS engine, mutating the DOM, rendered each
    /// frame. The content tier's proving ground (and the gc-arena soak's host).
    Scripted,
    /// Future automation-first profile. This is separate from `--headless`,
    /// which only selects the shell windowing mode.
    Headless,
}

impl EngineProfile {
    pub fn is_browser(self) -> bool {
        matches!(self, Self::Browser)
    }
}

impl Default for EngineProfile {
    fn default() -> Self {
        Self::Viewer
    }
}

impl fmt::Display for EngineProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Browser => "browser",
            Self::Viewer => "viewer",
            Self::Static => "static",
            Self::Scripted => "scripted",
            Self::Headless => "headless",
        };
        f.write_str(name)
    }
}

impl FromStr for EngineProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "browser" => Ok(Self::Browser),
            "viewer" => Ok(Self::Viewer),
            "static" => Ok(Self::Static),
            "scripted" => Ok(Self::Scripted),
            "headless" => Ok(Self::Headless),
            other => Err(format!(
                "unknown engine profile '{other}'; expected browser, viewer, static, scripted, or headless"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShellEngineCapabilities {
    pub javascript: bool,
    pub webdriver: bool,
    pub devtools: bool,
    pub webgpu: bool,
    pub webxr: bool,
}

pub trait ShellEngine {
    fn profile(&self) -> EngineProfile;
    fn capabilities(&self) -> ShellEngineCapabilities;
}

pub struct DeferredShellEngine {
    profile: EngineProfile,
}

impl DeferredShellEngine {
    pub fn new(profile: EngineProfile) -> Self {
        Self { profile }
    }

    pub fn unavailable_message(&self) -> String {
        format!(
            "pelt --engine {} is reserved for a future Serval validation path",
            self.profile
        )
    }
}

impl ShellEngine for DeferredShellEngine {
    fn profile(&self) -> EngineProfile {
        self.profile
    }

    fn capabilities(&self) -> ShellEngineCapabilities {
        ShellEngineCapabilities {
            // The scripted profile runs JS; the other profiles are script-free today.
            javascript: matches!(self.profile, EngineProfile::Scripted),
            ..ShellEngineCapabilities::default()
        }
    }
}

/// Host-shell resource-fetch contract: turn a URL into bytes for whichever engine
/// is hosted underneath. Networking is a *platform-integration* concern the shell
/// owns — kept off the engine, which only consumes bytes. Impls live in the ports
/// (a local-file fetcher, a netfetcher-backed fetcher, …); an engine's own byte
/// seams (e.g. serval's `ImageLoader`) delegate to whichever the shell supplies.
pub trait ResourceFetcher {
    /// Fetch `url` to bytes, or `None` on failure / unsupported scheme.
    fn fetch(&self, url: &str) -> Option<Vec<u8>>;
}
