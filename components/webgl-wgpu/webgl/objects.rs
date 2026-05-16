/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::pipeline::{f32_slice_to_bytes, u16_slice_to_bytes};
use super::*;

impl WebGlContext {
    pub fn create_buffer(&mut self) -> WebGlBufferId {
        let id = WebGlBufferId(self.next_buffer_id);
        self.next_buffer_id += 1;
        id
    }

    pub fn create_texture(&mut self) -> WebGlTextureId {
        let id = WebGlTextureId(self.next_texture_id);
        self.next_texture_id += 1;
        id
    }

    pub fn create_framebuffer(&mut self) -> WebGlFramebufferId {
        let id = WebGlFramebufferId(self.next_framebuffer_id);
        self.next_framebuffer_id += 1;
        self.framebuffers.insert(id, FramebufferObject::default());
        id
    }

    pub fn create_renderbuffer(&mut self) -> WebGlRenderbufferId {
        let id = WebGlRenderbufferId(self.next_renderbuffer_id);
        self.next_renderbuffer_id += 1;
        id
    }

    pub fn bind_buffer(&mut self, target: BufferTarget, buffer: Option<WebGlBufferId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        match target {
            BufferTarget::ArrayBuffer => self.bound_array_buffer = buffer,
            BufferTarget::ElementArrayBuffer => self.bound_element_array_buffer = buffer,
        }
    }

    pub fn bind_texture_2d(&mut self, texture: Option<WebGlTextureId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        self.bound_texture_2d_units[self.active_texture_unit as usize] = texture;
    }

    pub fn active_texture(&mut self, unit: u32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if unit as usize >= MAX_TEXTURE_IMAGE_UNITS {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        self.active_texture_unit = unit;
    }

    pub fn bind_framebuffer(&mut self, framebuffer: Option<WebGlFramebufferId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if framebuffer.is_some_and(|id| !self.framebuffers.contains_key(&id)) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        self.bound_framebuffer = framebuffer;
    }

    pub fn bind_renderbuffer(&mut self, renderbuffer: Option<WebGlRenderbufferId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        self.bound_renderbuffer = renderbuffer;
    }

    pub fn tex_image_2d_rgba8(&mut self, width: u32, height: u32, pixels: &[u8]) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if width == 0 || height == 0 || pixels.len() != width as usize * height as usize * 4 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        let Some(id) = self.bound_texture_2d_units[self.active_texture_unit as usize] else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let texture = self.canvas.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgl-wgpu rgba8 texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.canvas.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.insert(
            id,
            TextureObject {
                _texture: texture,
                view,
            },
        );
    }

    pub fn renderbuffer_storage_rgba8(&mut self, width: u32, height: u32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if width == 0 || height == 0 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        let Some(id) = self.bound_renderbuffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let texture = self.canvas.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("webgl-wgpu rgba8 renderbuffer"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.renderbuffers.insert(
            id,
            RenderbufferObject {
                texture,
                size: (width, height),
                format: wgpu::TextureFormat::Rgba8Unorm,
            },
        );
    }

    pub fn framebuffer_texture_2d(&mut self, texture: Option<WebGlTextureId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if texture.is_some_and(|texture| !self.textures.contains_key(&texture)) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(framebuffer_id) = self.bound_framebuffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(framebuffer) = self.framebuffers.get_mut(&framebuffer_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        framebuffer.color_texture = texture;
        framebuffer.color_renderbuffer = None;
    }

    pub fn framebuffer_renderbuffer(&mut self, renderbuffer: Option<WebGlRenderbufferId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if renderbuffer.is_some_and(|renderbuffer| !self.renderbuffers.contains_key(&renderbuffer))
        {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(framebuffer_id) = self.bound_framebuffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(framebuffer) = self.framebuffers.get_mut(&framebuffer_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        framebuffer.color_texture = None;
        framebuffer.color_renderbuffer = renderbuffer;
    }

    pub fn check_framebuffer_status(&mut self) -> WebGlFramebufferStatus {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return WebGlFramebufferStatus::IncompleteAttachment;
        }
        self.current_framebuffer_status()
    }

    pub fn viewport(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if width == 0 || height == 0 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        self.viewport = [x, y, width, height];
    }

    pub fn scissor(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if width == 0 || height == 0 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        self.scissor_box = [x, y, width, height];
    }

    pub fn set_scissor_test_enabled(&mut self, enabled: bool) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        self.scissor_test_enabled = enabled;
    }

    pub fn buffer_data_f32(&mut self, target: BufferTarget, data: &[f32], _usage: BufferUsage) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        let Some(id) = self.bound_buffer_for_target(target) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let bytes = f32_slice_to_bytes(data);
        let buffer = self.canvas.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgl-wgpu array buffer"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        buffer
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(&bytes);
        buffer.unmap();
        self.buffers.insert(
            id,
            BufferObject {
                buffer,
                byte_len: bytes.len() as u64,
                index_u16: None,
            },
        );
    }

    pub fn buffer_data_u16(&mut self, target: BufferTarget, data: &[u16], _usage: BufferUsage) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if target != BufferTarget::ElementArrayBuffer {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        let Some(id) = self.bound_buffer_for_target(target) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let bytes = u16_slice_to_bytes(data);
        let padded_size = bytes.len().div_ceil(4) * 4;
        let mut padded_bytes = vec![0u8; padded_size];
        padded_bytes[..bytes.len()].copy_from_slice(&bytes);
        let buffer = self.canvas.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgl-wgpu element array buffer"),
            size: padded_size as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        buffer
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(&padded_bytes);
        buffer.unmap();
        self.buffers.insert(
            id,
            BufferObject {
                buffer,
                byte_len: bytes.len() as u64,
                index_u16: Some(data.to_vec()),
            },
        );
    }
}
