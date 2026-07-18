/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::pipeline::{
    build_group_zero_bind_group, build_render_pipeline, effective_vertex_stride, vertex_stride,
};
use super::*;

/// Per-attribute resolution. Holds the buffer ID rather than
/// the buffer reference so the borrow checker lets the call
/// site interleave it with `programs` mutation.
#[derive(Clone, Copy)]
struct AttributeResolution {
    buffer_id: WebGlBufferId,
}

impl WebGlContext {
    fn resolve_attribute_pipeline_inputs(
        &self,
        reflection: &ProgramReflection,
        max_vertex_index: u64,
    ) -> Result<(Vec<AttributeBufferLayout>, Vec<AttributeResolution>), WebGlError> {
        let mut layouts = Vec::with_capacity(reflection.attributes.len());
        let mut resolutions = Vec::with_capacity(reflection.attributes.len());
        for attribute in &reflection.attributes {
            let attrib = *self
                .attribs
                .get(attribute.location as usize)
                .ok_or(WebGlError::InvalidOperation)?;
            if !attrib.enabled || !matches!(attrib.size, 1 | 2 | 3 | 4) {
                return Err(WebGlError::InvalidOperation);
            }
            // WebGL fills missing components when the vertex array format
            // has fewer components than the shader input (and ignores extra
            // components in the opposite direction). Keep the configured
            // array width for the GPU vertex format instead of requiring an
            // exact match with the reflected shader type.
            let format = match attrib.size {
                1 => VertexAttributeKind::Float32,
                2 => VertexAttributeKind::Float32x2,
                3 => VertexAttributeKind::Float32x3,
                4 => VertexAttributeKind::Float32x4,
                _ => unreachable!(),
            };
            let kind_bytes = vertex_stride(format);
            let stride = effective_vertex_stride(attrib, format);
            if stride < kind_bytes {
                return Err(WebGlError::InvalidOperation);
            }
            let buffer_id = attrib.buffer.ok_or(WebGlError::InvalidOperation)?;
            let buffer = self
                .buffers
                .get(&buffer_id)
                .ok_or(WebGlError::InvalidOperation)?;
            let required_bytes = attrib.offset + max_vertex_index * stride + kind_bytes;
            if required_bytes > buffer.byte_len {
                return Err(WebGlError::InvalidOperation);
            }
            layouts.push(AttributeBufferLayout {
                format,
                stride,
                offset: attrib.offset,
            });
            resolutions.push(AttributeResolution { buffer_id });
        }
        Ok((layouts, resolutions))
    }

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
        let uniform_block_bytes = program.uniform_block_bytes.clone();
        let sampler_texture_units = program.sampler_texture_units.clone();

        let max_vertex_index = (first as u64 + count as u64).saturating_sub(1);
        let (attribute_layouts, resolutions) =
            match self.resolve_attribute_pipeline_inputs(&reflection, max_vertex_index) {
                Ok(v) => v,
                Err(error) => {
                    self.record_error(error);
                    return;
                },
            };
        let depth_state = if self.depth_test_enabled && self.canvas.has_depth() {
            Some(self.depth_func)
        } else {
            None
        };
        let pipeline_key = VertexPipelineKey {
            attribute_layouts: attribute_layouts.clone(),
            depth_state,
            color_write_mask: color_mask_bits(self.color_mask),
        };

        let topology = match mode {
            PrimitiveMode::Triangles => wgpu::PrimitiveTopology::TriangleList,
        };
        if topology != wgpu::PrimitiveTopology::TriangleList {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        }
        let Some((view, target_format, _)) = self.current_color_target_view() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        };

        // Resolve sampler texture views before borrowing the
        // pipeline-cache slot mutably. Each sampler that has a
        // bound texture-unit + texture contributes one
        // (image_binding, sampler_binding, &view) row.
        let sampler_texture_ids =
            match self.resolve_sampler_texture_ids(&reflection, &sampler_texture_units) {
                Ok(v) => v,
                Err(error) => {
                    self.record_error(error);
                    return;
                },
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
                &pipeline_key,
            );
            let Some(program) = self.programs.get_mut(&program_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            program.pipelines.insert(pipeline_key.clone(), pipeline);
        }
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(pipeline) = program.pipelines.get(&pipeline_key) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };

        let sampler_views: Vec<(u32, u32, &wgpu::TextureView)> = sampler_texture_ids
            .iter()
            .filter_map(|(image, sampler, id)| {
                self.textures
                    .get(id)
                    .map(|texture| (*image, *sampler, &texture.view))
            })
            .collect();
        let group_zero = pipeline.group_zero_layout.as_ref().map(|layout| {
            build_group_zero_bind_group(
                &self.canvas.device,
                layout,
                if uniform_block_bytes.is_empty() {
                    None
                } else {
                    Some(uniform_block_bytes.as_slice())
                },
                &sampler_views,
            )
        });

        let depth_stencil_attachment = if depth_state.is_some() {
            self.canvas
                .depth_view()
                .map(|view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                })
        } else {
            None
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
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            if let Some(group) = group_zero.as_ref() {
                pass.set_bind_group(0, &group.bind_group, &[]);
            }
            self.apply_draw_state(&mut pass);
            for (slot, resolution) in resolutions.iter().enumerate() {
                let Some(buffer) = self.buffers.get(&resolution.buffer_id) else {
                    continue;
                };
                pass.set_vertex_buffer(slot as u32, buffer.buffer.slice(..));
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
        let uniform_block_bytes = program.uniform_block_bytes.clone();
        let sampler_texture_units = program.sampler_texture_units.clone();
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

        let (attribute_layouts, resolutions) =
            match self.resolve_attribute_pipeline_inputs(&reflection, max_vertex_index) {
                Ok(v) => v,
                Err(error) => {
                    self.record_error(error);
                    return;
                },
            };
        let depth_state = if self.depth_test_enabled && self.canvas.has_depth() {
            Some(self.depth_func)
        } else {
            None
        };
        let pipeline_key = VertexPipelineKey {
            attribute_layouts: attribute_layouts.clone(),
            depth_state,
            color_write_mask: color_mask_bits(self.color_mask),
        };

        let topology = match mode {
            PrimitiveMode::Triangles => wgpu::PrimitiveTopology::TriangleList,
        };
        if topology != wgpu::PrimitiveTopology::TriangleList {
            self.record_error(WebGlError::InvalidEnum);
            return;
        }
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        }
        let Some((view, target_format, _)) = self.current_color_target_view() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        };

        let sampler_texture_ids =
            match self.resolve_sampler_texture_ids(&reflection, &sampler_texture_units) {
                Ok(v) => v,
                Err(error) => {
                    self.record_error(error);
                    return;
                },
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
                &pipeline_key,
            );
            let Some(program) = self.programs.get_mut(&program_id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            program.pipelines.insert(pipeline_key.clone(), pipeline);
        }
        let Some(program) = self.programs.get(&program_id) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(pipeline) = program.pipelines.get(&pipeline_key) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };

        let sampler_views: Vec<(u32, u32, &wgpu::TextureView)> = sampler_texture_ids
            .iter()
            .filter_map(|(image, sampler, id)| {
                self.textures
                    .get(id)
                    .map(|texture| (*image, *sampler, &texture.view))
            })
            .collect();
        let group_zero = pipeline.group_zero_layout.as_ref().map(|layout| {
            build_group_zero_bind_group(
                &self.canvas.device,
                layout,
                if uniform_block_bytes.is_empty() {
                    None
                } else {
                    Some(uniform_block_bytes.as_slice())
                },
                &sampler_views,
            )
        });

        let depth_stencil_attachment = if depth_state.is_some() {
            self.canvas
                .depth_view()
                .map(|view| wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                })
        } else {
            None
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
                depth_stencil_attachment,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            if let Some(group) = group_zero.as_ref() {
                pass.set_bind_group(0, &group.bind_group, &[]);
            }
            self.apply_draw_state(&mut pass);
            for (slot, resolution) in resolutions.iter().enumerate() {
                let Some(buffer) = self.buffers.get(&resolution.buffer_id) else {
                    continue;
                };
                pass.set_vertex_buffer(slot as u32, buffer.buffer.slice(..));
            }
            if let Some(index_buffer) = self.buffers.get(&index_buffer_id) {
                pass.set_index_buffer(index_buffer.buffer.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(first_index as u32..first_index as u32 + count, 0, 0..1);
            }
        }
        self.canvas.queue.submit([encoder.finish()]);
        if self.bound_framebuffer.is_none() {
            self.canvas.output.damage =
                Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
        }
    }

    /// Resolve each sampler's bound texture by *unit*, not by
    /// view reference. Holding raw `WebGlTextureId`s here lets
    /// the caller continue mutating `self.programs` (e.g. to
    /// insert a new pipeline-cache entry) before turning them
    /// into `&wgpu::TextureView`s at bind-group-build time.
    /// Dispatches on `sampler.kind`: a `sampler2D` reads from
    /// `bound_texture_2d_units`, a `samplerCube` from
    /// `bound_texture_cube_units`.
    fn resolve_sampler_texture_ids(
        &self,
        reflection: &ProgramReflection,
        sampler_texture_units: &[Option<u32>],
    ) -> Result<Vec<(u32, u32, WebGlTextureId)>, WebGlError> {
        let mut ids = Vec::with_capacity(reflection.samplers.len());
        for (index, sampler) in reflection.samplers.iter().enumerate() {
            let unit = sampler_texture_units
                .get(index)
                .copied()
                .flatten()
                .ok_or(WebGlError::InvalidOperation)?;
            let unit_slot = match sampler.kind {
                crate::shader::UniformKind::Sampler2D => self
                    .bound_texture_2d_units
                    .get(unit as usize)
                    .ok_or(WebGlError::InvalidOperation)?,
                crate::shader::UniformKind::SamplerCube => self
                    .bound_texture_cube_units
                    .get(unit as usize)
                    .ok_or(WebGlError::InvalidOperation)?,
                _ => return Err(WebGlError::InvalidOperation),
            };
            let texture_id = unit_slot.ok_or(WebGlError::InvalidOperation)?;
            // Verify the bound texture's kind agrees with the
            // sampler's declared kind — catches the case where
            // a 2D texture got bound to the CUBE slot or vice
            // versa.
            if let Some(texture) = self.textures.get(&texture_id) {
                let expected = match sampler.kind {
                    crate::shader::UniformKind::Sampler2D => TextureKind::Texture2D,
                    crate::shader::UniformKind::SamplerCube => TextureKind::TextureCube,
                    _ => return Err(WebGlError::InvalidOperation),
                };
                if texture.kind != expected {
                    return Err(WebGlError::InvalidOperation);
                }
            }
            ids.push((sampler.image_binding, sampler.sampler_binding, texture_id));
        }
        Ok(ids)
    }
}
