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
        VertexAttributeKind::Float32x2 => wgpu::VertexFormat::Float32x2,
        VertexAttributeKind::Float32x4 => wgpu::VertexFormat::Float32x4,
    }
}

pub(super) fn vertex_stride(kind: VertexAttributeKind) -> u64 {
    match kind {
        VertexAttributeKind::Float32x2 => 8,
        VertexAttributeKind::Float32x4 => 16,
    }
}

pub(super) fn vertex_component_count(kind: VertexAttributeKind) -> u32 {
    match kind {
        VertexAttributeKind::Float32x2 => 2,
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
    let texcoord_attrs = reflection.texcoord_attribute.as_ref().map(|attribute| {
        [wgpu::VertexAttribute {
            format: vertex_format(attribute.kind),
            offset: pipeline_key.texcoord_offset.expect("texcoord offset"),
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
    if let (Some(texcoord_stride), Some(texcoord_attrs)) =
        (pipeline_key.texcoord_stride, texcoord_attrs.as_ref())
    {
        vertex_buffer_layouts.push(wgpu::VertexBufferLayout {
            array_stride: texcoord_stride,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: texcoord_attrs,
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
    let texture_bind_group_layout = reflection.fragment_texture_uniform.as_ref().map(|uniform| {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("webgl-wgpu texture bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: uniform.binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: uniform.binding + 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    });
    let bind_group_layouts = uniform_bind_group_layout
        .iter()
        .chain(texture_bind_group_layout.iter())
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
        texture_bind_group_layout,
    }
}

pub(super) fn build_uniform_bind_group(
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

pub(super) fn build_texture_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    image_binding: u32,
) -> (wgpu::Sampler, wgpu::BindGroup) {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("webgl-wgpu fragment texture sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("webgl-wgpu fragment texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: image_binding,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: image_binding + 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (sampler, bind_group)
}
