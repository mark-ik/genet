/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::sync::mpsc;

use super::*;

impl WebGlContext {
    pub fn read_pixels(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, WebGlCanvasError> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return Ok(Vec::new());
        }
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return Ok(Vec::new());
        }
        let Some((texture, (target_width, target_height))) = self.current_readback_texture() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return Ok(Vec::new());
        };
        if width == 0 || height == 0 || x + width > target_width || y + height > target_height {
            self.record_error(WebGlError::InvalidValue);
            return Ok(Vec::new());
        }
        Ok(read_texture_rect_rgba8(
            &self.canvas.device,
            &self.canvas.queue,
            texture,
            x,
            y,
            width,
            height,
        ))
    }
}

fn read_texture_rect_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let row_bytes = width * 4;
    let padded_row_bytes =
        row_bytes.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer_size = padded_row_bytes as u64 * height as u64;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("webgl-wgpu read pixels buffer"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("webgl-wgpu read pixels encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
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
    for row in 0..height as usize {
        let src = row * padded_row_bytes as usize;
        let dst = row * row_bytes as usize;
        pixels[dst..dst + row_bytes as usize]
            .copy_from_slice(&mapped[src..src + row_bytes as usize]);
    }
    drop(mapped);
    buffer.unmap();
    pixels
}
