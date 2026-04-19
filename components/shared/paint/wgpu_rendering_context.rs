/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A [`RenderingContext`] backed entirely by wgpu, with no GL dependency.
//!
//! The context owns the wgpu Instance, Adapter, Device, Queue, and Surface.
//! It provides frame targets for WebRender's `render_to_view()` zero-copy
//! path and handles surface presentation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use dpi::PhysicalSize;
use euclid::Size2D;
use image::RgbaImage;
use log::warn;
use webrender_api::units::{DeviceIntRect, DevicePixel};

use crate::rendering_context_core::{RenderingContextCore, WgpuCapability};

/// A pure-wgpu rendering context that owns the GPU device and presentation surface.
///
/// The embedder creates this from a window handle. Servo/WebRender receives a
/// clone of the device/queue via [`RenderingContext::backend_binding`] and renders
/// into frame targets provided by [`RenderingContext::acquire_wgpu_frame_target`].
pub struct WgpuRenderingContext {
    #[allow(dead_code)]
    instance: wgpu::Instance,
    #[allow(dead_code)]
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: RefCell<wgpu::SurfaceConfiguration>,
    size: Cell<PhysicalSize<u32>>,
    /// The current frame's surface texture, stored between acquire and present.
    current_frame: RefCell<Option<wgpu::SurfaceTexture>>,
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
            label: Some("servo_wgpu_rendering_context"),
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
        let preferred_format = surface.get_capabilities(&adapter).formats[0];
        let non_srgb_format = preferred_format.remove_srgb_suffix();
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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
        }
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
    }

    fn present(&self) {
        if let Some(frame) = self.current_frame.borrow_mut().take() {
            frame.present();
        }
    }

    fn read_to_image(&self, _rect: DeviceIntRect) -> Option<RgbaImage> {
        // TODO: Implement GPU→CPU readback via staging buffer for screenshots.
        None
    }

    // `gl()` uses the default `None` — this context has no GL capability.

    fn wgpu(&self) -> Option<&dyn WgpuCapability> {
        Some(self)
    }
}

impl WgpuCapability for WgpuRenderingContext {
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
