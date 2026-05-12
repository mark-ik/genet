/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::sync::mpsc;

use crate::shader::{
    ProgramCacheKey, ProgramReflection, TranslatedProgram, UniformKind, VertexAttributeKind,
    canonical_essl_cache_key, translate_canonical_essl_pair,
};
use crate::{WebGlCanvas, WebGlCanvasDescriptor, WebGlCanvasError};

const MAX_VERTEX_ATTRIBS: usize = 16;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlBufferId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlProgramId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlUniformLocation {
    program: WebGlProgramId,
    binding: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WebGlError {
    NoError,
    InvalidEnum,
    InvalidValue,
    InvalidOperation,
    ContextLostWebgl,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BufferTarget {
    ArrayBuffer,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BufferUsage {
    StaticDraw,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PrimitiveMode {
    Triangles,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct WebGlContextAttributes {
    pub alpha: bool,
    pub depth: bool,
    pub stencil: bool,
    pub antialias: bool,
    pub premultiplied_alpha: bool,
    pub preserve_drawing_buffer: bool,
}

impl Default for WebGlContextAttributes {
    fn default() -> Self {
        Self {
            alpha: true,
            depth: false,
            stencil: false,
            antialias: false,
            premultiplied_alpha: true,
            preserve_drawing_buffer: false,
        }
    }
}

#[derive(Clone, Copy)]
struct VertexAttribState {
    enabled: bool,
    buffer: Option<WebGlBufferId>,
    size: u32,
    stride: u64,
    offset: u64,
}

impl Default for VertexAttribState {
    fn default() -> Self {
        Self {
            enabled: false,
            buffer: None,
            size: 4,
            stride: 0,
            offset: 0,
        }
    }
}

struct BufferObject {
    buffer: wgpu::Buffer,
    byte_len: u64,
}

struct PipelineObject {
    pipeline: wgpu::RenderPipeline,
    uniform_bind_group_layout: Option<wgpu::BindGroupLayout>,
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
struct VertexPipelineKey {
    position_stride: u64,
    position_offset: u64,
    color_stride: Option<u64>,
    color_offset: Option<u64>,
}

struct ProgramObject {
    translated: TranslatedProgram,
    reflection: ProgramReflection,
    pipelines: HashMap<VertexPipelineKey, PipelineObject>,
    fragment_color_uniform: Option<[f32; 4]>,
}

pub struct WebGlContext {
    canvas: WebGlCanvas,
    attributes: WebGlContextAttributes,
    buffers: HashMap<WebGlBufferId, BufferObject>,
    programs: HashMap<WebGlProgramId, ProgramObject>,
    translated_programs: HashMap<ProgramCacheKey, TranslatedProgram>,
    attribs: [VertexAttribState; MAX_VERTEX_ATTRIBS],
    bound_array_buffer: Option<WebGlBufferId>,
    current_program: Option<WebGlProgramId>,
    next_buffer_id: u64,
    next_program_id: u64,
    pending_error: WebGlError,
    lost: bool,
}

impl WebGlContext {
    pub fn from_canvas(canvas: WebGlCanvas) -> Self {
        Self::from_canvas_with_attributes(canvas, WebGlContextAttributes::default())
    }

    pub fn from_canvas_with_attributes(
        canvas: WebGlCanvas,
        attributes: WebGlContextAttributes,
    ) -> Self {
        Self {
            canvas,
            attributes,
            buffers: HashMap::new(),
            programs: HashMap::new(),
            translated_programs: HashMap::new(),
            attribs: [VertexAttribState::default(); MAX_VERTEX_ATTRIBS],
            bound_array_buffer: None,
            current_program: None,
            next_buffer_id: 1,
            next_program_id: 1,
            pending_error: WebGlError::NoError,
            lost: false,
        }
    }

    pub fn from_wgpu_handles(
        device: wgpu::Device,
        queue: wgpu::Queue,
        descriptor: WebGlCanvasDescriptor,
    ) -> Result<Self, WebGlCanvasError> {
        Ok(Self::from_canvas(WebGlCanvas::from_wgpu_handles(
            device, queue, descriptor,
        )?))
    }

    pub fn canvas(&self) -> &WebGlCanvas {
        &self.canvas
    }

    pub fn context_attributes(&self) -> WebGlContextAttributes {
        self.attributes
    }

    pub fn texture(&self) -> &crate::WebGlCanvasTexture {
        self.canvas.texture()
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), WebGlCanvasError> {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return Ok(());
        }
        self.canvas.resize(width, height)
    }

    pub fn clear(&mut self, color: wgpu::Color) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        self.canvas.clear(color);
    }

    pub fn create_buffer(&mut self) -> WebGlBufferId {
        let id = WebGlBufferId(self.next_buffer_id);
        self.next_buffer_id += 1;
        id
    }

    pub fn bind_buffer(&mut self, target: BufferTarget, buffer: Option<WebGlBufferId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        match target {
            BufferTarget::ArrayBuffer => self.bound_array_buffer = buffer,
        }
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
            },
        );
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
        let Ok(cache_key) = canonical_essl_cache_key(vertex_source, fragment_source) else {
            self.record_error(WebGlError::InvalidOperation);
            return None;
        };
        let translated = match self.translated_programs.get(&cache_key) {
            Some(translated) => translated.clone(),
            None => {
                let Ok(translated) = translate_canonical_essl_pair(vertex_source, fragment_source)
                else {
                    self.record_error(WebGlError::InvalidOperation);
                    return None;
                };
                self.translated_programs
                    .insert(cache_key, translated.clone());
                translated
            },
        };
        let reflection = translated.reflection.clone();
        let id = WebGlProgramId(self.next_program_id);
        self.next_program_id += 1;
        self.programs.insert(
            id,
            ProgramObject {
                translated,
                fragment_color_uniform: reflection
                    .fragment_color_uniform
                    .as_ref()
                    .map(|_| [0.0, 0.0, 0.0, 1.0]),
                reflection,
                pipelines: HashMap::new(),
            },
        );
        Some(id)
    }

    pub fn use_program(&mut self, program: Option<WebGlProgramId>) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if program.is_some_and(|id| !self.programs.contains_key(&id)) {
            self.record_error(WebGlError::InvalidOperation);
            return;
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
        if program.reflection.position_attribute.name == name {
            program.reflection.position_attribute.location as i32
        } else if let Some(color_attribute) = program.reflection.color_attribute.as_ref() {
            if color_attribute.name == name {
                color_attribute.location as i32
            } else {
                -1
            }
        } else {
            -1
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
        let Some(uniform) = program_object.reflection.fragment_color_uniform.as_ref() else {
            return None;
        };
        if uniform.name == name {
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
        let Some(uniform) = program.reflection.fragment_color_uniform.as_ref() else {
            self.record_error(WebGlError::InvalidOperation);
            return;
        };
        if uniform.binding != location.binding || uniform.kind != UniformKind::Float32x4 {
            self.record_error(WebGlError::InvalidOperation);
            return;
        }
        program.fragment_color_uniform = Some([x, y, z, w]);
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
        let uniform_value = program.fragment_color_uniform;
        let reflection = program.reflection.clone();
        let translated = program.translated.clone();
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
        };
        let needs_pipeline = self.programs.get(&program_id).map_or(true, |program| {
            !program.pipelines.contains_key(&pipeline_key)
        });
        if needs_pipeline {
            let pipeline = build_render_pipeline(
                &self.canvas.device,
                self.canvas.output.format,
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

        let view = self.canvas.output.create_view();
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
            pass.set_vertex_buffer(0, position_buffer.buffer.slice(..));
            if let Some((_, _, color_buffer)) = color_attribute {
                pass.set_vertex_buffer(1, color_buffer.buffer.slice(..));
            }
            pass.draw(first..first + count, 0..1);
        }
        self.canvas.queue.submit([encoder.finish()]);
        self.canvas.output.damage =
            Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
    }

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
        let (canvas_width, canvas_height) = self.canvas.output.size;
        if width == 0 || height == 0 || x + width > canvas_width || y + height > canvas_height {
            self.record_error(WebGlError::InvalidValue);
            return Ok(Vec::new());
        }
        Ok(read_texture_rect_rgba8(
            &self.canvas.device,
            &self.canvas.queue,
            &self.canvas.output.texture,
            x,
            y,
            width,
            height,
        ))
    }

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
        self.programs.clear();
        self.translated_programs.clear();
        self.attribs = [VertexAttribState::default(); MAX_VERTEX_ATTRIBS];
        self.bound_array_buffer = None;
        self.current_program = None;
        self.lost = false;
        Ok(())
    }

    fn bound_buffer_for_target(&self, target: BufferTarget) -> Option<WebGlBufferId> {
        match target {
            BufferTarget::ArrayBuffer => self.bound_array_buffer,
        }
    }

    fn attrib_mut(&mut self, index: u32) -> Option<&mut VertexAttribState> {
        self.attribs.get_mut(index as usize)
    }

    fn record_error(&mut self, error: WebGlError) {
        if self.pending_error == WebGlError::NoError {
            self.pending_error = error;
        }
    }
}

fn f32_slice_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn vertex_format(kind: VertexAttributeKind) -> wgpu::VertexFormat {
    match kind {
        VertexAttributeKind::Float32x2 => wgpu::VertexFormat::Float32x2,
        VertexAttributeKind::Float32x4 => wgpu::VertexFormat::Float32x4,
    }
}

fn vertex_stride(kind: VertexAttributeKind) -> u64 {
    match kind {
        VertexAttributeKind::Float32x2 => 8,
        VertexAttributeKind::Float32x4 => 16,
    }
}

fn vertex_component_count(kind: VertexAttributeKind) -> u32 {
    match kind {
        VertexAttributeKind::Float32x2 => 2,
        VertexAttributeKind::Float32x4 => 4,
    }
}

fn effective_vertex_stride(attrib: VertexAttribState, kind: VertexAttributeKind) -> u64 {
    if attrib.stride == 0 {
        vertex_stride(kind)
    } else {
        attrib.stride
    }
}

fn build_render_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    translated: &TranslatedProgram,
    reflection: &ProgramReflection,
    pipeline_key: VertexPipelineKey,
) -> PipelineObject {
    let position_attribute = &reflection.position_attribute;
    let position_attrs = [wgpu::VertexAttribute {
        format: vertex_format(position_attribute.kind),
        offset: pipeline_key.position_offset,
        shader_location: position_attribute.location,
    }];
    let color_attrs = reflection.color_attribute.as_ref().map(|attribute| {
        [wgpu::VertexAttribute {
            format: vertex_format(attribute.kind),
            offset: pipeline_key.color_offset.expect("color offset"),
            shader_location: attribute.location,
        }]
    });
    let mut vertex_buffer_layouts = vec![wgpu::VertexBufferLayout {
        array_stride: pipeline_key.position_stride,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &position_attrs,
    }];
    if let (Some(color_stride), Some(color_attrs)) =
        (pipeline_key.color_stride, color_attrs.as_ref())
    {
        vertex_buffer_layouts.push(wgpu::VertexBufferLayout {
            array_stride: color_stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: color_attrs,
        });
    }
    let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("webgl-wgpu canonical triangle vertex shader"),
        source: wgpu::ShaderSource::Wgsl(translated.vertex_wgsl.clone().into()),
    });
    let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("webgl-wgpu canonical triangle fragment shader"),
        source: wgpu::ShaderSource::Wgsl(translated.fragment_wgsl.clone().into()),
    });
    let uniform_bind_group_layout = reflection.fragment_color_uniform.as_ref().map(|uniform| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("webgl-wgpu uniform bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: uniform.binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        })
    });
    let bind_group_layouts = uniform_bind_group_layout
        .iter()
        .map(Some)
        .collect::<Vec<_>>();
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("webgl-wgpu pipeline layout"),
        bind_group_layouts: &bind_group_layouts,
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("webgl-wgpu canonical triangle pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &vertex_shader,
            entry_point: Some("main"),
            buffers: &vertex_buffer_layouts,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &fragment_shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    PipelineObject {
        pipeline,
        uniform_bind_group_layout,
    }
}

fn build_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    value: [f32; 4],
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let bytes = f32_slice_to_bytes(&value);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("webgl-wgpu fragment color uniform buffer"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(&bytes);
    buffer.unmap();
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("webgl-wgpu fragment color uniform bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    (buffer, bind_group)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CANONICAL_TRIANGLE_FRAGMENT_SHADER, CANONICAL_TRIANGLE_VERTEX_SHADER};

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

    fn make_context(width: u32, height: u32) -> WebGlContext {
        let (device, queue) = make_device();
        WebGlContext::from_wgpu_handles(device, queue, WebGlCanvasDescriptor::new(width, height))
            .expect("context")
    }

    #[test]
    fn webgl_context_clear_triangle_read_pixels_and_get_error() {
        let mut context = make_context(32, 32);
        assert_eq!(
            context.context_attributes(),
            WebGlContextAttributes::default()
        );
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });

        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
        assert_eq!(context.get_error(), WebGlError::InvalidOperation);
        assert_eq!(context.get_error(), WebGlError::NoError);

        let program = context
            .create_program_from_essl(
                CANONICAL_TRIANGLE_VERTEX_SHADER,
                CANONICAL_TRIANGLE_FRAGMENT_SHADER,
            )
            .expect("canonical program");
        let position_location = context.get_attrib_location(program, "a_position");
        assert_eq!(position_location, 0);
        assert_eq!(context.get_attrib_location(program, "missing"), -1);
        context.use_program(Some(program));

        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(position_location as u32);
        context.vertex_attrib_pointer_f32(position_location as u32, 2, false, 0, 0);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);
        assert_eq!(context.get_error(), WebGlError::NoError);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 255, 0, 255]);
        let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
        assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn webgl_context_rejects_noncanonical_shader_pair() {
        let mut context = make_context(4, 4);
        let program = context.create_program_from_essl(
            "attribute vec2 a_position; void main() { gl_Position = vec4(0.0); }",
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        );
        assert!(program.is_none());
        assert_eq!(context.get_error(), WebGlError::InvalidOperation);
    }

    #[test]
    fn webgl_context_get_attrib_location_reports_invalid_program() {
        let mut context = make_context(4, 4);

        assert_eq!(
            context.get_attrib_location(WebGlProgramId(99), "a_position"),
            -1
        );
        assert_eq!(context.get_error(), WebGlError::InvalidOperation);
    }

    #[test]
    fn webgl_context_caches_canonical_shader_translation() {
        let mut context = make_context(4, 4);
        let reformatted_fragment = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
            }
        "#;

        let first = context
            .create_program_from_essl(
                CANONICAL_TRIANGLE_VERTEX_SHADER,
                CANONICAL_TRIANGLE_FRAGMENT_SHADER,
            )
            .expect("first canonical program");
        let second = context
            .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, reformatted_fragment)
            .expect("second canonical program");

        assert_ne!(first, second);
        assert_eq!(context.translated_programs.len(), 1);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    #[test]
    fn webgl_context_draws_literal_fragment_color() {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let blue_fragment = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(0.0, 0.0, 1.0, 1.0);
            }
        "#;

        let program = context
            .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, blue_fragment)
            .expect("literal color program");
        context.use_program(Some(program));
        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(0);
        context.vertex_attrib_pointer_f32(0, 2, false, 0, 0);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 0, 255, 255]);
        let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
        assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    #[test]
    fn webgl_context_draws_uniform_fragment_color() {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let uniform_fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() {
                gl_FragColor = u_color;
            }
        "#;

        let program = context
            .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, uniform_fragment)
            .expect("uniform color program");
        context.use_program(Some(program));
        let location = context
            .get_uniform_location(program, "u_color")
            .expect("uniform location");
        assert!(context.get_uniform_location(program, "missing").is_none());
        context.uniform4f(location, 0.0, 0.0, 1.0, 1.0);

        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
            BufferUsage::StaticDraw,
        );
        let position_location = context.get_attrib_location(program, "a_position") as u32;
        context.enable_vertex_attrib_array(position_location);
        context.vertex_attrib_pointer_f32(position_location, 2, false, 0, 0);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 0, 255, 255]);
        let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
        assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    #[test]
    fn webgl_context_draws_varying_vertex_color() {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let vertex = r#"
            attribute vec2 a_position;
            attribute vec4 a_color;
            varying vec4 v_color;
            void main() {
                v_color = a_color;
                gl_Position = vec4(a_position, 0.0, 1.0);
            }
        "#;
        let fragment = r#"
            precision mediump float;
            varying vec4 v_color;
            void main() {
                gl_FragColor = v_color;
            }
        "#;

        let program = context
            .create_program_from_essl(vertex, fragment)
            .expect("varying color program");
        let position_location = context.get_attrib_location(program, "a_position");
        let color_location = context.get_attrib_location(program, "a_color");
        assert_eq!(position_location, 0);
        assert_eq!(color_location, 1);
        context.use_program(Some(program));

        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[
                -0.8, -0.8, 0.0, 0.0, 1.0, 1.0, 0.8, -0.8, 0.0, 0.0, 1.0, 1.0, 0.0, 0.8, 0.0, 0.0,
                1.0, 1.0,
            ],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(position_location as u32);
        context.vertex_attrib_pointer_f32(position_location as u32, 2, false, 24, 0);
        context.enable_vertex_attrib_array(color_location as u32);
        context.vertex_attrib_pointer_f32(color_location as u32, 4, false, 24, 8);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 0, 255, 255]);
        let corner = context.read_pixels(0, 0, 1, 1).expect("corner read");
        assert_eq!(&corner[0..4], &[255, 0, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    #[test]
    fn webgl_context_uniform4f_requires_current_program() {
        let mut context = make_context(4, 4);
        let uniform_fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() { gl_FragColor = u_color; }
        "#;
        let program = context
            .create_program_from_essl(CANONICAL_TRIANGLE_VERTEX_SHADER, uniform_fragment)
            .expect("uniform color program");
        let location = context
            .get_uniform_location(program, "u_color")
            .expect("uniform location");

        context.uniform4f(location, 0.0, 0.0, 1.0, 1.0);

        assert_eq!(context.get_error(), WebGlError::InvalidOperation);
    }

    #[test]
    fn webgl_context_draws_with_renamed_position_attribute() {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let vertex = r#"
            attribute vec2 position;
            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
            }
        "#;

        let program = context
            .create_program_from_essl(vertex, CANONICAL_TRIANGLE_FRAGMENT_SHADER)
            .expect("renamed attribute program");
        let position_location = context.get_attrib_location(program, "position");
        assert_eq!(position_location, 0);
        assert_eq!(context.get_attrib_location(program, "a_position"), -1);
        context.use_program(Some(program));
        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
            BufferUsage::StaticDraw,
        );
        context.enable_vertex_attrib_array(position_location as u32);
        context.vertex_attrib_pointer_f32(position_location as u32, 2, false, 0, 0);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 255, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }

    #[test]
    fn webgl_context_draws_interleaved_vertex_stride() {
        let mut context = make_context(32, 32);
        context.clear(wgpu::Color {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let program = context
            .create_program_from_essl(
                CANONICAL_TRIANGLE_VERTEX_SHADER,
                CANONICAL_TRIANGLE_FRAGMENT_SHADER,
            )
            .expect("canonical program");
        context.use_program(Some(program));

        let vertices = context.create_buffer();
        context.bind_buffer(BufferTarget::ArrayBuffer, Some(vertices));
        context.buffer_data_f32(
            BufferTarget::ArrayBuffer,
            &[
                -0.8, -0.8, 9.0, 9.0, 0.8, -0.8, 9.0, 9.0, 0.0, 0.8, 9.0, 9.0,
            ],
            BufferUsage::StaticDraw,
        );
        let position_location = context.get_attrib_location(program, "a_position") as u32;
        context.enable_vertex_attrib_array(position_location);
        context.vertex_attrib_pointer_f32(position_location, 2, false, 16, 0);
        context.draw_arrays(PrimitiveMode::Triangles, 0, 3);

        let center = context.read_pixels(16, 16, 1, 1).expect("center read");
        assert_eq!(&center[0..4], &[0, 255, 0, 255]);
        assert_eq!(context.get_error(), WebGlError::NoError);
        assert_eq!(
            context
                .programs
                .get(&program)
                .expect("program")
                .pipelines
                .len(),
            1
        );
    }

    #[test]
    fn webgl_context_loss_and_restore_recreate_default_framebuffer() {
        let mut context = make_context(4, 4);
        let initial_generation = context.texture().generation;
        context.lose_context();
        context.clear(wgpu::Color::BLACK);
        assert_eq!(context.get_error(), WebGlError::ContextLostWebgl);

        context.restore_context().expect("restore");
        assert_eq!(context.texture().generation, initial_generation + 1);
        context.clear(wgpu::Color::BLACK);
        assert_eq!(context.get_error(), WebGlError::NoError);
    }
}
