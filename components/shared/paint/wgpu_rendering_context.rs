/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A [`RenderingContext`] backed entirely by wgpu, with no GL dependency.
//!
//! The context owns the wgpu Instance, Adapter, Device, Queue, and Surface.
//! It provides frame targets for WebRender's `render_to_view()` zero-copy
//! path and handles surface presentation.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use dpi::PhysicalSize;
use image::RgbaImage;
use log::warn;
use paint_types::units::DeviceIntRect;

use crate::rendering_context_core::{RenderingContextCore, WgpuCapability};
use crate::wgpu_readback::read_texture_to_image;

/// A pure-wgpu rendering context that owns the GPU device and presentation surface.
///
/// The embedder creates this from a window handle. Servo/WebRender receives a
/// clone of the device/queue via [`RenderingContext::backend_binding`] and renders
/// into frame targets provided by [`RenderingContext::acquire_wgpu_frame_target`].
pub struct WgpuRenderingContext {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: RefCell<wgpu::SurfaceConfiguration>,
    size: Cell<PhysicalSize<u32>>,
    /// The current frame's surface texture, stored between acquire and present.
    current_frame: RefCell<Option<wgpu::SurfaceTexture>>,
    /// A GPU-side copy of the most recently rendered frame for readback.
    captured_frame: RefCell<Option<CapturedFrame>>,
}

struct CapturedFrame {
    texture: wgpu::Texture,
    size: PhysicalSize<u32>,
    format: wgpu::TextureFormat,
}

impl WgpuRenderingContext {
    /// Create a new wgpu rendering context for the given window.
    ///
    /// This creates a wgpu Instance, requests an Adapter and Device, and
    /// configures a Surface for presentation.
    pub fn new(
        window: Arc<
            impl raw_window_handle::HasWindowHandle
            + raw_window_handle::HasDisplayHandle
            + Send
            + Sync
            + 'static,
        >,
        size: PhysicalSize<u32>,
    ) -> Self {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .expect("Failed to create wgpu surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("No suitable GPU adapter found");

        // Request features that WebRender can take advantage of, intersected
        // with what the adapter actually supports.
        let wanted_features = wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
            | wgpu::Features::DUAL_SOURCE_BLENDING
            | wgpu::Features::TIMESTAMP_QUERY;
        let required_features = adapter.features() & wanted_features;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("serval_rendering_context"),
            required_features,
            required_limits: wgpu::Limits {
                // WebRender's composite shader uses up to @location(17).
                max_inter_stage_shader_variables: 28,
                ..Default::default()
            },
            ..Default::default()
        }))
        .expect("Failed to create wgpu device");

        // Configure surface — prefer non-sRGB to avoid double-encoding since
        // WebRender's output is already display-encoded (sRGB).
        let surface_caps = surface.get_capabilities(&adapter);
        let preferred_format = surface_caps.formats[0];
        let non_srgb_format = preferred_format.remove_srgb_suffix();
        let mut surface_usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if surface_caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            surface_usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let surface_config = wgpu::SurfaceConfiguration {
            usage: surface_usage,
            format: preferred_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: if non_srgb_format != preferred_format {
                vec![non_srgb_format]
            } else {
                vec![]
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            surface_config: RefCell::new(surface_config),
            size: Cell::new(size),
            current_frame: RefCell::new(None),
            captured_frame: RefCell::new(None),
        }
    }

    /// Create from pre-existing wgpu resources.
    ///
    /// Use this when the embedder already owns the wgpu device/queue/surface
    /// (e.g. from an egui or iced application).
    pub fn from_existing(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        surface_config: wgpu::SurfaceConfiguration,
        size: PhysicalSize<u32>,
    ) -> Self {
        Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            surface_config: RefCell::new(surface_config),
            size: Cell::new(size),
            current_frame: RefCell::new(None),
            captured_frame: RefCell::new(None),
        }
    }

    fn snapshot_current_frame(&self, frame: &wgpu::SurfaceTexture) {
        let config = self.surface_config.borrow();
        if !config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            return;
        }

        let size = PhysicalSize::new(config.width, config.height);
        let format = config.format;
        let mut captured_frame = self.captured_frame.borrow_mut();
        let needs_new_texture = captured_frame
            .as_ref()
            .is_none_or(|existing| existing.size != size || existing.format != format);
        if needs_new_texture {
            *captured_frame = Some(CapturedFrame {
                texture: self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("serval_readback_snapshot"),
                    size: wgpu::Extent3d {
                        width: size.width,
                        height: size.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                }),
                size,
                format,
            });
        }

        let Some(captured_frame) = captured_frame.as_ref() else {
            return;
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("serval_snapshot_encoder"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &captured_frame.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
    }
}

impl RenderingContextCore for WgpuRenderingContext {
    fn size(&self) -> PhysicalSize<u32> {
        self.size.get()
    }

    fn resize(&self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            warn!("WgpuRenderingContext: ignoring resize to {size:?} (dimensions must be >= 1)");
            return;
        }
        let mut config = self.surface_config.borrow_mut();
        config.width = size.width;
        config.height = size.height;
        self.surface.configure(&self.device, &config);
        self.size.set(size);
        self.captured_frame.borrow_mut().take();
    }

    fn present(&self) {
        if let Some(frame) = self.current_frame.borrow_mut().take() {
            self.snapshot_current_frame(&frame);
            frame.present();
        }
    }

    fn read_to_image(&self, rect: DeviceIntRect) -> Option<RgbaImage> {
        let captured_frame = self.captured_frame.borrow();
        let captured_frame = captured_frame.as_ref()?;
        read_texture_to_image(
            &self.device,
            &self.queue,
            &captured_frame.texture,
            captured_frame.format,
            captured_frame.size,
            rect,
        )
    }

    // `gl()` uses the default `None` — this context has no GL capability.

    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        Some(self)
    }
}

impl WgpuCapability for WgpuRenderingContext {
    fn instance(&self) -> wgpu::Instance {
        self.instance.clone()
    }

    fn adapter(&self) -> wgpu::Adapter {
        self.adapter.clone()
    }

    fn device(&self) -> wgpu::Device {
        self.device.clone()
    }

    fn queue(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    fn acquire_frame_target(&self) -> Option<wgpu::TextureView> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Reconfigure and retry once.
                let config = self.surface_config.borrow();
                self.surface.configure(&self.device, &config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(f)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
                    other => {
                        warn!(
                            "WgpuRenderingContext: failed to acquire frame after reconfigure: {other:?}"
                        );
                        return None;
                    },
                }
            },
            other => {
                warn!("WgpuRenderingContext: surface error: {other:?}");
                return None;
            },
        };

        // Create a non-sRGB view to avoid double-encoding WebRender's output.
        let config = self.surface_config.borrow();
        let non_srgb_format = config.format.remove_srgb_suffix();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(non_srgb_format),
            ..Default::default()
        });

        *self.current_frame.borrow_mut() = Some(frame);
        Some(view)
    }
}
