/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

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
        shader.source = source.to_string();
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
                fragment_color_uniform: None,
                fragment_texture_unit: None,
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
        program_object.fragment_color_uniform = reflection
            .fragment_color_uniform
            .as_ref()
            .map(|_| [0.0, 0.0, 0.0, 1.0]);
        program_object.fragment_texture_unit =
            reflection.fragment_texture_uniform.as_ref().map(|_| 0);
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
        if reflection.position_attribute.name == name {
            reflection.position_attribute.location as i32
        } else if let Some(color_attribute) = reflection.color_attribute.as_ref() {
            if color_attribute.name == name {
                color_attribute.location as i32
            } else {
                reflection
                    .texcoord_attribute
                    .as_ref()
                    .filter(|attribute| attribute.name == name)
                    .map_or(-1, |attribute| attribute.location as i32)
            }
        } else {
            reflection
                .texcoord_attribute
                .as_ref()
                .filter(|attribute| attribute.name == name)
                .map_or(-1, |attribute| attribute.location as i32)
        }
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
        let uniform = reflection
            .fragment_color_uniform
            .as_ref()
            .or(reflection.fragment_texture_uniform.as_ref());
        if let Some(uniform) = uniform.filter(|uniform| uniform.name == name) {
            Some(WebGlUniformLocation {
                program,
                binding: uniform.binding,
            })
        } else {
            None
        }
    }

    pub fn uniform4f(&mut self, location: WebGlUniformLocation, x: f32, y: f32, z: f32, w: f32) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if self.current_program != Some(location.program) {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        let Some(program) = self.programs.get_mut(&location.program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(uniform) = program
            .reflection
            .as_ref()
            .and_then(|reflection| reflection.fragment_color_uniform.as_ref())
        else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        if uniform.binding != location.binding || uniform.kind != UniformKind::Float32x4 {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        program.fragment_color_uniform = Some([x, y, z, w]);
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
        let Some(program) = self.programs.get_mut(&location.program) else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        let Some(uniform) = program
            .reflection
            .as_ref()
            .and_then(|reflection| reflection.fragment_texture_uniform.as_ref())
        else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        if uniform.binding != location.binding || uniform.kind != UniformKind::Sampler2D {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        program.fragment_texture_unit = Some(value as u32);
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
        if normalized || !matches!(size, 2 | 4) || stride % 4 != 0 || offset % 4 != 0 {
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
