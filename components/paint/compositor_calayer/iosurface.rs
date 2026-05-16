/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFNumberType, CFRetained, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use objc2_io_surface::{
    IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceBytesPerRow, kIOSurfaceHeight,
    kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use crate::interop::HostWgpuContext;
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
pub(super) fn create_iosurface_rgba8(
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
    let cf_pf =
        cf_number_i32(IOSURFACE_FOURCC_RGBA).ok_or("CFNumberCreate(pixel_format) failed")?;

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
    let surface = unsafe { IOSurfaceRef::new(&dict) }.ok_or("IOSurfaceCreate returned nil")?;
    drop(dict);
    Ok(surface)
}

/// Wrap an existing IOSurface as a Metal texture (`MTLTexture`)
/// usable as a copy / render-pass destination.
///
/// Returns the new `MTLTexture` retained; caller is responsible for
/// keeping it alive while wgpu / CALayer reference the underlying
/// IOSurface.
pub(super) fn iosurface_to_mtl_texture(
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
pub(super) fn wgpu_texture_from_iosurface_mtl(
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
        host.device
            .create_texture_from_hal::<wgpu::wgc::api::Metal>(
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
