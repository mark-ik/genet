/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use pelt_core::EngineProfile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowingMode {
    Headed,
    Headless,
}

impl WindowingMode {
    pub fn from_headless_flag(headless: bool) -> Self {
        match headless {
            true => Self::Headless,
            false => Self::Headed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesktopHostProfile {
    pub engine: EngineProfile,
    pub windowing: WindowingMode,
}

impl DesktopHostProfile {
    pub fn new(engine: EngineProfile, windowing: WindowingMode) -> Self {
        Self { engine, windowing }
    }
}
