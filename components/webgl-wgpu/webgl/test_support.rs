/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

pub(super) fn make_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("wgpu adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("webgl-wgpu test device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    }))
    .expect("wgpu device")
}

pub(super) fn make_context(width: u32, height: u32) -> WebGlContext {
    let (device, queue) = make_device();
    WebGlContext::from_wgpu_handles(device, queue, WebGlCanvasDescriptor::new(width, height))
        .expect("context")
}

pub(super) fn make_context_with_depth(width: u32, height: u32) -> WebGlContext {
    let (device, queue) = make_device();
    WebGlContext::from_wgpu_handles(
        device,
        queue,
        WebGlCanvasDescriptor::new(width, height).with_depth(true),
    )
    .expect("context with depth")
}
