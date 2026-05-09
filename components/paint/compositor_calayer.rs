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
//! Cross-queue: netrender submits on wgpu's hidden `MTLCommandQueue`,
//! and this backend allocates its own `MTLCommandQueue` for the
//! per-frame blit. wgpu-hal 29 does **not** expose its Metal queue
//! (only `Queue::queue_from_raw` is public; see
//! `wgpu-hal-29.0.3/src/metal/mod.rs:459-481`), so a GPU-side
//! `encodeWaitForEvent` between the two queues is not available.
//! Today the consumer CPU-waits via [`wgpu::Device::poll`] in
//! [`MacosCALayerBackend::present_master`]. The `MTLSharedEvent`
//! field is reserved for a future GPU-side wait once `wgpu-hal`
//! grows a queue accessor or we adopt a wgpu-side blit path that
//! lives on netrender's queue.
//!
//! ## Status
//!
//! **Master path landed.** Construction (`new`) extracts the
//! `MTLDevice` from the wgpu-hal Metal device, attaches a
//! `CAMetalLayer` to the embedder root layer, and allocates the
//! per-backend `MTLCommandQueue` + `MTLSharedEvent`. The per-frame
//! body in `present_master` syncs `drawableSize` to the master,
//! CPU-waits the wgpu submit, and blits the master into
//! `nextDrawable().texture` via an `MTLBlitCommandEncoder`. The
//! per-`SurfaceKey` `declare`/`destroy`/`present` paths (for
//! declared compositor surfaces) are still stubs — they're not
//! exercised by the current single-master smoke and land with the
//! per-surface CALayer/IOSurface plumbing later.

#![allow(unsafe_code)]
#![allow(dead_code)]

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice,
    MTLPixelFormat, MTLSharedEvent, MTLTexture,
};
use objc2_quartz_core::{CAAutoresizingMask, CALayer, CAMetalDrawable, CAMetalLayer};
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
    /// Cloned wgpu device handle. Held so `present_master` can
    /// `poll(PollType::Wait)` to flush netrender's submit before
    /// our own `MTLCommandQueue` reads the master texture.
    /// `wgpu::Device` is `Arc`-shared internally so the clone is
    /// cheap.
    wgpu_device: wgpu::Device,
    metal_device: Retained<ProtocolObject<dyn MTLDevice>>,
    metal_queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    metal_layer: Retained<CAMetalLayer>,
    /// Embedder-supplied root layer; we hold a reference so the
    /// `metal_layer` sublayer attachment outlives the backend.
    parent_layer: Retained<CALayer>,
    /// `MTLSharedEvent` producer/consumer fence. Reserved for the
    /// future GPU-side wait (`encodeWaitForEvent:value:`) once
    /// `wgpu-hal::metal::Queue` exposes its `MTLCommandQueue` so the
    /// netrender producer can `encodeSignalEvent` on the shared
    /// queue. Today the cross-queue sync is CPU-side via
    /// `wgpu::Device::poll` in `present_master`.
    shared_event: Retained<ProtocolObject<dyn MTLSharedEvent>>,
    /// Monotonically-increasing event value the producer signals at
    /// after netrender's submit completes. Currently unused;
    /// reserved for the GPU-side wait path noted on `shared_event`.
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
    ///
    /// `root_layer` must be a raw pointer to a **`CALayer`**, not an
    /// `NSView` or `UIView`. Views are not CALayers — they have a
    /// backing CALayer accessible via the `layer` property.
    /// Embedders that hold an NSView/UIView should call its
    /// `[view layer]` (after `setWantsLayer:YES` for AppKit, which
    /// `[crate::compositor_factory::default_compositor_for_window]`
    /// does for them) and pass the result here.
    ///
    /// The pointer must outlive the backend; the caller is
    /// responsible for retaining the underlying CALayer on their
    /// side. The backend retains its own reference internally, so
    /// the caller's reference is independent of the backend's copy.
    ///
    /// # Safety
    ///
    /// `root_layer` must point to a valid `CALayer` (or subclass)
    /// instance. The returned backend retains the layer; the
    /// caller's reference is not consumed.
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

        // Pull the wgpu Metal device's underlying `MTLDevice` out
        // via wgpu-hal. Same pattern WNTI uses for the import side
        // (see `wgpu-graft/wgpu-native-texture-interop/src/sync_metal.rs:67-80`).
        // The hal-device borrow ends with the explicit drop; the
        // `Retained<MTLDevice>` survives independently.
        let metal_device: Retained<ProtocolObject<dyn MTLDevice>> = unsafe {
            let hal_device = host
                .device
                .as_hal::<wgpu::wgc::api::Metal>()
                .ok_or(BackendError::NoHalDevice)?;
            let device = hal_device.raw_device().clone();
            drop(hal_device);
            device
        };

        // Retain the embedder-supplied root layer (NSView.layer or
        // CALayer*). The caller is responsible for ensuring the
        // pointer stays valid for the lifetime of this backend.
        let parent_layer: Retained<CALayer> = unsafe {
            Retained::retain(root_layer.cast::<CALayer>()).ok_or(BackendError::NullLayer)?
        };

        // Configure CAMetalLayer for the wgpu Metal device.
        // `RGBA8Unorm` matches the master format the netrender smoke
        // selects (`Renderer::render_with_compositor` is called with
        // `wgpu::TextureFormat::Rgba8Unorm`); same-format both sides
        // lets `MTLBlitCommandEncoder copyFromTexture:toTexture:`
        // succeed without a render-graph format conversion.
        // `framebufferOnly: false` is required because we blit into
        // the drawable's texture rather than rendering through a
        // `MTLRenderPassDescriptor`.
        //
        // Frame + autoresizing: a freshly-allocated CALayer has
        // `frame == {0,0,0,0}`, which would make the sublayer
        // invisible regardless of `drawableSize` or how much we
        // present into it. Anchor it to the parent's current bounds
        // and set the standard width/height autoresizing mask so it
        // tracks the embedder view as it lays out / resizes.
        let metal_layer: Retained<CAMetalLayer> = {
            let layer = CAMetalLayer::new();
            layer.setDevice(Some(&*metal_device));
            layer.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
            layer.setFramebufferOnly(false);
            layer.setFrame(parent_layer.bounds());
            layer.setAutoresizingMask(
                CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable,
            );
            // Inherit contentsScale from the parent so the
            // CAMetalLayer presents at the screen's backing pixel
            // density. AppKit sets the parent layer's contentsScale
            // to the host display's `backingScaleFactor`
            // automatically; programmatically-added sublayers
            // default to 1.0 and would render at half-resolution on
            // Retina without this inheritance.
            layer.setContentsScale(parent_layer.contentsScale());
            layer
        };
        parent_layer.addSublayer(&metal_layer);

        let metal_queue: Retained<ProtocolObject<dyn MTLCommandQueue>> = metal_device
            .newCommandQueue()
            .ok_or(BackendError::QueueAlloc)?;

        let shared_event: Retained<ProtocolObject<dyn MTLSharedEvent>> = metal_device
            .newSharedEvent()
            .ok_or(BackendError::SharedEventAlloc)?;

        Ok(Self {
            wgpu_device: host.device.clone(),
            metal_device,
            metal_queue,
            metal_layer,
            parent_layer,
            shared_event,
            next_event_value: std::cell::Cell::new(0),
            surfaces: FxHashMap::default(),
        })
    }

    /// Present the netrender master texture into the CAMetalLayer.
    ///
    /// Per-frame flow:
    /// 1. Sync `metal_layer.drawableSize` to the master dims so the
    ///    OS doesn't resample.
    /// 2. CPU-wait for netrender's submit via `wgpu::Device::poll`
    ///    (wgpu-hal Metal does not expose its `MTLCommandQueue`, so
    ///    a GPU-side `encodeWaitForEvent` is not available; the
    ///    `shared_event` field is reserved for that future path).
    /// 3. Acquire `nextDrawable`.
    /// 4. Pull the master's `MTLTexture` via `wgpu::Texture::as_hal`.
    /// 5. Encode `copyFromTexture:toTexture:` on a fresh
    ///    `MTLBlitCommandEncoder`, present the drawable, commit.
    pub fn present_master(&mut self, master: &Texture) -> Result<(), BackendError> {
        // Reserve event value for the future GPU-side wait path.
        // CPU sync is used today; the advance preserves the value
        // protocol for consumers of `next_event_value`.
        let _producer_value = {
            let v = self.next_event_value.get();
            self.next_event_value.set(v + 1);
            v + 1
        };

        let size = master.size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }

        // Match `drawableSize` to the master so the OS compositor
        // hands back a drawable of the right dimensions and doesn't
        // resample. CGSize is f64 per the AppKit convention.
        let target_size = CGSize {
            width: size.width as f64,
            height: size.height as f64,
        };
        if self.metal_layer.drawableSize() != target_size {
            self.metal_layer.setDrawableSize(target_size);
        }

        // Block until netrender's submit is GPU-complete.
        // `wgpu-hal::metal::Queue` does not expose its underlying
        // `MTLCommandQueue` (only `queue_from_raw` is public — see
        // `wgpu-hal-29.0.3/src/metal/mod.rs:459-481`), so a GPU-side
        // `encodeWaitForEvent` between netrender's queue and ours is
        // not available. CPU-wait is the simplest correct sync; the
        // smoke runs at low cadence so the stall is invisible.
        self.wgpu_device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| BackendError::Poll(format!("{e:?}")))?;

        // Acquire the next drawable. Blocks if the layer's pool is
        // exhausted (`maximumDrawableCount` defaults to 3).
        let drawable: Retained<ProtocolObject<dyn CAMetalDrawable>> = self
            .metal_layer
            .nextDrawable()
            .ok_or(BackendError::NoDrawable)?;
        let drawable_texture: Retained<ProtocolObject<dyn MTLTexture>> = drawable.texture();

        // Pull the master's `MTLTexture` via wgpu-hal Metal.
        // `raw_handle()` returns a borrowed `&ProtocolObject` whose
        // lifetime is tied to `master_hal`; we use it inline for the
        // blit and explicitly drop the hal handle after.
        let master_hal = unsafe {
            master
                .as_hal::<wgpu::wgc::api::Metal>()
                .ok_or(BackendError::NoHalDevice)?
        };
        let master_texture: &ProtocolObject<dyn MTLTexture> = master_hal.raw_handle();

        // Allocate command buffer + blit encoder, copy master →
        // drawable, present, commit.
        let command_buffer = self
            .metal_queue
            .commandBuffer()
            .ok_or(BackendError::CommandBufferAlloc)?;
        let blit_encoder = command_buffer
            .blitCommandEncoder()
            .ok_or(BackendError::BlitEncoderAlloc)?;
        unsafe {
            blit_encoder.copyFromTexture_toTexture(master_texture, &*drawable_texture);
        }
        blit_encoder.endEncoding();

        // `presentDrawable` takes an `MTLDrawable` reference;
        // `CAMetalDrawable: MTLDrawable`, so upcast via
        // `ProtocolObject::from_ref`.
        let drawable_obj: &ProtocolObject<dyn objc2_metal::MTLDrawable> =
            ProtocolObject::from_ref(&*drawable);
        command_buffer.presentDrawable(drawable_obj);
        command_buffer.commit();

        drop(master_hal);
        Ok(())
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
            surface.layer.removeFromSuperlayer();
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
            Self::Unwired(area) => write!(f, "MacosCALayerBackend: not yet wired: {area}"),
        }
    }
}

impl std::error::Error for BackendError {}
