/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::sync::mpsc;

use dpi::PhysicalSize;
use image::RgbaImage;
use log::warn;
use webrender_api::units::DeviceIntRect;

fn clamp_readback_rect(
    rect: DeviceIntRect,
    size: PhysicalSize<u32>,
) -> Option<(wgpu::Origin3d, u32, u32)> {
    let width = size.width as i32;
    let height = size.height as i32;
    if width <= 0 || height <= 0 {
        return None;
    }

    let x0 = rect.min.x.clamp(0, width);
    let y0 = rect.min.y.clamp(0, height);
    let x1 = rect.max.x.clamp(x0, width);
    let y1 = rect.max.y.clamp(y0, height);
    let copy_width = (x1 - x0) as u32;
    let copy_height = (y1 - y0) as u32;
    if copy_width == 0 || copy_height == 0 {
        return None;
    }

    Some((
        wgpu::Origin3d {
            x: x0 as u32,
            y: y0 as u32,
            z: 0,
        },
        copy_width,
        copy_height,
    ))
}

fn normalize_pixels(format: wgpu::TextureFormat, pixels: &mut [u8]) -> Option<()> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Some(()),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            Some(())
        },
        _ => {
            warn!("Unsupported wgpu screenshot format: {format:?}");
            None
        },
    }
}

pub fn read_texture_to_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    size: PhysicalSize<u32>,
    rect: DeviceIntRect,
) -> Option<RgbaImage> {
    let (origin, width, height) = clamp_readback_rect(rect, size)?;
    let padded_bytes_per_row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("servo_wgpu_readback_buffer"),
        size: padded_bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("servo_wgpu_readback_encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
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

    let buffer_slice = readback_buffer.slice(..);
    let (sender, receiver) = mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    if device.poll(wgpu::PollType::wait_indefinitely()).is_err() {
        warn!("wgpu readback device poll failed");
        return None;
    }
    match receiver.recv().ok()? {
        Ok(()) => {},
        Err(error) => {
            warn!("wgpu readback map failed: {error}");
            return None;
        },
    }

    let image = {
        let mapped = buffer_slice.get_mapped_range();
        let row_bytes = (width * 4) as usize;
        let mut pixels = vec![0; row_bytes * height as usize];
        for row in 0..height as usize {
            let src_start = row * padded_bytes_per_row as usize;
            let dst_start = row * row_bytes;
            pixels[dst_start..dst_start + row_bytes]
                .copy_from_slice(&mapped[src_start..src_start + row_bytes]);
        }
        normalize_pixels(format, &mut pixels)?;
        RgbaImage::from_raw(width, height, pixels)
    };
    readback_buffer.unmap();
    image
}

#[cfg(test)]
mod test {
    use super::normalize_pixels;

    #[test]
    fn test_normalize_pixels_swizzles_bgra() {
        let mut pixels = vec![1, 2, 3, 4, 10, 20, 30, 40];
        normalize_pixels(wgpu::TextureFormat::Bgra8Unorm, &mut pixels)
            .expect("BGRA readback should be supported");
        assert_eq!(pixels, vec![3, 2, 1, 4, 30, 20, 10, 40]);
    }

    #[test]
    fn test_normalize_pixels_preserves_rgba() {
        let mut pixels = vec![1, 2, 3, 4, 10, 20, 30, 40];
        normalize_pixels(wgpu::TextureFormat::Rgba8Unorm, &mut pixels)
            .expect("RGBA readback should be supported");
        assert_eq!(pixels, vec![1, 2, 3, 4, 10, 20, 30, 40]);
    }
}