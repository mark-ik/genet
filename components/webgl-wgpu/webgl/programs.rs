/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::pipeline::f32_slice_to_bytes;
use super::*;

impl WebGlContext {
    pub fn create_shader(&mut self, stage: ShaderStage) -> WebGlShaderId {
        let id = WebGlShaderId(self.next_shader_id);
        self.next_shader_id += 1;
        self.shaders.insert(
            id,
            ShaderObject {
                stage,
                source: String::new(),
                compile_status: false,
                info_log: String::new(),
            },
        );
        id
    }

    pub fn shader_source(&mut self, shader: WebGlShaderId, source: &str) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        let Some(shader) = self.shaders.get_mut(&shader) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        // WebGL 1's draw-buffer form is commonly emitted by the Khronos
        // helpers as `gl_FragData[0]`. The current Rust frontend owns one
        // fragment output (`gl_FragColor`), so normalize the primary slot at
        // this boundary before validation and lowering. Higher draw-buffer
        // indices remain unsupported and must not be silently collapsed.
        shader.source = if matches!(shader.stage, ShaderStage::Fragment) {
            source.replace("gl_FragData[0]", "gl_FragColor")
        } else {
            source.to_string()
        };
        shader.compile_status = false;
        shader.info_log.clear();
    }

    pub fn compile_shader(&mut self, shader: WebGlShaderId) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        let Some(shader) = self.shaders.get_mut(&shader) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let validation = match shader.stage {
            ShaderStage::Vertex => validate_canonical_vertex_source(&shader.source),
            ShaderStage::Fragment => validate_canonical_fragment_source(&shader.source),
        };
        match validation {
            Ok(()) => {
                shader.compile_status = true;
                shader.info_log.clear();
            },
            Err(error) => {
                shader.compile_status = false;
                shader.info_log = format!("{:?} shader compile failed: {error}", shader.stage);
            },
        }
    }

    pub fn get_shader_compile_status(&mut self, shader: WebGlShaderId) -> bool {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return false;
        }
        let Some(shader) = self.shaders.get(&shader) else {
            self.record_error(WebGlError::InvalidOperation);
            return false;
        };
        shader.compile_status
    }

    pub fn get_shader_info_log(&mut self, shader: WebGlShaderId) -> Option<String> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return None;
        }
        let Some(shader) = self.shaders.get(&shader) else {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        };
        Some(shader.info_log.clone())
    }

    pub fn create_program(&mut self) -> WebGlProgramId {
        let id = WebGlProgramId(self.next_program_id);
        self.next_program_id += 1;
        self.programs.insert(
            id,
            ProgramObject {
                attached_vertex_shader: None,
                attached_fragment_shader: None,
                translated: None,
                reflection: None,
                pipelines: HashMap::new(),
                uniform_block_bytes: Vec::new(),
                sampler_texture_units: Vec::new(),
                link_status: false,
                info_log: String::new(),
            },
        );
        id
    }

    pub fn attach_shader(&mut self, program: WebGlProgramId, shader: WebGlShaderId) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        let Some(shader_object) = self.shaders.get(&shader) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(program) = self.programs.get_mut(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        match shader_object.stage {
            ShaderStage::Vertex => program.attached_vertex_shader = Some(shader),
            ShaderStage::Fragment => program.attached_fragment_shader = Some(shader),
        }
        program.link_status = false;
        program.translated = None;
        program.reflection = None;
        program.pipelines.clear();
        program.uniform_block_bytes.clear();
        program.sampler_texture_units.clear();
        program.info_log.clear();
    }

    pub fn link_program(&mut self, program: WebGlProgramId) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        let Some((vertex_shader_id, fragment_shader_id)) =
            self.programs.get(&program).and_then(|program| {
                Some((
                    program.attached_vertex_shader?,
                    program.attached_fragment_shader?,
                ))
            })
        else {
            self.set_program_link_failure(program, "program link failed: missing shader stage");
            return;
        };
        let Some(vertex_shader) = self.shaders.get(&vertex_shader_id) else {
            self.set_program_link_failure(program, "program link failed: missing vertex shader");
            return;
        };
        let Some(fragment_shader) = self.shaders.get(&fragment_shader_id) else {
            self.set_program_link_failure(program, "program link failed: missing fragment shader");
            return;
        };
        if !vertex_shader.compile_status || !fragment_shader.compile_status {
            self.set_program_link_failure(
                program,
                "program link failed: attached shader did not compile",
            );
            return;
        }
        let vertex_source = vertex_shader.source.clone();
        let fragment_source = fragment_shader.source.clone();
        let Ok(cache_key) = canonical_essl_cache_key(&vertex_source, &fragment_source) else {
            self.set_program_link_failure(program, "program link failed: interface validation");
            return;
        };
        let translated = match self.translated_programs.get(&cache_key) {
            Some(translated) => translated.clone(),
            None => {
                let Ok(translated) =
                    translate_canonical_essl_pair(&vertex_source, &fragment_source)
                else {
                    self.set_program_link_failure(
                        program,
                        "program link failed: translation rejected linked pair",
                    );
                    return;
                };
                self.translated_programs
                    .insert(cache_key, translated.clone());
                translated
            },
        };
        let reflection = translated.reflection.clone();
        let Some(program_object) = self.programs.get_mut(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        program_object.translated = Some(translated);
        program_object.uniform_block_bytes = vec![0u8; reflection.uniform_block_size as usize];
        // WebGL initializes sampler uniforms to texture unit 0. Keeping them
        // unset here makes helper programs that rely on the default sampler
        // value fail at draw time even though link succeeds.
        program_object.sampler_texture_units = vec![Some(0); reflection.samplers.len()];
        program_object.reflection = Some(reflection);
        program_object.pipelines.clear();
        program_object.link_status = true;
        program_object.info_log.clear();
    }

    pub fn get_program_link_status(&mut self, program: WebGlProgramId) -> bool {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return false;
        }
        let Some(program) = self.programs.get(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return false;
        };
        program.link_status
    }

    pub fn get_program_info_log(&mut self, program: WebGlProgramId) -> Option<String> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return None;
        }
        let Some(program) = self.programs.get(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        };
        Some(program.info_log.clone())
    }

    pub fn create_program_from_essl(
        &mut self,
        vertex_source: &str,
        fragment_source: &str,
    ) -> Option<WebGlProgramId> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return None;
        }
        let vertex_shader = self.create_shader(ShaderStage::Vertex);
        self.shader_source(vertex_shader, vertex_source);
        self.compile_shader(vertex_shader);
        if !self.get_shader_compile_status(vertex_shader) {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        }
        let fragment_shader = self.create_shader(ShaderStage::Fragment);
        self.shader_source(fragment_shader, fragment_source);
        self.compile_shader(fragment_shader);
        if !self.get_shader_compile_status(fragment_shader) {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        }
        let program = self.create_program();
        self.attach_shader(program, vertex_shader);
        self.attach_shader(program, fragment_shader);
        self.link_program(program);
        if self.get_program_link_status(program) {
            Some(program)
        } else {
            self.record_error(WebGlError::InvalidOperation);
            None
        }
    }

    pub fn use_program(&mut self, program: Option<WebGlProgramId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if let Some(id) = program {
            let Some(program) = self.programs.get(&id) else {
                self.record_error(WebGlError::InvalidOperation);
                return;
            };
            if !program.link_status {
                self.record_error(WebGlError::InvalidOperation);
                return;
            }
        }
        self.current_program = program;
    }

    pub fn get_attrib_location(&mut self, program: WebGlProgramId, name: &str) -> i32 {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return -1;
        }
        let Some(program) = self.programs.get(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return -1;
        };
        let Some(reflection) = program.reflection.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return -1;
        };
        reflection
            .attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .map_or(-1, |attribute| attribute.location as i32)
    }

    pub fn get_uniform_location(
        &mut self,
        program: WebGlProgramId,
        name: &str,
    ) -> Option<WebGlUniformLocation> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return None;
        }
        let Some(program_object) = self.programs.get(&program) else {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        };
        let Some(reflection) = program_object.reflection.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        };
        if let Some((index, _)) = reflection
            .uniforms
            .iter()
            .enumerate()
            .find(|(_, u)| u.name == name)
        {
            return Some(WebGlUniformLocation {
                program,
                slot: UniformSlot::BlockMember {
                    index: index as u32,
                },
            });
        }
        if let Some((index, _)) = reflection
            .samplers
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
        {
            return Some(WebGlUniformLocation {
                program,
                slot: UniformSlot::Sampler {
                    index: index as u32,
                },
            });
        }
        None
    }

    pub fn uniform1f(&mut self, location: WebGlUniformLocation, x: f32) {
        self.write_block_member(location, UniformKind::Float32, &x.to_ne_bytes());
    }

    pub fn uniform2f(&mut self, location: WebGlUniformLocation, x: f32, y: f32) {
        let bytes = [x.to_ne_bytes(), y.to_ne_bytes()].concat();
        self.write_block_member(location, UniformKind::Float32x2, &bytes);
    }

    pub fn uniform3f(&mut self, location: WebGlUniformLocation, x: f32, y: f32, z: f32) {
        let bytes = [x.to_ne_bytes(), y.to_ne_bytes(), z.to_ne_bytes()].concat();
        self.write_block_member(location, UniformKind::Float32x3, &bytes);
    }

    pub fn uniform4f(&mut self, location: WebGlUniformLocation, x: f32, y: f32, z: f32, w: f32) {
        let bytes = [
            x.to_ne_bytes(),
            y.to_ne_bytes(),
            z.to_ne_bytes(),
            w.to_ne_bytes(),
        ]
        .concat();
        self.write_block_member(location, UniformKind::Float32x4, &bytes);
    }

    pub fn uniform2fv(&mut self, location: WebGlUniformLocation, value: &[f32; 2]) {
        self.write_block_member(location, UniformKind::Float32x2, &f32_slice_to_bytes(value));
    }

    pub fn uniform3fv(&mut self, location: WebGlUniformLocation, value: &[f32; 3]) {
        self.write_block_member(location, UniformKind::Float32x3, &f32_slice_to_bytes(value));
    }

    pub fn uniform4fv(&mut self, location: WebGlUniformLocation, value: &[f32; 4]) {
        self.write_block_member(location, UniformKind::Float32x4, &f32_slice_to_bytes(value));
    }

    /// Column-major mat3 — WGSL pads each column to 16 bytes
    /// (vec3 occupies 12, pad up to 16). Caller passes 9 floats
    /// in column-major order; this writes 48 bytes with the
    /// padding inserted.
    pub fn uniform_matrix3fv(&mut self, location: WebGlUniformLocation, value: &[f32; 9]) {
        let mut padded = [0u8; 48];
        for column in 0..3 {
            for row in 0..3 {
                let src = value[column * 3 + row];
                let dst = column * 16 + row * 4;
                padded[dst..dst + 4].copy_from_slice(&src.to_ne_bytes());
            }
        }
        self.write_block_member(location, UniformKind::Matrix3, &padded);
    }

    /// Column-major mat4 — 4 columns × vec4 each, 64 bytes, no
    /// padding gymnastics. Caller passes 16 floats column-major.
    pub fn uniform_matrix4fv(&mut self, location: WebGlUniformLocation, value: &[f32; 16]) {
        self.write_block_member(location, UniformKind::Matrix4, &f32_slice_to_bytes(value));
    }

    fn write_block_member(
        &mut self,
        location: WebGlUniformLocation,
        expected_kind: UniformKind,
        bytes: &[u8],
    ) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if self.current_program != Some(location.program) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let UniformSlot::BlockMember { index } = location.slot else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(program) = self.programs.get_mut(&location.program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(reflection) = program.reflection.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(uniform) = reflection.uniforms.get(index as usize) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        if uniform.kind != expected_kind {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let offset = uniform.block_offset as usize;
        let end = offset + bytes.len();
        if end > program.uniform_block_bytes.len() {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        program.uniform_block_bytes[offset..end].copy_from_slice(bytes);
    }

    pub fn uniform1i(&mut self, location: WebGlUniformLocation, value: i32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if self.current_program != Some(location.program) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        if value < 0 || value as usize >= MAX_TEXTURE_IMAGE_UNITS {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        let UniformSlot::Sampler { index } = location.slot else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(program) = self.programs.get_mut(&location.program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(reflection) = program.reflection.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(sampler) = reflection.samplers.get(index as usize) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        if sampler.kind != UniformKind::Sampler2D && sampler.kind != UniformKind::SamplerCube {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(slot) = program.sampler_texture_units.get_mut(index as usize) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        *slot = Some(value as u32);
    }

    pub fn enable_vertex_attrib_array(&mut self, index: u32) {
        let Some(attrib) = self.attrib_mut(index) else {
            self.record_error(WebGlError::InvalidValue);
            return;
        };
        attrib.enabled = true;
    }

    pub fn vertex_attrib_pointer_f32(
        &mut self,
        index: u32,
        size: u32,
        normalized: bool,
        stride: u64,
        offset: u64,
    ) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if normalized || !matches!(size, 1 | 2 | 3 | 4) || stride % 4 != 0 || offset % 4 != 0 {
            self.record_error(WebGlError::InvalidValue);
            return;
        }
        let Some(bound) = self.bound_array_buffer else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(attrib) = self.attrib_mut(index) else {
            self.record_error(WebGlError::InvalidValue);
            return;
        };
        attrib.buffer = Some(bound);
        attrib.size = size;
        attrib.stride = stride;
        attrib.offset = offset;
    }
}
