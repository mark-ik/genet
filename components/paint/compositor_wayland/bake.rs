/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Rotation + opacity bake pipeline. See `compositor_wayland/mod.rs`
//! for the gating predicate.

pub struct BakePipeline {
    // Real impl lands in Task 7.1.
    _placeholder: (),
}

impl BakePipeline {
    pub fn new(_device: &wgpu::Device) -> Self {
        Self { _placeholder: () }
    }
}
