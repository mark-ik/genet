/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::shader::{
    ProgramCacheKey, ProgramReflection, TranslatedProgram, UniformKind, VertexAttributeKind,
    canonical_essl_cache_key, translate_canonical_essl_pair, validate_canonical_fragment_source,
    validate_canonical_vertex_source,
};
use crate::{WebGlCanvas, WebGlCanvasDescriptor, WebGlCanvasError};
use std::collections::HashMap;

const MAX_VERTEX_ATTRIBS: usize = 16;
const MAX_TEXTURE_IMAGE_UNITS: usize = 8;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlBufferId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlTextureId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlFramebufferId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlRenderbufferId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlShaderId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlProgramId(u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct WebGlUniformLocation {
    program: WebGlProgramId,
    slot: UniformSlot,
}

/// What the WebGL `getUniformLocation` call resolved to: an
/// index into either the program's Block-uniform list (for
/// `vec_n` / `mat_n` / scalars) or its sampler list (for
/// `sampler2D` / `samplerCube`). Setters dispatch on this tag
/// to write into the right CPU mirror.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum UniformSlot {
    /// `program.reflection.uniforms[index]`.
    BlockMember { index: u32 },
    /// `program.reflection.samplers[index]`.
    Sampler { index: u32 },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WebGlError {
    NoError,
    InvalidEnum,
    InvalidValue,
    InvalidOperation,
    InvalidFramebufferOperation,
    ContextLostWebgl,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BufferTarget {
    ArrayBuffer,
    ElementArrayBuffer,
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
pub enum IndexType {
    UnsignedShort,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WebGlFramebufferStatus {
    Complete,
    IncompleteMissingAttachment,
    IncompleteAttachment,
}

/// One face of a cube-map texture. Matches the WebGL
/// `TEXTURE_CUBE_MAP_POSITIVE_X` / ... `NEGATIVE_Z` set; the
/// numeric mapping is the wgpu layer index (0..6) the face
/// occupies inside a cube `wgpu::Texture`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum CubeFace {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}

impl CubeFace {
    pub(super) fn layer(self) -> u32 {
        match self {
            Self::PositiveX => 0,
            Self::NegativeX => 1,
            Self::PositiveY => 2,
            Self::NegativeY => 3,
            Self::PositiveZ => 4,
            Self::NegativeZ => 5,
        }
    }
}

/// WebGL `gl.depthFunc` comparison. Determines which incoming
/// fragments survive the depth test against the existing depth
/// buffer value.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum DepthFunc {
    Never,
    Less,
    Equal,
    LessOrEqual,
    Greater,
    NotEqual,
    GreaterOrEqual,
    Always,
}

impl DepthFunc {
    pub(super) fn to_wgpu(self) -> wgpu::CompareFunction {
        match self {
            Self::Never => wgpu::CompareFunction::Never,
            Self::Less => wgpu::CompareFunction::Less,
            Self::Equal => wgpu::CompareFunction::Equal,
            Self::LessOrEqual => wgpu::CompareFunction::LessEqual,
            Self::Greater => wgpu::CompareFunction::Greater,
            Self::NotEqual => wgpu::CompareFunction::NotEqual,
            Self::GreaterOrEqual => wgpu::CompareFunction::GreaterEqual,
            Self::Always => wgpu::CompareFunction::Always,
        }
    }
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
    index_u16: Option<Vec<u16>>,
}

struct TextureObject {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    kind: TextureKind,
    size: (u32, u32),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TextureKind {
    Texture2D,
    TextureCube,
}

struct RenderbufferObject {
    texture: wgpu::Texture,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

#[derive(Default)]
struct FramebufferObject {
    color_texture: Option<WebGlTextureId>,
    color_renderbuffer: Option<WebGlRenderbufferId>,
}

struct ShaderObject {
    stage: ShaderStage,
    source: String,
    compile_status: bool,
    info_log: String,
}

struct PipelineObject {
    pipeline: wgpu::RenderPipeline,
    /// Single bind-group layout for `@group(0)` — covers the
    /// uniform Block buffer (if any) plus every sampler. `None`
    /// when the shader pair declares no uniforms and no
    /// samplers.
    group_zero_layout: Option<wgpu::BindGroupLayout>,
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
struct AttributeBufferLayout {
    stride: u64,
    offset: u64,
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
struct VertexPipelineKey {
    /// One entry per declared vertex attribute, in declaration
    /// order. Each carries the stride / offset the WebGL caller
    /// configured via `vertexAttribPointer` — the pipeline is
    /// cached keyed on this tuple so stride changes (e.g.
    /// interleaved vs. tightly-packed) re-bake the pipeline.
    attribute_layouts: Vec<AttributeBufferLayout>,
    /// Depth-test state at draw time. `None` when depth test
    /// is disabled (no DepthStencilState attached); `Some` when
    /// enabled, carrying the comparison function. The cache key
    /// includes this so toggling depth state rebakes the
    /// pipeline.
    depth_state: Option<DepthFunc>,
    /// `gl.colorMask` packed into 4 bits at draw time (see
    /// [`color_mask_bits`]). Part of the cache key so changing
    /// the mask rebakes the pipeline with a new `write_mask`.
    color_write_mask: u8,
}

struct ProgramObject {
    attached_vertex_shader: Option<WebGlShaderId>,
    attached_fragment_shader: Option<WebGlShaderId>,
    translated: Option<TranslatedProgram>,
    reflection: Option<ProgramReflection>,
    pipelines: HashMap<VertexPipelineKey, PipelineObject>,
    /// CPU mirror of the uniform Block buffer. Sized to the
    /// program's `uniform_block_size`. Mutated by `uniformXXX`
    /// setters at the offsets the reflection records; uploaded
    /// to the GPU on each draw.
    uniform_block_bytes: Vec<u8>,
    /// Per-sampler texture-unit assignments set via
    /// `uniform1i` on the sampler's location. Indexed by
    /// sampler member index.
    sampler_texture_units: Vec<Option<u32>>,
    link_status: bool,
    info_log: String,
}

pub struct WebGlContext {
    canvas: WebGlCanvas,
    attributes: WebGlContextAttributes,
    buffers: HashMap<WebGlBufferId, BufferObject>,
    textures: HashMap<WebGlTextureId, TextureObject>,
    framebuffers: HashMap<WebGlFramebufferId, FramebufferObject>,
    renderbuffers: HashMap<WebGlRenderbufferId, RenderbufferObject>,
    shaders: HashMap<WebGlShaderId, ShaderObject>,
    programs: HashMap<WebGlProgramId, ProgramObject>,
    translated_programs: HashMap<ProgramCacheKey, TranslatedProgram>,
    attribs: [VertexAttribState; MAX_VERTEX_ATTRIBS],
    bound_array_buffer: Option<WebGlBufferId>,
    bound_element_array_buffer: Option<WebGlBufferId>,
    bound_texture_2d_units: [Option<WebGlTextureId>; MAX_TEXTURE_IMAGE_UNITS],
    bound_texture_cube_units: [Option<WebGlTextureId>; MAX_TEXTURE_IMAGE_UNITS],
    active_texture_unit: u32,
    bound_framebuffer: Option<WebGlFramebufferId>,
    bound_renderbuffer: Option<WebGlRenderbufferId>,
    current_program: Option<WebGlProgramId>,
    next_buffer_id: u64,
    next_texture_id: u64,
    next_framebuffer_id: u64,
    next_renderbuffer_id: u64,
    next_shader_id: u64,
    next_program_id: u64,
    viewport: [u32; 4],
    scissor_box: [u32; 4],
    scissor_test_enabled: bool,
    depth_test_enabled: bool,
    depth_func: DepthFunc,
    depth_clear_value: f32,
    /// `gl.colorMask` per-channel write enable (R, G, B, A). Default all
    /// true. Affects both `drawArrays`/`drawElements` (via the pipeline's
    /// color `write_mask`) and `clear` (a fully-masked clear short-circuits;
    /// a partial mask is emulated with a full-viewport quad so unmasked
    /// channels are preserved, since wgpu's `LoadOp::Clear` ignores masks).
    color_mask: [bool; 4],
    pending_error: WebGlError,
    lost: bool,
}

pub(super) const DEPTH_ATTACHMENT_FORMAT: wgpu::TextureFormat = crate::CANVAS_DEPTH_FORMAT;

/// Translate a `gl.colorMask` (R, G, B, A) into wgpu's per-channel
/// `ColorWrites` for a pipeline's color target.
pub(super) fn color_writes_from_mask(mask: [bool; 4]) -> wgpu::ColorWrites {
    let mut writes = wgpu::ColorWrites::empty();
    if mask[0] {
        writes |= wgpu::ColorWrites::RED;
    }
    if mask[1] {
        writes |= wgpu::ColorWrites::GREEN;
    }
    if mask[2] {
        writes |= wgpu::ColorWrites::BLUE;
    }
    if mask[3] {
        writes |= wgpu::ColorWrites::ALPHA;
    }
    writes
}

/// Pack a `gl.colorMask` into 4 bits (R=1, G=2, B=4, A=8) for use as a
/// pipeline-cache key component.
pub(super) fn color_mask_bits(mask: [bool; 4]) -> u8 {
    (mask[0] as u8) | (mask[1] as u8) << 1 | (mask[2] as u8) << 2 | (mask[3] as u8) << 3
}

/// Inverse of [`color_mask_bits`]: unpack 4 mask bits into (R, G, B, A).
pub(super) fn unpack_color_mask(bits: u8) -> [bool; 4] {
    [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0]
}

mod draw;
mod objects;
mod pipeline;
mod programs;
mod readback;
mod state;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_framebuffer;
#[cfg(test)]
mod tests_index_texture;
#[cfg(test)]
mod tests_variants;
#[cfg(test)]
mod tests_widened;

impl WebGlContext {
    pub fn from_canvas(canvas: WebGlCanvas) -> Self {
        Self::from_canvas_with_attributes(canvas, WebGlContextAttributes::default())
    }

    pub fn from_canvas_with_attributes(
        canvas: WebGlCanvas,
        attributes: WebGlContextAttributes,
    ) -> Self {
        let (width, height) = canvas.texture().size;
        Self {
            canvas,
            attributes,
            buffers: HashMap::new(),
            textures: HashMap::new(),
            framebuffers: HashMap::new(),
            renderbuffers: HashMap::new(),
            shaders: HashMap::new(),
            programs: HashMap::new(),
            translated_programs: HashMap::new(),
            attribs: [VertexAttribState::default(); MAX_VERTEX_ATTRIBS],
            bound_array_buffer: None,
            bound_element_array_buffer: None,
            bound_texture_2d_units: [None; MAX_TEXTURE_IMAGE_UNITS],
            bound_texture_cube_units: [None; MAX_TEXTURE_IMAGE_UNITS],
            active_texture_unit: 0,
            bound_framebuffer: None,
            bound_renderbuffer: None,
            current_program: None,
            next_buffer_id: 1,
            next_texture_id: 1,
            next_framebuffer_id: 1,
            next_renderbuffer_id: 1,
            next_shader_id: 1,
            next_program_id: 1,
            viewport: [0, 0, width, height],
            scissor_box: [0, 0, width, height],
            scissor_test_enabled: false,
            depth_test_enabled: false,
            depth_func: DepthFunc::Less,
            depth_clear_value: 1.0,
            color_mask: [true; 4],
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
        self.canvas.resize(width, height)?;
        self.viewport = [0, 0, width, height];
        self.scissor_box = [0, 0, width, height];
        Ok(())
    }

    pub fn clear(&mut self, color: wgpu::Color) {
        if self.lost {
            self.record_error(WebGlError::ContextLostWebgl);
            return;
        }
        if self.current_framebuffer_status() != WebGlFramebufferStatus::Complete {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        }
        let Some((view, format, _)) = self.current_color_target_view() else {
            self.record_error(WebGlError::InvalidFramebufferOperation);
            return;
        };
        // A partial color mask can't go through `LoadOp::Clear` (wgpu clears
        // all channels regardless of mask). Emulate it with a full-viewport
        // quad that writes the clear color through the masked channels only,
        // preserving the rest.
        if self.color_mask != [true; 4] {
            self.masked_clear(&view, format, color);
            if self.bound_framebuffer.is_none() {
                self.canvas.output.damage =
                    Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
            }
            return;
        }
        let mut encoder =
            self.canvas
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("webgl-wgpu context clear encoder"),
                });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgl-wgpu context clear"),
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
        self.canvas.queue.submit([encoder.finish()]);
        if self.bound_framebuffer.is_none() {
            self.canvas.output.damage =
                Some([0, 0, self.canvas.output.size.0, self.canvas.output.size.1]);
        }
    }

    /// Emulate a color-masked `clear` by drawing a full-viewport solid-color
    /// quad with the pipeline `write_mask` set to the enabled channels and
    /// `LoadOp::Load` (preserve). The clear color rides in a uniform buffer.
    /// Used only when `color_mask` is partial; an all-true mask takes the
    /// faster `LoadOp::Clear` path.
    fn masked_clear(
        &mut self,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        color: wgpu::Color,
    ) {
        let device = &self.canvas.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("webgl-wgpu masked-clear shader"),
            source: wgpu::ShaderSource::Wgsl(MASKED_CLEAR_WGSL.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("webgl-wgpu masked-clear bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("webgl-wgpu masked-clear layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("webgl-wgpu masked-clear pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: color_writes_from_mask(self.color_mask),
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
        let color_bytes: [f32; 4] = [
            color.r as f32,
            color.g as f32,
            color.b as f32,
            color.a as f32,
        ];
        let mut raw = Vec::with_capacity(16);
        for c in color_bytes {
            raw.extend_from_slice(&c.to_ne_bytes());
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgl-wgpu masked-clear color"),
            size: raw.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        buffer
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(&raw);
        buffer.unmap();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("webgl-wgpu masked-clear bind group"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let mut encoder =
            self.canvas
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("webgl-wgpu masked-clear encoder"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("webgl-wgpu masked-clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.canvas.queue.submit([encoder.finish()]);
    }
}

/// Full-viewport solid-color shader for masked clears. The vertex stage emits
/// a single oversized triangle covering the viewport; the fragment stage
/// outputs the uniform clear color (the pipeline's `write_mask` restricts it
/// to the enabled channels).
const MASKED_CLEAR_WGSL: &str = r#"
@group(0) @binding(0) var<uniform> clear_color: vec4<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    return vec4<f32>(p[vi], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return clear_color;
}
"#;
