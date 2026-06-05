/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WebGL-over-wgpu adapter shell.
//!
//! This crate owns the first Servo-side texture contract for WebGL
//! canvases rendered on the same `wgpu::Device` as the compositor.
//! It also hosts the first W3 WebGL-shaped state slice and shader
//! translation seam, while keeping broad WebGL conformance work out
//! of this initial adapter.

#![deny(unsafe_code)]

mod shader;
mod webgl;

use paint_api::rendering_context_core::{RenderingContextCore, WgpuCapability};

pub use shader::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};
pub use webgl::{
    BufferTarget, BufferUsage, CubeFace, DepthFunc, IndexType, PrimitiveMode, ShaderStage,
    WebGlBufferId, WebGlContext, WebGlContextAttributes, WebGlError, WebGlFramebufferId,
    WebGlFramebufferStatus, WebGlProgramId, WebGlRenderbufferId, WebGlShaderId, WebGlTextureId,
    WebGlUniformLocation,
};

const CANVAS_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC)
    .union(wgpu::TextureUsages::COPY_DST);

/// How the canvas alpha channel should be interpreted by the consumer.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CanvasAlphaMode {
    /// The canvas should be treated as opaque at composition time.
    Opaque,
    /// Color channels are straight alpha.
    Straight,
    /// Color channels are premultiplied by alpha.
    Premultiplied,
}

/// Construction options for a WebGL canvas texture.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct WebGlCanvasDescriptor {
    pub size: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub alpha_mode: CanvasAlphaMode,
    /// Whether to allocate a depth-stencil attachment alongside
    /// the color texture. Mirrors `WebGlContextAttributes.depth`
    /// — if `false`, `WebGlContext::set_depth_test_enabled(true)`
    /// records `InvalidOperation`.
    pub depth: bool,
}

impl WebGlCanvasDescriptor {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            format: wgpu::TextureFormat::Rgba8Unorm,
            alpha_mode: CanvasAlphaMode::Premultiplied,
            depth: false,
        }
    }

    pub fn with_format(mut self, format: wgpu::TextureFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_alpha_mode(mut self, alpha_mode: CanvasAlphaMode) -> Self {
        self.alpha_mode = alpha_mode;
        self
    }

    pub fn with_depth(mut self, depth: bool) -> Self {
        self.depth = depth;
        self
    }
}

/// Same-device canvas output consumable by netrender or Pelt.
pub struct WebGlCanvasTexture {
    pub texture: wgpu::Texture,
    pub size: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub alpha_mode: CanvasAlphaMode,
    pub generation: u64,
    pub damage: Option<[u32; 4]>,
}

impl WebGlCanvasTexture {
    pub fn create_view(&self) -> wgpu::TextureView {
        self.texture
            .create_view(&wgpu::TextureViewDescriptor::default())
    }
}

/// Minimal owner for the WebGL default framebuffer texture.
pub struct WebGlCanvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    output: WebGlCanvasTexture,
    /// Depth-stencil attachment that mirrors `output` in size,
    /// allocated only when the canvas descriptor sets `depth =
    /// true`. Reallocated on `resize`. `WebGlContext` reads
    /// `depth_view()` when depth test is enabled.
    depth: Option<CanvasDepthAttachment>,
}

struct CanvasDepthAttachment {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

pub(crate) const CANVAS_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

impl WebGlCanvas {
    pub fn from_rendering_context(
        context: &dyn RenderingContextCore,
        descriptor: WebGlCanvasDescriptor,
    ) -> Result<Self, WebGlCanvasError> {
        let capability = context
            .wgpu()
            .ok_or(WebGlCanvasError::MissingWgpuCapability)?;
        Self::from_wgpu_capability(capability, descriptor)
    }

    pub fn from_wgpu_capability(
        capability: &dyn WgpuCapability,
        descriptor: WebGlCanvasDescriptor,
    ) -> Result<Self, WebGlCanvasError> {
        Self::from_wgpu_handles(capability.device(), capability.queue(), descriptor)
    }

    pub fn from_wgpu_handles(
        device: wgpu::Device,
        queue: wgpu::Queue,
        descriptor: WebGlCanvasDescriptor,
    ) -> Result<Self, WebGlCanvasError> {
        let output = create_canvas_texture(&device, descriptor, 0)?;
        let depth = if descriptor.depth {
            Some(create_canvas_depth(&device, descriptor.size))
        } else {
            None
        };
        Ok(Self {
            device,
            queue,
            output,
            depth,
        })
    }

    pub fn texture(&self) -> &WebGlCanvasTexture {
        &self.output
    }

    /// `true` when this canvas was built with `depth = true` and
    /// therefore owns a depth-stencil attachment the context can
    /// render against.
    pub fn has_depth(&self) -> bool {
        self.depth.is_some()
    }

    pub(crate) fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth.as_ref().map(|d| &d.view)
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), WebGlCanvasError> {
        let descriptor = WebGlCanvasDescriptor {
            size: (width, height),
            format: self.output.format,
            alpha_mode: self.output.alpha_mode,
            depth: self.depth.is_some(),
        };
        self.output = create_canvas_texture(&self.device, descriptor, self.output.generation + 1)?;
        if self.depth.is_some() {
            self.depth = Some(create_canvas_depth(&self.device, descriptor.size));
        }
        Ok(())
    }

    pub fn clear(&mut self, color: wgpu::Color) {
        let view = self.output.create_view();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("webgl-wgpu canvas clear encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgl-wgpu canvas clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit([encoder.finish()]);
        self.output.damage = Some([0, 0, self.output.size.0, self.output.size.1]);
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WebGlCanvasError {
    MissingWgpuCapability,
    EmptySize,
}

fn create_canvas_texture(
    device: &wgpu::Device,
    descriptor: WebGlCanvasDescriptor,
    generation: u64,
) -> Result<WebGlCanvasTexture, WebGlCanvasError> {
    let (width, height) = descriptor.size;
    if width == 0 || height == 0 {
        return Err(WebGlCanvasError::EmptySize);
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("webgl-wgpu canvas texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: descriptor.format,
        usage: CANVAS_USAGE,
        view_formats: &[],
    });

    Ok(WebGlCanvasTexture {
        texture,
        size: descriptor.size,
        format: descriptor.format,
        alpha_mode: descriptor.alpha_mode,
        generation,
        damage: Some([0, 0, width, height]),
    })
}

fn create_canvas_depth(device: &wgpu::Device, size: (u32, u32)) -> CanvasDepthAttachment {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("webgl-wgpu canvas depth attachment"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: CANVAS_DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    CanvasDepthAttachment { texture, view }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    struct TestWgpuCapability {
        device: wgpu::Device,
        queue: wgpu::Queue,
    }

    impl WgpuCapability for TestWgpuCapability {
        fn instance(&self) -> wgpu::Instance {
            panic!("not needed by WebGlCanvas W1 smoke")
        }

        fn adapter(&self) -> wgpu::Adapter {
            panic!("not needed by WebGlCanvas W1 smoke")
        }

        fn device(&self) -> wgpu::Device {
            self.device.clone()
        }

        fn queue(&self) -> wgpu::Queue {
            self.queue.clone()
        }

        fn acquire_frame_target(&self) -> Option<wgpu::TextureView> {
            None
        }
    }

    fn make_device() -> (wgpu::Device, wgpu::Queue) {
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

    fn read_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let row_bytes = width * 4;
        let padded_row_bytes = row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer_size = padded_row_bytes as u64 * height as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgl-wgpu readback buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("webgl-wgpu readback encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).expect("send map result")
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
        receiver.recv().expect("map result").expect("map buffer");

        let mapped = slice.get_mapped_range();
        let mut pixels = vec![0; (row_bytes * height) as usize];
        for y in 0..height as usize {
            let src = y * padded_row_bytes as usize;
            let dst = y * row_bytes as usize;
            pixels[dst..dst + row_bytes as usize]
                .copy_from_slice(&mapped[src..src + row_bytes as usize]);
        }
        drop(mapped);
        buffer.unmap();
        pixels
    }

    #[test]
    fn webgl_canvas_to_netrender_texture_allocates_resizes_and_clears() {
        let (device, queue) = make_device();
        let capability = TestWgpuCapability {
            device: device.clone(),
            queue: queue.clone(),
        };
        let mut canvas =
            WebGlCanvas::from_wgpu_capability(&capability, WebGlCanvasDescriptor::new(4, 4))
                .expect("canvas");

        assert_eq!(canvas.texture().size, (4, 4));
        assert_eq!(canvas.texture().format, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(canvas.texture().alpha_mode, CanvasAlphaMode::Premultiplied);
        assert_eq!(canvas.texture().generation, 0);
        assert_eq!(canvas.texture().damage, Some([0, 0, 4, 4]));

        canvas.clear(wgpu::Color {
            r: 0.25,
            g: 0.5,
            b: 0.75,
            a: 1.0,
        });
        let pixels = read_rgba8(&device, &queue, &canvas.texture().texture, 4, 4);
        assert_eq!(&pixels[0..4], &[64, 128, 191, 255]);

        canvas.resize(2, 3).expect("resize");
        assert_eq!(canvas.texture().size, (2, 3));
        assert_eq!(canvas.texture().generation, 1);
        assert_eq!(canvas.texture().damage, Some([0, 0, 2, 3]));
    }

    #[test]
    fn webgl_canvas_rejects_empty_size() {
        let (device, queue) = make_device();
        let result =
            WebGlCanvas::from_wgpu_handles(device, queue, WebGlCanvasDescriptor::new(0, 1));
        assert!(matches!(result, Err(WebGlCanvasError::EmptySize)));
    }
}
