/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Egui chrome contracts for Pelt.
//!
//! This crate is the destination for tabs, location UI, browser dialogs,
//! webdriver/protocol controls, and development chrome.

use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChromeRenderBackend {
    /// Target path for native wgpu presentation and NetRender integration.
    #[default]
    Wgpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChromeRendererPlan {
    pub backend: ChromeRenderBackend,
}

impl ChromeRendererPlan {
    pub fn wgpu() -> Self {
        Self {
            backend: ChromeRenderBackend::Wgpu,
        }
    }

    pub fn backend_available(self) -> bool {
        match self.backend {
            ChromeRenderBackend::Wgpu => cfg!(feature = "wgpu-renderer"),
        }
    }
}

#[cfg(feature = "wgpu-renderer")]
pub mod wgpu {
    pub use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChromeCommand<ViewId> {
    Go(String),
    Back,
    Forward,
    Reload,
    ReloadAll,
    NewWebView,
    CloseWebView(ViewId),
    NewWindow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChromeState<ViewId> {
    pub location: String,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    view_id: PhantomData<ViewId>,
}

impl<ViewId> ChromeState<ViewId> {
    pub fn new(location: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            can_go_back: false,
            can_go_forward: false,
            view_id: PhantomData,
        }
    }
}
