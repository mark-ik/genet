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
//! The master->drawable copy runs on wgpu's queue (the same queue
//! netrender's vello submits to), so it gets natural FIFO ordering
//! against vello's render submit — no fence dance, no
//! `device.poll(Wait)` CPU stall, no own-`MTLCommandQueue`
//! allocation. `[drawable present]` (the no-arg
//! `MTLDrawable::present`) waits on the drawable's pending GPU
//! writes per Apple's `MTLDrawable` docs, sidestepping the wgpu-
//! hal `MTLCommandQueue` accessor that would otherwise be needed
//! to put `presentDrawable:` on the same Metal command buffer as
//! the blit.
//!
//! ## Status
//!
//! **Master path landed.** `new` extracts the `MTLDevice` from
//! the wgpu-hal Metal device, attaches a `CAMetalLayer`
//! (`BGRA8Unorm`, Apple-blessed) to the embedder root layer, and
//! allocates a reusable `wgpu::util::TextureBlitter` for the
//! per-frame `Rgba8Unorm` master -> `Bgra8Unorm` drawable format
//! conversion. `present_master` syncs `drawableSize`, imports the
//! drawable's `MTLTexture` into wgpu via `texture_from_raw` +
//! `create_texture_from_hal::<Metal>`, runs the `TextureBlitter`
//! copy on wgpu's queue, submits, and presents the drawable.
//!
//! **Per-`SurfaceKey` path landed.** `declare` allocates an
//! `IOSurface` (`'RGBA'` FourCC for blit-format-match against the
//! `Rgba8Unorm` master), wraps it as an `MTLTexture` via
//! `MTLDevice::newTextureWithDescriptor:iosurface:plane:`, hands
//! to wgpu via `create_texture_from_hal::<Metal>`, creates a
//! per-surface `CALayer` with `contents = IOSurface`, and adds it
//! as a sublayer. `present` applies `transform` / `clip` /
//! `opacity` to the per-surface CALayer; `destroy` unparents the
//! sublayer.

#![allow(unsafe_code)]
#![allow(dead_code)]

use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{
    kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionary, CFNumber,
    CFNumberType, CFRetained, CGPoint, CGRect, CGSize,
};
use objc2_io_surface::{
    kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow, kIOSurfaceHeight, kIOSurfacePixelFormat,
    kIOSurfaceWidth, IOSurfaceRef,
};
use objc2_metal::{
    MTLDevice, MTLDrawable, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor,
    MTLTextureUsage,
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
    /// Cloned wgpu device handle. Used for:
    ///   - allocating per-frame `CommandEncoder`s in `present_master`
    ///   - the per-`SurfaceKey` `declare` path's
    ///     `create_texture_from_hal` import of IOSurface-backed
    ///     MTLTextures.
    /// `wgpu::Device` is `Arc`-shared internally so the clone is
    /// cheap.
    wgpu_device: wgpu::Device,
    /// Cloned wgpu queue. The per-frame master->drawable copy
    /// submits on this queue, getting natural FIFO ordering against
    /// netrender's vello submit (which also runs on this queue).
    /// No `device.poll(Wait)` CPU stall, no MTLSharedEvent, no own
    /// MTLCommandQueue.
    wgpu_queue: wgpu::Queue,
    /// Reusable format-converting copy from `Rgba8Unorm` master to
    /// `Bgra8Unorm` drawable. Vello hardcodes `Rgba8Unorm` as its
    /// storage-binding format and explicitly recommends a
    /// `TextureBlitter` to format-convert at the
    /// surface/drawable boundary (vello/src/lib.rs:466-473);
    /// CAMetalLayer documents `BGRA8Unorm` as a supported
    /// `pixelFormat` value, `RGBA8Unorm` is not on the list.
    /// Allocated once at backend construction and reused per frame.
    master_to_drawable_blitter: wgpu::util::TextureBlitter,
    metal_device: Retained<ProtocolObject<dyn MTLDevice>>,
    metal_layer: Retained<CAMetalLayer>,
    /// Embedder-supplied root layer; we hold a reference so the
    /// `metal_layer` sublayer attachment outlives the backend.
    parent_layer: Retained<CALayer>,
    surfaces: FxHashMap<SurfaceKey, CALayerSurface>,
}

/// Per-`SurfaceKey` CALayer node. Holds everything keyed to a
/// declared compositor surface (iframes, video, will-change
/// islands):
///
/// - `layer`: a `CALayer` (sublayer of `parent_layer`) whose
///   `contents` is set to the IOSurface; the OS compositor
///   composites pixels directly from the shared memory.
/// - `iosurface`: the underlying shared memory the
///   destination `MTLTexture` is backed by. Held for refcount
///   ownership; CoreAnimation also retains it via
///   `layer.contents`.
/// - `_mtl_texture`: the IOSurface-backed `MTLTexture` we handed
///   to wgpu via `texture_from_raw`. wgpu's
///   `create_texture_from_hal` retains its own copy, but we keep a
///   reference here in case the wgpu side ever drops first.
/// - `_destination_format`: format we created the wgpu wrapper at;
///   stashed for future format-change detection if/when we grow
///   reallocation logic.
struct CALayerSurface {
    layer: Retained<CALayer>,
    iosurface: CFRetained<IOSurfaceRef>,
    _mtl_texture: Retained<ProtocolObject<dyn MTLTexture>>,
    _destination_format: wgpu::TextureFormat,
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
        // `BGRA8Unorm` is on Apple's documented
        // `CAMetalLayer.pixelFormat` allow-list (alongside
        // `BGRA8Unorm_sRGB` / `RGBA16Float` / etc.). Vello's
        // master is `Rgba8Unorm` (its compute pipeline's
        // storage-binding format is hardcoded — see
        // `vello/src/lib.rs:474`), so we use a
        // `wgpu::util::TextureBlitter` per-frame in
        // `present_master` to format-convert master → drawable.
        // Vello's own docs explicitly recommend `TextureBlitter`
        // for this conversion at the surface boundary.
        //
        // `framebufferOnly: false` is required because the
        // TextureBlitter renders into the drawable's texture (it
        // uses a render pass), not just present-only access.
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
            layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
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

        // Allocate the master->drawable format-converting copy
        // once. Reused per frame in `present_master`.
        let master_to_drawable_blitter =
            wgpu::util::TextureBlitter::new(&host.device, wgpu::TextureFormat::Bgra8Unorm);

        Ok(Self {
            wgpu_device: host.device.clone(),
            wgpu_queue: host.queue.clone(),
            master_to_drawable_blitter,
            metal_device,
            metal_layer,
            parent_layer,
            surfaces: FxHashMap::default(),
        })
    }

    /// Present the netrender master texture into the CAMetalLayer.
    ///
    /// Per-frame flow:
    /// 1. Sync `metal_layer.drawableSize` to the master dims so the
    ///    OS doesn't resample.
    /// 2. Acquire `nextDrawable` and pull its `MTLTexture`.
    /// 3. Import the drawable's `MTLTexture` into wgpu via
    ///    `wgpu::hal::metal::Device::texture_from_raw` +
    ///    `Device::create_texture_from_hal::<Metal>`.
    /// 4. Encode the master->drawable copy with
    ///    `wgpu::util::TextureBlitter::copy` — handles the
    ///    `Rgba8Unorm` master to `Bgra8Unorm` drawable format
    ///    conversion via its built-in fragment-shader-based blit.
    /// 5. Submit on wgpu's queue. Submit is FIFO-ordered against
    ///    netrender's vello submit (same queue), so no fence /
    ///    `device.poll(Wait)` stall is needed.
    /// 6. Call `[drawable present]` (the no-arg `MTLDrawable::present`).
    ///    Apple's docs guarantee the OS waits for the drawable's
    ///    pending GPU writes (which the wgpu submit registered as
    ///    pending) before displaying — sidesteps the wgpu-hal
    ///    `MTLCommandQueue`-accessor block that would otherwise
    ///    require us to put `presentDrawable:` on the same command
    ///    buffer as the blit.
    pub fn present_master(&mut self, master: &Texture) -> Result<(), BackendError> {
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

        // Acquire the next drawable. Blocks if the layer's pool is
        // exhausted (`maximumDrawableCount` defaults to 3).
        let drawable: Retained<ProtocolObject<dyn CAMetalDrawable>> = self
            .metal_layer
            .nextDrawable()
            .ok_or(BackendError::NoDrawable)?;
        let drawable_mtl_texture: Retained<ProtocolObject<dyn MTLTexture>> = drawable.texture();

        // Import the drawable's `MTLTexture` into wgpu so the
        // `TextureBlitter` (which speaks wgpu) can render into it.
        // `raw_handle` -> `texture_from_raw` -> `create_texture_from_hal`
        // is the same import path the per-`SurfaceKey` IOSurface
        // helper uses; here the underlying texture is the
        // CAMetalLayer's per-frame drawable instead of an
        // IOSurface-backed allocation.
        //
        // SAFETY: drawable_mtl_texture is a fresh, valid MTLTexture
        // returned by `[CAMetalDrawable texture]`. It's retained by
        // `drawable` for the duration of this function. The wgpu
        // wrapper takes its own retain via `texture_from_raw`'s
        // `Retained` argument (we clone the retained handle for it).
        let drawable_wgpu = unsafe {
            let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
                drawable_mtl_texture.clone(),
                wgpu::TextureFormat::Bgra8Unorm,
                objc2_metal::MTLTextureType::Type2D,
                1,
                1,
                wgpu::hal::CopyExtent {
                    width: size.width,
                    height: size.height,
                    depth: 1,
                },
            );
            self.wgpu_device.create_texture_from_hal::<wgpu::wgc::api::Metal>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("MacosCALayerBackend drawable view"),
                    size: wgpu::Extent3d {
                        width: size.width,
                        height: size.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
            )
        };
        let master_view = master.create_view(&wgpu::TextureViewDescriptor::default());
        let drawable_view = drawable_wgpu.create_view(&wgpu::TextureViewDescriptor::default());

        // Encode the format-converting copy and submit on wgpu's
        // queue (same queue netrender's vello submits to → FIFO
        // ordering, no fence needed).
        let mut encoder =
            self.wgpu_device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("MacosCALayerBackend::present_master master->drawable"),
                });
        self.master_to_drawable_blitter.copy(
            &self.wgpu_device,
            &mut encoder,
            &master_view,
            &drawable_view,
        );
        self.wgpu_queue.submit([encoder.finish()]);

        // `[drawable present]` (no-arg `MTLDrawable::present`)
        // schedules the drawable for display at the next refresh
        // tick. Per Apple's `MTLDrawable` docs, the OS waits for
        // any pending GPU writes to the drawable's texture to
        // complete before display — covering our just-submitted
        // wgpu blit — so no explicit fence wiring is needed.
        let drawable_obj: &ProtocolObject<dyn objc2_metal::MTLDrawable> =
            ProtocolObject::from_ref(&*drawable);
        drawable_obj.present();

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

    fn declare(
        &mut self,
        key: SurfaceKey,
        host: &HostWgpuContext,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::Texture, crate::compositor::BoxedBackendError> {
        // Currently only `Rgba8Unorm` is supported — vello 0.8's
        // compute pipeline hardcodes that as its storage-texture
        // binding format, so the master is `Rgba8Unorm` and the
        // per-surface IOSurface-backed destination must match for
        // `copy_texture_to_texture` to succeed (same-format src/dst
        // requirement). See the pixel-format note in
        // `MacosCALayerBackend::new` for the BGRA-vs-RGBA story.
        // Wide-gamut / HDR support follows the master format
        // story; lift this when those land.
        if format != wgpu::TextureFormat::Rgba8Unorm {
            return Err(Box::new(BackendError::UnsupportedFormat(format!(
                "{format:?} (only Rgba8Unorm is supported today)"
            ))));
        }

        // 1. Allocate IOSurface (shared memory the OS compositor +
        //    Metal both read).
        let iosurface = create_iosurface_rgba8(width, height)
            .map_err(|e| Box::new(BackendError::IOSurface(format!("{e}"))))?;

        // 2. Wrap as a Metal texture so wgpu can render into it.
        let mtl_texture = iosurface_to_mtl_texture(&self.metal_device, &iosurface, width, height)
            .map_err(|e| Box::new(BackendError::MtlTextureFromIOSurface(format!("{e}"))))?;

        // 3. Hand the MTLTexture to wgpu via wgpu-hal's
        //    `texture_from_raw`. The returned `wgpu::Texture` is a
        //    handle into the same MTLTexture; wgpu's `copy_*` and
        //    render-pass APIs work against it normally.
        let dest = wgpu_texture_from_iosurface_mtl(host, mtl_texture.clone(), width, height, format);

        // 4. Create a per-surface CALayer; set `contents` to the
        //    IOSurface so the OS compositor reads pixels directly
        //    from shared memory (no draw step).
        let layer = unsafe {
            let l = CALayer::new();
            l.setContentsScale(self.parent_layer.contentsScale());
            // CALayer.contents accepts `Option<&AnyObject>`; an
            // IOSurface is an `AnyObject` via its `__IOSurfaceRef`
            // type-encoding. Cast through the raw pointer.
            let iosurface_obj: *mut objc2::runtime::AnyObject =
                CFRetained::as_ptr(&iosurface).as_ptr() as *mut _;
            l.setContents(Some(&*iosurface_obj));
            l
        };

        // Frame the per-surface CALayer at its declared bounds. The
        // wrapper computes bounds-relative position; here we set the
        // raw frame against the parent. `present` overrides this on
        // each frame from the `transform` arg, so this is just the
        // initial position.
        layer.setFrame(CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: width as f64 / self.parent_layer.contentsScale(),
                height: height as f64 / self.parent_layer.contentsScale(),
            },
        });

        self.parent_layer.addSublayer(&layer);

        self.surfaces.insert(
            key,
            CALayerSurface {
                layer,
                iosurface,
                _mtl_texture: mtl_texture,
                _destination_format: format,
            },
        );

        Ok(dest)
    }

    fn destroy(&mut self, key: SurfaceKey) {
        if let Some(surface) = self.surfaces.remove(&key) {
            surface.layer.removeFromSuperlayer();
        }
    }

    fn present(
        &mut self,
        key: SurfaceKey,
        transform: [f32; 6],
        clip: Option<[f32; 4]>,
        opacity: f32,
    ) {
        let Some(surface) = self.surfaces.get(&key) else {
            log::warn!(
                "[MacosCALayerBackend] present({key:?}) called before declare; skipping"
            );
            return;
        };

        // World coordinates are pixels; CALayer's coordinate space
        // is points. `setAffineTransform`'s linear part (a/b/c/d)
        // is unitless rotation/scale and passes through unchanged,
        // but the translation (tx, ty) must be converted to points
        // via `contentsScale`. netrender composes the surface's
        // `bounds.origin` into `world_transform.tx/ty` already
        // (see `netrender::vello_tile_rasterizer::build_layer_presents`),
        // so this single conversion places the per-surface CALayer
        // at its declared world-position without a separate origin
        // application step.
        let scale = self.parent_layer.contentsScale();
        surface.layer.setAffineTransform(objc2_core_foundation::CGAffineTransform {
            a: transform[0] as f64,
            b: transform[1] as f64,
            c: transform[2] as f64,
            d: transform[3] as f64,
            tx: transform[4] as f64 / scale,
            ty: transform[5] as f64 / scale,
        });

        // Clip: `Some([min_x, min_y, max_x, max_y])` becomes the
        // layer's `bounds` + `masksToBounds`. `None` clears the mask
        // so the full layer composites.
        match clip {
            Some([x0, y0, x1, y1]) => {
                surface.layer.setMasksToBounds(true);
                surface.layer.setBounds(CGRect {
                    origin: CGPoint {
                        x: x0 as f64 / scale,
                        y: y0 as f64 / scale,
                    },
                    size: CGSize {
                        width: (x1 - x0) as f64 / scale,
                        height: (y1 - y0) as f64 / scale,
                    },
                });
            },
            None => {
                surface.layer.setMasksToBounds(false);
            },
        }

        surface.layer.setOpacity(opacity);
    }
}

// =============================================================================
// IOSurface plumbing for per-`SurfaceKey` declared compositor surfaces
// =============================================================================

/// FourCC `'RGBA'` packed big-endian as a 32-bit integer. Used as
/// the `kIOSurfacePixelFormat` value for the IOSurface storage we
/// allocate. Matches vello 0.8's hardcoded `Rgba8Unorm` storage-
/// binding format (so the master can be blitted into the IOSurface
/// without a format-converting pass). See the pixel-format note in
/// `MacosCALayerBackend::new` for the BGRA-vs-RGBA story.
const IOSURFACE_FOURCC_RGBA: i32 =
    ((b'R' as i32) << 24) | ((b'G' as i32) << 16) | ((b'B' as i32) << 8) | (b'A' as i32);

/// Build a CFNumber wrapping a 32-bit signed integer. Helper for
/// the IOSurface-properties dictionary.
fn cf_number_i32(value: i32) -> Option<CFRetained<CFNumber>> {
    unsafe {
        CFNumber::new(
            None,
            CFNumberType::SInt32Type,
            &value as *const _ as *const c_void,
        )
    }
}

/// Allocate an RGBA8-formatted IOSurface of `width x height` pixels.
///
/// The IOSurface is shared memory readable by both the OS
/// compositor (via `CALayer.contents`) and Metal (via
/// `MTLDevice::newTextureWithDescriptor:iosurface:plane:`).
///
/// Pixel format is `'RGBA'` (FourCC `0x52474241`) with 4 bytes per
/// pixel and a row stride of `width * 4`. `Rgba8Unorm` is the
/// master format vello 0.8 produces (storage-binding format
/// hardcoded), so this matches the master without a format-
/// converting blit. See the pixel-format note in
/// `MacosCALayerBackend::new` for the BGRA-vs-RGBA story.
fn create_iosurface_rgba8(
    width: u32,
    height: u32,
) -> Result<CFRetained<IOSurfaceRef>, &'static str> {
    let bytes_per_element: i32 = 4;
    let bytes_per_row: i32 = (width as i32)
        .checked_mul(bytes_per_element)
        .ok_or("IOSurface bytes_per_row overflow")?;

    let cf_width = cf_number_i32(width as i32).ok_or("CFNumberCreate(width) failed")?;
    let cf_height = cf_number_i32(height as i32).ok_or("CFNumberCreate(height) failed")?;
    let cf_bpr = cf_number_i32(bytes_per_row).ok_or("CFNumberCreate(bytes_per_row) failed")?;
    let cf_bpe =
        cf_number_i32(bytes_per_element).ok_or("CFNumberCreate(bytes_per_element) failed")?;
    let cf_pf = cf_number_i32(IOSURFACE_FOURCC_RGBA)
        .ok_or("CFNumberCreate(pixel_format) failed")?;

    // Build a 5-entry CFDictionary with the IOSurface property keys.
    // Using `CFDictionary::new` (the raw CFDictionaryCreate
    // wrapper); pairs of `*const c_void` — cast keys / values
    // through `as_ptr`.
    //
    // SAFETY: the `kIOSurface*` extern statics are CFString
    // singletons exported by IOSurface.framework; reading them is
    // sound but requires an `unsafe` block per Rust's extern-static
    // rule.
    let keys: [*const c_void; 5] = unsafe {
        [
            (&**kIOSurfaceWidth) as *const _ as *const c_void,
            (&**kIOSurfaceHeight) as *const _ as *const c_void,
            (&**kIOSurfaceBytesPerRow) as *const _ as *const c_void,
            (&**kIOSurfaceBytesPerElement) as *const _ as *const c_void,
            (&**kIOSurfacePixelFormat) as *const _ as *const c_void,
        ]
    };
    let values: [*const c_void; 5] = [
        CFRetained::as_ptr(&cf_width).as_ptr() as *const c_void,
        CFRetained::as_ptr(&cf_height).as_ptr() as *const c_void,
        CFRetained::as_ptr(&cf_bpr).as_ptr() as *const c_void,
        CFRetained::as_ptr(&cf_bpe).as_ptr() as *const c_void,
        CFRetained::as_ptr(&cf_pf).as_ptr() as *const c_void,
    ];
    let dict = unsafe {
        CFDictionary::new(
            None,
            keys.as_ptr() as *mut _,
            values.as_ptr() as *mut _,
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    .ok_or("CFDictionaryCreate failed")?;

    // Hand the properties dict to IOSurfaceRef::new (the
    // non-deprecated wrapper around IOSurfaceCreate). The dict is
    // borrowed for the call only.
    let surface = unsafe { IOSurfaceRef::new(&dict) }
        .ok_or("IOSurfaceCreate returned nil")?;
    drop(dict);
    Ok(surface)
}

/// Wrap an existing IOSurface as a Metal texture (`MTLTexture`)
/// usable as a copy / render-pass destination.
///
/// Returns the new `MTLTexture` retained; caller is responsible for
/// keeping it alive while wgpu / CALayer reference the underlying
/// IOSurface.
fn iosurface_to_mtl_texture(
    metal_device: &ProtocolObject<dyn MTLDevice>,
    iosurface: &IOSurfaceRef,
    width: u32,
    height: u32,
) -> Result<Retained<ProtocolObject<dyn MTLTexture>>, &'static str> {
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::RGBA8Unorm,
            width as usize,
            height as usize,
            false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    // `Shared` for IOSurface backing — the surface is allocated in
    // shared memory and visible to the OS compositor; `Private`
    // would refuse the IOSurface attachment.
    descriptor.setStorageMode(MTLStorageMode::Shared);

    metal_device
        .newTextureWithDescriptor_iosurface_plane(&descriptor, iosurface, 0)
        .ok_or("newTextureWithDescriptor:iosurface:plane: returned nil")
}

/// Hand an IOSurface-backed `MTLTexture` to wgpu via wgpu-hal's
/// `texture_from_raw` -> `create_texture_from_hal` pipeline. The
/// returned `wgpu::Texture` is a regular handle into the same
/// underlying storage; `copy_texture_to_texture` and render-pass
/// APIs work against it normally.
fn wgpu_texture_from_iosurface_mtl(
    host: &HostWgpuContext,
    mtl_texture: Retained<ProtocolObject<dyn MTLTexture>>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    unsafe {
        let hal_texture = wgpu::hal::metal::Device::texture_from_raw(
            mtl_texture,
            format,
            objc2_metal::MTLTextureType::Type2D,
            1,
            1,
            wgpu::hal::CopyExtent {
                width,
                height,
                depth: 1,
            },
        );
        host.device.create_texture_from_hal::<wgpu::wgc::api::Metal>(
            hal_texture,
            &wgpu::TextureDescriptor {
                label: Some("MacosCALayerBackend IOSurface destination"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        )
    }
}

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
                write!(f, "MacosCALayerBackend: unsupported destination format: {fmt}")
            },
            Self::IOSurface(reason) => {
                write!(f, "MacosCALayerBackend: IOSurface creation failed: {reason}")
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
