/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
use crate::interop::InteropBackend;
/// Errors raised by [`MacosCALayerBackend::new`] /
/// [`MacosCALayerBackend::present_master`] / `declare`.
#[derive(Debug)]
pub enum BackendError {
    /// The supplied host wgpu context is not running on Metal.
    WrongBackend(InteropBackend),
    /// Failed to obtain the wgpu-hal Metal device.
    NoHalDevice,
    /// The provided root-layer pointer was null.
    NullLayer,
    /// Failed to allocate an MTLCommandQueue.
    QueueAlloc,
    /// Failed to allocate an MTLSharedEvent.
    SharedEventAlloc,
    /// `wgpu::Device::poll` returned an error during the per-frame
    /// CPU-side wait for netrender's submit.
    Poll(String),
    /// `CAMetalLayer::nextDrawable` returned `nil` — the layer's
    /// drawable pool is exhausted or the layer is misconfigured.
    NoDrawable,
    /// `MTLCommandQueue::commandBuffer` returned `nil`.
    CommandBufferAlloc,
    /// `MTLCommandBuffer::blitCommandEncoder` returned `nil`.
    BlitEncoderAlloc,
    /// `declare` was called with an unsupported `wgpu::TextureFormat`.
    UnsupportedFormat(String),
    /// IOSurface creation failed (CFDictionary construction or
    /// `IOSurfaceCreate` itself).
    IOSurface(String),
    /// `MTLDevice::newTextureWithDescriptor:iosurface:plane:` failed.
    MtlTextureFromIOSurface(String),
    /// A path that hasn't been wired yet — see the named area.
    Unwired(&'static str),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongBackend(b) => {
                write!(f, "MacosCALayerBackend requires Metal, found {b:?}")
            },
            Self::NoHalDevice => {
                f.write_str("MacosCALayerBackend: wgpu-hal Metal device unavailable")
            },
            Self::NullLayer => f.write_str("MacosCALayerBackend: null root-layer pointer"),
            Self::QueueAlloc => f.write_str("MacosCALayerBackend: newCommandQueue returned nil"),
            Self::SharedEventAlloc => {
                f.write_str("MacosCALayerBackend: newSharedEvent returned nil")
            },
            Self::Poll(err) => write!(f, "MacosCALayerBackend: wgpu device.poll: {err}"),
            Self::NoDrawable => f.write_str("MacosCALayerBackend: nextDrawable returned nil"),
            Self::CommandBufferAlloc => {
                f.write_str("MacosCALayerBackend: commandBuffer returned nil")
            },
            Self::BlitEncoderAlloc => {
                f.write_str("MacosCALayerBackend: blitCommandEncoder returned nil")
            },
            Self::UnsupportedFormat(fmt) => {
                write!(
                    f,
                    "MacosCALayerBackend: unsupported destination format: {fmt}"
                )
            },
            Self::IOSurface(reason) => {
                write!(
                    f,
                    "MacosCALayerBackend: IOSurface creation failed: {reason}"
                )
            },
            Self::MtlTextureFromIOSurface(reason) => write!(
                f,
                "MacosCALayerBackend: MTLDevice::newTextureWithDescriptor:iosurface:plane: failed: {reason}",
            ),
            Self::Unwired(area) => write!(f, "MacosCALayerBackend: not yet wired: {area}"),
        }
    }
}

impl std::error::Error for BackendError {}
