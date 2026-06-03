/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

impl WebGlContext {
    pub fn get_error(&mut self) -> WebGlError {
        let error = self.pending_error;
        self.pending_error = WebGlError::NoError;
        error
    }

    pub fn lose_context(&mut self) {
        self.lost = true;
        self.record_error(WebGlError::ContextLostWebgl);
    }

    pub fn restore_context(&mut self) -> Result<(), WebGlCanvasError> {
        let (width, height) = self.canvas.output.size;
        self.canvas.resize(width, height)?;
        self.buffers.clear();
        self.textures.clear();
        self.framebuffers.clear();
        self.renderbuffers.clear();
        self.shaders.clear();
        self.programs.clear();
        self.translated_programs.clear();
        self.attribs = [VertexAttribState::default(); MAX_VERTEX_ATTRIBS];
        self.bound_array_buffer = None;
        self.bound_element_array_buffer = None;
        self.bound_texture_2d_units = [None; MAX_TEXTURE_IMAGE_UNITS];
        self.active_texture_unit = 0;
        self.bound_framebuffer = None;
        self.bound_renderbuffer = None;
        self.current_program = None;
        self.viewport = [0, 0, width, height];
        self.scissor_box = [0, 0, width, height];
        self.scissor_test_enabled = false;
        self.lost = false;
        Ok(())
    }

    pub(super) fn bound_buffer_for_target(&self, target: BufferTarget) -> Option<WebGlBufferId> {
        match target {
            BufferTarget::ArrayBuffer => self.bound_array_buffer,
            BufferTarget::ElementArrayBuffer => self.bound_element_array_buffer,
        }
    }

    pub(super) fn current_color_target_view(
        &self,
    ) -> Option<(wgpu::TextureView, wgpu::TextureFormat, (u32, u32))> {
        let Some(framebuffer_id) = self.bound_framebuffer else {
            return Some((
                self.canvas.output.create_view(),
                self.canvas.output.format,
                self.canvas.output.size,
            ));
        };
        let framebuffer = self.framebuffers.get(&framebuffer_id)?;
        if let Some(texture_id) = framebuffer.color_texture {
            let texture = self.textures.get(&texture_id)?;
            return Some((
                texture
                    ._texture
                    .create_view(&wgpu::TextureViewDescriptor::default()),
                wgpu::TextureFormat::Rgba8Unorm,
                self.texture_extent(texture_id)?,
            ));
        }
        if let Some(renderbuffer_id) = framebuffer.color_renderbuffer {
            let renderbuffer = self.renderbuffers.get(&renderbuffer_id)?;
            return Some((
                renderbuffer
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default()),
                renderbuffer.format,
                renderbuffer.size,
            ));
        }
        None
    }

    pub(super) fn current_framebuffer_status(&self) -> WebGlFramebufferStatus {
        let Some(framebuffer_id) = self.bound_framebuffer else {
            return WebGlFramebufferStatus::Complete;
        };
        let Some(framebuffer) = self.framebuffers.get(&framebuffer_id) else {
            return WebGlFramebufferStatus::IncompleteAttachment;
        };
        match (framebuffer.color_texture, framebuffer.color_renderbuffer) {
            (None, None) => WebGlFramebufferStatus::IncompleteMissingAttachment,
            (Some(texture_id), None) => {
                if self.textures.contains_key(&texture_id) {
                    WebGlFramebufferStatus::Complete
                } else {
                    WebGlFramebufferStatus::IncompleteAttachment
                }
            },
            (None, Some(renderbuffer_id)) => {
                if self.renderbuffers.contains_key(&renderbuffer_id) {
                    WebGlFramebufferStatus::Complete
                } else {
                    WebGlFramebufferStatus::IncompleteAttachment
                }
            },
            (Some(_), Some(_)) => WebGlFramebufferStatus::IncompleteAttachment,
        }
    }

    pub(super) fn bound_texture_for_unit(
        &self,
        texture_unit: Option<u32>,
    ) -> Option<&TextureObject> {
        let unit = texture_unit? as usize;
        let texture_id = self.bound_texture_2d_units.get(unit).copied().flatten()?;
        self.textures.get(&texture_id)
    }

    pub(super) fn current_readback_texture(&self) -> Option<(&wgpu::Texture, (u32, u32))> {
        let Some(framebuffer_id) = self.bound_framebuffer else {
            return Some((&self.canvas.output.texture, self.canvas.output.size));
        };
        let framebuffer = self.framebuffers.get(&framebuffer_id)?;
        if let Some(texture_id) = framebuffer.color_texture {
            let texture = self.textures.get(&texture_id)?;
            return Some((&texture._texture, self.texture_extent(texture_id)?));
        }
        if let Some(renderbuffer_id) = framebuffer.color_renderbuffer {
            let renderbuffer = self.renderbuffers.get(&renderbuffer_id)?;
            return Some((&renderbuffer.texture, renderbuffer.size));
        }
        None
    }

    pub(super) fn texture_extent(&self, texture_id: WebGlTextureId) -> Option<(u32, u32)> {
        let texture = self.textures.get(&texture_id)?;
        let size = texture._texture.size();
        Some((size.width, size.height))
    }

    pub(super) fn attrib_mut(&mut self, index: u32) -> Option<&mut VertexAttribState> {
        self.attribs.get_mut(index as usize)
    }

    pub(super) fn record_error(&mut self, error: WebGlError) {
        if self.pending_error == WebGlError::NoError {
            self.pending_error = error;
        }
    }

    pub(super) fn set_program_link_failure(&mut self, program: WebGlProgramId, message: &str) {
        let Some(program) = self.programs.get_mut(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        program.translated = None;
        program.reflection = None;
        program.pipelines.clear();
        program.uniform_block_bytes.clear();
        program.sampler_texture_units.clear();
        program.link_status = false;
        program.info_log = message.to_string();
    }

    pub(super) fn apply_draw_state<'pass>(&self, pass: &mut wgpu::RenderPass<'pass>) {
        pass.set_viewport(
            self.viewport[0] as f32,
            self.viewport[1] as f32,
            self.viewport[2] as f32,
            self.viewport[3] as f32,
            0.0,
            1.0,
        );
        if self.scissor_test_enabled {
            pass.set_scissor_rect(
                self.scissor_box[0],
                self.scissor_box[1],
                self.scissor_box[2],
                self.scissor_box[3],
            );
        }
    }
}
