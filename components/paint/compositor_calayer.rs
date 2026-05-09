/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! macOS / iOS `OsCompositorBackend` impl using CALayer + Metal.
//!
//! Bridges netrender's wgpu Metal master texture into a `CAMetalLayer`
//! attached to an `NSView`. The embedder hands an `NSView` (or
//! `CALayer` / `UIView` on iOS) at construction; the backend creates a
//! sublayer (`CAMetalLayer`) on top of it, pulls drawables per frame,
//! and blits the master into them via Metal.
//!
//! ## Why CAMetalLayer (not bare CALayer + IOSurface)
//!
//! There are two viable paths on macOS:
//!
//! 1. **`CAMetalLayer`** — managed swap chain; Metal hands us a
//!    drawable per frame whose `texture` we render/blit into. The OS
//!    compositor handles the visual tree integration. This is the
//!    standard path for new macOS Metal apps and what wgpu uses for
//!    its native macOS surfaces.
//! 2. **`CALayer.contents` + IOSurface** — manual; we own an
//!    `IOSurface`, render into it via Metal (`MTLTexture` backed by
//!    the surface), then assign the IOSurface to a CALayer's
//!    `contents`. More flexible (multiple surfaces, custom z-order)
//!    but the IOSurface pool, validation, and CATransaction
//!    bookkeeping are all manual.
//!
//! C4 uses (1) for the master path — same shape as the
//! `WindowsDxgiBackend`'s composition swapchain. (2) is reserved for
//! per-`SurfaceKey` declared compositor surfaces (iframes, video,
//! `will-change` islands), which are deferred.
//!
//! ## Construction
//!
//! Caller passes a `*mut c_void` pointing to an `NSView` (typical
//! winit) or `CALayer` (root layer of an embedded surface). The
//! backend creates a `CAMetalLayer`, configures it for the wgpu Metal
//! device (`MTLPixelFormat::BGRA8Unorm`, premultiplied alpha,
//! framebuffer-only=false because we blit into it), and attaches it
//! as a sublayer.
//!
//! ## Synchronization
//!
//! The producer/consumer fence is an `MTLSharedEvent` (Apple's
//! cross-API fence). The producer (netrender's wgpu Metal queue)
//! signals; the consumer (this backend's `MTLBlitCommandEncoder`
//! waits before encoding the copy. Same-queue path (which the cut
//! milestone uses) doesn't need explicit waits — work is FIFO on the
//! Metal queue.
//!
//! ## Status
//!
//! **Skeleton.** Construction signatures are real; the per-frame
//! `present_master` body is a documented stub. macOS validation
//! requires hardware; this code has not been compiled on a Mac.
//! Treat as a shape lock-in pending a focused macOS session.

#![allow(unsafe_code)]
#![allow(dead_code)]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_metal::{MTLCommandQueue, MTLDevice, MTLSharedEvent};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use rustc_hash::FxHashMap;
use wgpu::Texture;

use crate::compositor::OsCompositorBackend;
use crate::interop::{HostWgpuContext, InteropBackend, SyncMechanism};
use netrender_device::compositor::SurfaceKey;

/// macOS / iOS CALayer-backed compositor backend.
///
/// Construction allocates:
/// - The wgpu `MTLDevice` + `MTLCommandQueue` (cached from the
///   `HostWgpuContext` so the backend's `MTLBlitCommandEncoder` runs
///   on the same Metal queue as netrender's submit — natural FIFO
///   ordering without explicit fences).
/// - A `CAMetalLayer` configured for the device, attached as a
///   sublayer of the embedder-supplied root `CALayer`.
/// - An `MTLSharedEvent` for the producer/consumer fence dance
///   (currently unused for the same-queue path; reserved for the
///   multi-queue future).
pub struct MacosCALayerBackend {
    metal_device: Retained<ProtocolObject<dyn MTLDevice>>,
    metal_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    metal_layer: Retained<CAMetalLayer>,
    /// Embedder-supplied root layer; we hold a reference so the
    /// `metal_layer` sublayer attachment outlives the backend.
    parent_layer: Retained<CALayer>,
    /// `MTLSharedEvent` producer/consumer fence. Reserved for the
    /// multi-queue path; same-queue submits are FIFO-ordered without
    /// explicit waits.
    shared_event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    /// Monotonically-increasing event value the producer signals at
    /// after netrender's submit completes.
    next_event_value: std::cell::Cell<u64>,
    surfaces: FxHashMap<SurfaceKey, CALayerSurface>,
}

/// Per-`SurfaceKey` CALayer node. Holds the layer for declared
/// compositor surfaces (iframes, video, will-change islands).
struct CALayerSurface {
    layer: Retained<CALayer>,
    destination: Texture,
}

unsafe impl Send for MacosCALayerBackend {}

impl MacosCALayerBackend {
    /// Construct a backend over the embedder-supplied root layer.
    /// `root_layer` is a raw pointer to a `CALayer` (or `NSView` —
    /// pass its `view.layer` if the embedder hands an NSView). The
    /// pointer must outlive the backend; the caller is responsible
    /// for retaining it on their side.
    ///
    /// # Safety
    ///
    /// `root_layer` must point to a valid `CALayer` instance. The
    /// returned backend retains the layer; the caller's reference is
    /// not consumed.
    pub unsafe fn new(
        host: &HostWgpuContext,
        root_layer: *mut std::ffi::c_void,
    ) -> Result<Self, BackendError> {
        if host.backend != InteropBackend::Metal {
            return Err(BackendError::WrongBackend(host.backend));
        }
        if root_layer.is_null() {
            return Err(BackendError::NullLayer);
        }

        // Reach into wgpu-hal for the MTLDevice + MTLCommandQueue.
        // FIXME(C4 follow-up): the as_hal calls here need verification
        // against wgpu 29's actual Metal hal surface — the method names
        // / return types may differ slightly from the Dx12 path.
        let metal_device: Retained<ProtocolObject<dyn MTLDevice>> = unsafe {
            let _hal_device = host
                .device
                .as_hal::<wgpu::wgc::api::Metal>()
                .ok_or(BackendError::NoHalDevice)?;
            // FIXME: extract MTLDevice from hal device. The exact
            // accessor depends on wgpu-hal's metal module shape.
            // Placeholder — code below intentionally panics at
            // runtime on macOS until the extraction is wired:
            return Err(BackendError::Unwired(
                "MTLDevice extraction from wgpu-hal Metal device",
            ));
        };

        // Suppress unused-binding warnings on the placeholders that
        // follow. None of this runs because the early-return above
        // shorts the construction.
        #[allow(unreachable_code)]
        {
            let parent_layer: Retained<CALayer> = unsafe {
                Retained::retain(root_layer.cast::<CALayer>())
                    .ok_or(BackendError::NullLayer)?
            };

            let metal_layer: Retained<CAMetalLayer> = unsafe {
                let layer = CAMetalLayer::new();
                layer.setDevice(Some(&*metal_device));
                layer.setPixelFormat(objc2_metal::MTLPixelFormat::BGRA8Unorm);
                layer.setFramebufferOnly(false);
                layer
            };

            unsafe { parent_layer.addSublayer(&metal_layer) };

            let metal_queue: Retained<ProtocolObject<dyn MTLCommandQueue>> = unsafe {
                metal_device
                    .newCommandQueue()
                    .ok_or(BackendError::QueueAlloc)?
            };

            let shared_event: Retained<ProtocolObject<dyn MTLSharedEvent>> = unsafe {
                metal_device
                    .newSharedEvent()
                    .ok_or(BackendError::SharedEventAlloc)?
            };

            Ok(Self {
                metal_device,
                metal_queue,
                metal_layer,
                parent_layer,
                shared_event,
                next_event_value: std::cell::Cell::new(0),
                surfaces: FxHashMap::default(),
            })
        }
    }

    /// Present the netrender master texture into the CAMetalLayer.
    ///
    /// FIXME(C4 follow-up): the per-frame body still needs:
    /// 1. `metal_layer.nextDrawable()` → drawable + its texture.
    /// 2. Resize `metal_layer.drawableSize` if it doesn't match the
    ///    master.
    /// 3. Pull the master `MTLTexture` from
    ///    `master.as_hal::<Metal>().raw_handle()` (or equivalent).
    /// 4. `MTLCommandBuffer` from `metal_queue`, encode
    ///    `MTLBlitCommandEncoder` `copyFromTexture:toTexture:`,
    ///    `presentDrawable:`, `commit`.
    /// 5. `next_event_value.set(value + 1)`; signal `shared_event` if
    ///    in the multi-queue path.
    ///
    /// Today: no-op stub. The skeleton is here so the
    /// `OsCompositorBackend` impl below resolves.
    pub fn present_master(&mut self, _master: &Texture) -> Result<(), BackendError> {
        let v = self.next_event_value.get();
        self.next_event_value.set(v + 1);
        let _ = (
            &self.metal_device,
            &self.metal_queue,
            &self.metal_layer,
            &self.parent_layer,
            &self.shared_event,
        );
        Err(BackendError::Unwired("present_master per-frame body"))
    }
}

impl OsCompositorBackend for MacosCALayerBackend {
    fn interop_backend(&self) -> InteropBackend {
        InteropBackend::Metal
    }

    fn sync_mechanism(&self) -> SyncMechanism {
        // Same-queue submits are FIFO-ordered on Metal; the
        // shared-event path is reserved for multi-queue.
        SyncMechanism::None
    }

    fn present_master(&mut self, master: &Texture) {
        if let Err(err) = MacosCALayerBackend::present_master(self, master) {
            log::warn!("[MacosCALayerBackend] present_master: {err}");
        }
    }

    fn declare(&mut self, _key: SurfaceKey, _host: &HostWgpuContext, _native: &Texture) {
        // FIXME(C4 follow-up): allocate IOSurface-backed MTLTexture
        // for the surface, create a CALayer, set its `contents` to
        // the IOSurface, addSublayer to root.
    }

    fn destroy(&mut self, key: SurfaceKey) {
        if let Some(surface) = self.surfaces.remove(&key) {
            unsafe { surface.layer.removeFromSuperlayer() };
        }
    }

    fn present(
        &mut self,
        _key: SurfaceKey,
        _transform: [f32; 6],
        _clip: Option<[f32; 4]>,
        _opacity: f32,
    ) {
        // FIXME(C4 follow-up): apply transform/clip/opacity to the
        // per-surface CALayer, kick a CATransaction.
    }
}

/// Errors raised by [`MacosCALayerBackend::new`] /
/// [`MacosCALayerBackend::present_master`].
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
    /// A path that hasn't been wired yet — see the named area.
    Unwired(&'static str),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongBackend(b) => {
                write!(f, "MacosCALayerBackend requires Metal, found {b:?}")
            },
            Self::NoHalDevice => f.write_str("MacosCALayerBackend: wgpu-hal Metal device unavailable"),
            Self::NullLayer => f.write_str("MacosCALayerBackend: null root-layer pointer"),
            Self::QueueAlloc => f.write_str("MacosCALayerBackend: newCommandQueue returned nil"),
            Self::SharedEventAlloc => {
                f.write_str("MacosCALayerBackend: newSharedEvent returned nil")
            },
            Self::Unwired(area) => write!(f, "MacosCALayerBackend: not yet wired: {area}"),
        }
    }
}

impl std::error::Error for BackendError {}
