/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::*;

pub(super) fn f32_slice_to_bytes(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

pub(super) fn u16_slice_to_bytes(values: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 2);
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}

fn vertex_format(kind: VertexAttributeKind) -> wgpu::VertexFormat {
    match kind {
        VertexAttributeKind::Float32 => wgpu::VertexFormat::Float32,
        VertexAttributeKind::Float32x2 => wgpu::VertexFormat::Float32x2,
        VertexAttributeKind::Float32x3 => wgpu::VertexFormat::Float32x3,
        VertexAttributeKind::Float32x4 => wgpu::VertexFormat::Float32x4,
    }
}

pub(super) fn vertex_stride(kind: VertexAttributeKind) -> u64 {
    match kind {
        VertexAttributeKind::Float32 => 4,
        VertexAttributeKind::Float32x2 => 8,
        VertexAttributeKind::Float32x3 => 12,
        VertexAttributeKind::Float32x4 => 16,
    }
}

pub(super) fn vertex_component_count(kind: VertexAttributeKind) -> u32 {
    match kind {
        VertexAttributeKind::Float32 => 1,
        VertexAttributeKind::Float32x2 => 2,
        VertexAttributeKind::Float32x3 => 3,
        VertexAttributeKind::Float32x4 => 4,
    }
}

pub(super) fn effective_vertex_stride(attrib: VertexAttribState, kind: VertexAttributeKind) -> u64 {
    if attrib.stride == 0 {
        vertex_stride(kind)
    } else {
        attrib.stride
    }
}

pub(super) fn build_render_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    translated: &TranslatedProgram,
    reflection: &ProgramReflection,
    pipeline_key: &VertexPipelineKey,
) -> PipelineObject {
    // Each attribute lives in its own VertexBufferLayout — one
    // wgpu vertex buffer slot per attribute, matching the WebGL
    // model where `vertexAttribPointer(i, ...)` configures
    // location `i` independently. The `wgpu::VertexAttribute`
    // values must outlive the pipeline build call, so we
    // materialize them into a Vec before borrowing.
    let attribute_descriptors: Vec<[wgpu::VertexAttribute; 1]> = reflection
        .attributes
        .iter()
        .zip(pipeline_key.attribute_layouts.iter())
        .map(|(attribute, layout)| {
            [wgpu::VertexAttribute {
                format: vertex_format(attribute.kind),
                offset: layout.offset,
                shader_location: attribute.location,
            }]
        })
        .collect();
    let vertex_buffer_layouts: Vec<wgpu::VertexBufferLayout> = attribute_descriptors
        .iter()
        .zip(pipeline_key.attribute_layouts.iter())
        .map(|(attrs, layout)| wgpu::VertexBufferLayout {
            array_stride: layout.stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: attrs,
        })
        .collect();

    let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("webgl-wgpu vertex shader"),
        source: wgpu::ShaderSource::Wgsl(translated.vertex_wgsl.clone().into()),
    });
    let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("webgl-wgpu fragment shader"),
        source: wgpu::ShaderSource::Wgsl(translated.fragment_wgsl.clone().into()),
    });

    // All resources go in a single `@group(0)` bind-group
    // layout matching webgl-essl's emission: Block uniform at
    // `@binding(0)`, then each sampler at `image_binding` +
    // `image_binding + 1`. The Block + samplers can coexist in
    // one bind group, so we no longer split uniform / texture
    // into two BGLs.
    let mut group_zero_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
    if reflection.uniform_block_size > 0 {
        group_zero_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
    }
    for sampler in &reflection.samplers {
        let view_dimension = match sampler.kind {
            crate::shader::UniformKind::SamplerCube => wgpu::TextureViewDimension::Cube,
            _ => wgpu::TextureViewDimension::D2,
        };
        group_zero_entries.push(wgpu::BindGroupLayoutEntry {
            binding: sampler.image_binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        });
        group_zero_entries.push(wgpu::BindGroupLayoutEntry {
            binding: sampler.sampler_binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    let group_zero_layout = if group_zero_entries.is_empty() {
        None
    } else {
        Some(
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("webgl-wgpu group(0) bind group layout"),
                entries: &group_zero_entries,
            }),
        )
    };
    let bind_group_layouts: Vec<Option<&wgpu::BindGroupLayout>> = group_zero_layout
        .as_ref()
        .map(|l| vec![Some(l)])
        .unwrap_or_default();
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("webgl-wgpu pipeline layout"),
        bind_group_layouts: &bind_group_layouts,
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("webgl-wgpu render pipeline"),
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
        group_zero_layout,
    }
}

/// Build the single `@group(0)` bind group for the current
/// draw. Pulls together (when present) the uniform Block buffer
/// + one (texture-view, sampler) pair per declared sampler.
/// Returns the bind group plus any resources it owns
/// (`wgpu::Sampler` handles need to outlive the draw call).
pub(super) struct GroupZero {
    pub(super) bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    pub(super) uniform_buffer: Option<wgpu::Buffer>,
    #[allow(dead_code)]
    pub(super) samplers: Vec<wgpu::Sampler>,
}

pub(super) fn build_group_zero_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_block_bytes: Option<&[u8]>,
    sampler_views: &[(u32, u32, &wgpu::TextureView)],
) -> GroupZero {
    let uniform_buffer = uniform_block_bytes.map(|bytes| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("webgl-wgpu uniform Block buffer"),
            size: bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        buffer
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytes);
        buffer.unmap();
        buffer
    });
    let samplers: Vec<wgpu::Sampler> = sampler_views
        .iter()
        .map(|_| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("webgl-wgpu fragment sampler"),
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        })
        .collect();
    let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();
    if let Some(buffer) = uniform_buffer.as_ref() {
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        });
    }
    for ((image_binding, sampler_binding, view), sampler) in
        sampler_views.iter().zip(samplers.iter())
    {
        entries.push(wgpu::BindGroupEntry {
            binding: *image_binding,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: *sampler_binding,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
    }
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("webgl-wgpu group(0) bind group"),
        layout,
        entries: &entries,
    });
    GroupZero {
        bind_group,
        uniform_buffer,
        samplers,
    }
}
