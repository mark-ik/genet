/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::pipeline::{
    build_render_pipeline, build_texture_bind_group, build_uniform_bind_group,
    effective_vertex_stride, vertex_component_count, vertex_stride,
};
use super::*;

impl WebGlContext {
    pub fn draw_arrays(&mut self, mode: PrimitiveMode, first: u32, count: u32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if count == 0 {
            return;
        }
        let Some(program_id) = self.current_program else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(reflection) = program.reflection.clone() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(translated) = program.translated.clone() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let uniform_value = program.fragment_color_uniform;
        let texture_unit = program.fragment_texture_unit;
        let Some(attrib) = self
            .attribs
            .get(reflection.position_attribute.location as usize)
            .copied()
        else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let position_kind = reflection.position_attribute.kind;
        if !attrib.enabled || attrib.size != vertex_component_count(position_kind) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let position_bytes = vertex_stride(position_kind);
        let position_stride = effective_vertex_stride(attrib, position_kind);
        if position_stride < position_bytes {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(position_buffer_id) = attrib.buffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(position_buffer) = self.buffers.get(&position_buffer_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let position_required_bytes =
            attrib.offset + (first as u64 + count as u64 - 1) * position_stride + position_bytes;
        if position_required_bytes > position_buffer.byte_len {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let color_attribute = if let Some(color_reflection) = reflection.color_attribute.as_ref() {
            let Some(color_attrib) = self
                .attribs
                .get(color_reflection.location as usize)
                .copied()
            else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let color_kind = color_reflection.kind;
            if !color_attrib.enabled || color_attrib.size != vertex_component_count(color_kind) {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            let color_bytes = vertex_stride(color_kind);
            let color_stride = effective_vertex_stride(color_attrib, color_kind);
            if color_stride < color_bytes {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            let Some(color_buffer_id) = color_attrib.buffer else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let Some(color_buffer) = self.buffers.get(&color_buffer_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let color_required_bytes = color_attrib.offset
                + (first as u64 + count as u64 - 1) * color_stride
                + color_bytes;
            if color_required_bytes > color_buffer.byte_len {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            Some((color_attrib, color_stride, color_buffer))
        } else {
            None
        };
        let texcoord_attribute =
            if let Some(texcoord_reflection) = reflection.texcoord_attribute.as_ref() {
                let Some(texcoord_attrib) = self
                    .attribs
                    .get(texcoord_reflection.location as usize)
                    .copied()
                else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let texcoord_kind = texcoord_reflection.kind;
                if !texcoord_attrib.enabled
                    || texcoord_attrib.size != vertex_component_count(texcoord_kind)
                {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                let texcoord_bytes = vertex_stride(texcoord_kind);
                let texcoord_stride = effective_vertex_stride(texcoord_attrib, texcoord_kind);
                if texcoord_stride < texcoord_bytes {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                let Some(texcoord_buffer_id) = texcoord_attrib.buffer else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let Some(texcoord_buffer) = self.buffers.get(&texcoord_buffer_id) else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let texcoord_required_bytes = texcoord_attrib.offset
                    + (first as u64 + count as u64 - 1) * texcoord_stride
                    + texcoord_bytes;
                if texcoord_required_bytes > texcoord_buffer.byte_len {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                Some((texcoord_attrib, texcoord_stride, texcoord_buffer))
            } else {
                None
            };

        let topology = match mode {
            PrimitiveMode::Triangles => wgpu::PrimitiveTopology::TriangleList,
        };
        if topology != wgpu::PrimitiveTopology::TriangleList {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        let pipeline_key = VertexPipelineKey {
            position_stride,
            position_offset: attrib.offset,
            color_stride: color_attribute.as_ref().map(|(_, stride, _)| *stride),
            color_offset: color_attribute
                .as_ref()
                .map(|(color_attrib, _, _)| color_attrib.offset),
            texcoord_stride: texcoord_attribute.as_ref().map(|(_, stride, _)| *stride),
            texcoord_offset: texcoord_attribute
                .as_ref()
                .map(|(texcoord_attrib, _, _)| texcoord_attrib.offset),
        };
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        }
        let Some((view, target_format, _)) = self.current_color_target_view() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        };
        let needs_pipeline = self.programs.get(&program_id).map_or(true, |program| {
            !program.pipelines.contains_key(&pipeline_key)
        });
        if needs_pipeline {
            let pipeline = build_render_pipeline(
                &self.canvas.device,
                target_format,
                &translated,
                &reflection,
                pipeline_key,
            );
            let Some(program) = self.programs.get_mut(&program_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            program.pipelines.insert(pipeline_key, pipeline);
        }
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(pipeline) = program.pipelines.get(&pipeline_key) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let uniform_bind_group = match (uniform_value, pipeline.uniform_bind_group_layout.as_ref())
        {
            (Some(value), Some(layout)) => {
                Some(build_uniform_bind_group(&self.canvas.device, layout, value))
            },
            (None, None) => None,
            _ => {
                self.record_error(WebGlError::InvalidOperation);
                return;
            },
        };
        let texture_image_binding = reflection
            .fragment_texture_uniform
            .as_ref()
            .map(|uniform| uniform.binding);
        let texture_bind_group = match (
            texture_unit,
            pipeline.texture_bind_group_layout.as_ref(),
            self.bound_texture_for_unit(texture_unit),
            texture_image_binding,
        ) {
            (Some(0), Some(layout), Some(texture), Some(image_binding)) => {
                Some(build_texture_bind_group(
                    &self.canvas.device,
                    layout,
                    &texture.view,
                    image_binding,
                ))
            },
            (Some(_), Some(layout), Some(texture), Some(image_binding)) => {
                Some(build_texture_bind_group(
                    &self.canvas.device,
                    layout,
                    &texture.view,
                    image_binding,
                ))
            },
            (None, None, _, _) => None,
            _ => {
                self.record_error(WebGlError::InvalidOperation);
                return;
            },
        };

        let mut encoder =
            self.canvas
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("webgl-wgpu draw encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgl-wgpu draw pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            if let Some((_, bind_group)) = uniform_bind_group.as_ref() {
                pass.set_bind_group(0, bind_group, &[]);
            }
            if let Some((_, bind_group)) = texture_bind_group.as_ref() {
                pass.set_bind_group(0, bind_group, &[]);
            }
            self.apply_draw_state(&mut pass);
            pass.set_vertex_buffer(0, position_buffer.buffer.slice(..));
            if let Some((_, _, color_buffer)) = color_attribute {
                pass.set_vertex_buffer(1, color_buffer.buffer.slice(..));
            }
            if let Some((_, _, texcoord_buffer)) = texcoord_attribute {
                pass.set_vertex_buffer(1, texcoord_buffer.buffer.slice(..));
            }
            pass.draw(first..first + count, 0..1);
        }
        self.canvas.queue.submit([encoder.finish()]);
        if self.bound_framebuffer.is_none() {
            self.canvas.output.damage =
                Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
        }
    }

    pub fn draw_elements(
        &mut self,
        mode: PrimitiveMode,
        count: u32,
        index_type: IndexType,
        offset: u64,
    ) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if count == 0 {
            return;
        }
        if index_type != IndexType::UnsignedShort || offset % 2 != 0 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        let Some(program_id) = self.current_program else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(reflection) = program.reflection.clone() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(translated) = program.translated.clone() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let uniform_value = program.fragment_color_uniform;
        let texture_unit = program.fragment_texture_unit;
        let Some(index_buffer_id) = self.bound_element_array_buffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(index_buffer) = self.buffers.get(&index_buffer_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let required_index_bytes = offset + count as u64 * 2;
        if required_index_bytes > index_buffer.byte_len {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(indices) = index_buffer.index_u16.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let first_index = (offset / 2) as usize;
        let last_index_exclusive = first_index + count as usize;
        let Some(index_slice) = indices.get(first_index..last_index_exclusive) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let max_vertex_index = index_slice.iter().copied().max().unwrap_or(0) as u64;

        let Some(attrib) = self
            .attribs
            .get(reflection.position_attribute.location as usize)
            .copied()
        else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let position_kind = reflection.position_attribute.kind;
        if !attrib.enabled || attrib.size != vertex_component_count(position_kind) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let position_bytes = vertex_stride(position_kind);
        let position_stride = effective_vertex_stride(attrib, position_kind);
        if position_stride < position_bytes {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(position_buffer_id) = attrib.buffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(position_buffer) = self.buffers.get(&position_buffer_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let position_required_bytes =
            attrib.offset + max_vertex_index * position_stride + position_bytes;
        if position_required_bytes > position_buffer.byte_len {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let color_attribute = if let Some(color_reflection) = reflection.color_attribute.as_ref() {
            let Some(color_attrib) = self
                .attribs
                .get(color_reflection.location as usize)
                .copied()
            else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let color_kind = color_reflection.kind;
            if !color_attrib.enabled || color_attrib.size != vertex_component_count(color_kind) {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            let color_bytes = vertex_stride(color_kind);
            let color_stride = effective_vertex_stride(color_attrib, color_kind);
            if color_stride < color_bytes {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            let Some(color_buffer_id) = color_attrib.buffer else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let Some(color_buffer) = self.buffers.get(&color_buffer_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            let color_required_bytes =
                color_attrib.offset + max_vertex_index * color_stride + color_bytes;
            if color_required_bytes > color_buffer.byte_len {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
            Some((color_attrib, color_stride, color_buffer))
        } else {
            None
        };
        let texcoord_attribute =
            if let Some(texcoord_reflection) = reflection.texcoord_attribute.as_ref() {
                let Some(texcoord_attrib) = self
                    .attribs
                    .get(texcoord_reflection.location as usize)
                    .copied()
                else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let texcoord_kind = texcoord_reflection.kind;
                if !texcoord_attrib.enabled
                    || texcoord_attrib.size != vertex_component_count(texcoord_kind)
                {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                let texcoord_bytes = vertex_stride(texcoord_kind);
                let texcoord_stride = effective_vertex_stride(texcoord_attrib, texcoord_kind);
                if texcoord_stride < texcoord_bytes {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                let Some(texcoord_buffer_id) = texcoord_attrib.buffer else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let Some(texcoord_buffer) = self.buffers.get(&texcoord_buffer_id) else {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                };
                let texcoord_required_bytes =
                    texcoord_attrib.offset + max_vertex_index * texcoord_stride + texcoord_bytes;
                if texcoord_required_bytes > texcoord_buffer.byte_len {
                    self.record_error(WebGlError::InvalidOperation);
                    return;
                }
                Some((texcoord_attrib, texcoord_stride, texcoord_buffer))
            } else {
                None
            };

        let topology = match mode {
            PrimitiveMode::Triangles => wgpu::PrimitiveTopology::TriangleList,
        };
        if topology != wgpu::PrimitiveTopology::TriangleList {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        let pipeline_key = VertexPipelineKey {
            position_stride,
            position_offset: attrib.offset,
            color_stride: color_attribute.as_ref().map(|(_, stride, _)| *stride),
            color_offset: color_attribute
                .as_ref()
                .map(|(color_attrib, _, _)| color_attrib.offset),
            texcoord_stride: texcoord_attribute.as_ref().map(|(_, stride, _)| *stride),
            texcoord_offset: texcoord_attribute
                .as_ref()
                .map(|(texcoord_attrib, _, _)| texcoord_attrib.offset),
        };
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        }
        let Some((view, target_format, _)) = self.current_color_target_view() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        };
        let needs_pipeline = self.programs.get(&program_id).map_or(true, |program| {
            !program.pipelines.contains_key(&pipeline_key)
        });
        if needs_pipeline {
            let pipeline = build_render_pipeline(
                &self.canvas.device,
                target_format,
                &translated,
                &reflection,
                pipeline_key,
            );
            let Some(program) = self.programs.get_mut(&program_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            program.pipelines.insert(pipeline_key, pipeline);
        }
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(pipeline) = program.pipelines.get(&pipeline_key) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let uniform_bind_group = match (uniform_value, pipeline.uniform_bind_group_layout.as_ref())
        {
            (Some(value), Some(layout)) => {
                Some(build_uniform_bind_group(&self.canvas.device, layout, value))
            },
            (None, None) => None,
            _ => {
                self.record_error(WebGlError::InvalidOperation);
                return;
            },
        };
        let texture_image_binding = reflection
            .fragment_texture_uniform
            .as_ref()
            .map(|uniform| uniform.binding);
        let texture_bind_group = match (
            texture_unit,
            pipeline.texture_bind_group_layout.as_ref(),
            self.bound_texture_for_unit(texture_unit),
            texture_image_binding,
        ) {
            (Some(0), Some(layout), Some(texture), Some(image_binding)) => {
                Some(build_texture_bind_group(
                    &self.canvas.device,
                    layout,
                    &texture.view,
                    image_binding,
                ))
            },
            (Some(_), Some(layout), Some(texture), Some(image_binding)) => {
                Some(build_texture_bind_group(
                    &self.canvas.device,
                    layout,
                    &texture.view,
                    image_binding,
                ))
            },
            (None, None, _, _) => None,
            _ => {
                self.record_error(WebGlError::InvalidOperation);
                return;
            },
        };

        let mut encoder =
            self.canvas
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("webgl-wgpu indexed draw encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgl-wgpu indexed draw pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            if let Some((_, bind_group)) = uniform_bind_group.as_ref() {
                pass.set_bind_group(0, bind_group, &[]);
            }
            if let Some((_, bind_group)) = texture_bind_group.as_ref() {
                pass.set_bind_group(0, bind_group, &[]);
            }
            self.apply_draw_state(&mut pass);
            pass.set_vertex_buffer(0, position_buffer.buffer.slice(..));
            if let Some((_, _, color_buffer)) = color_attribute {
                pass.set_vertex_buffer(1, color_buffer.buffer.slice(..));
            }
            if let Some((_, _, texcoord_buffer)) = texcoord_attribute {
                pass.set_vertex_buffer(1, texcoord_buffer.buffer.slice(..));
            }
            pass.set_index_buffer(index_buffer.buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(first_index as u32..first_index as u32 + count, 0, 0..1);
        }
        self.canvas.queue.submit([encoder.finish()]);
        if self.bound_framebuffer.is_none() {
            self.canvas.output.damage =
                Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
        }
    }
}
