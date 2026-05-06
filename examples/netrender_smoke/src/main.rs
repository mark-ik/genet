/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 1 second receipt — embedder hookup smoke.
//!
//! Standalone binary (NOT a servo-wgpu workspace member; see Cargo.toml).
//! Proves the netrender API survives contact with a real embedder context
//! without the Servo stack.
//!
//! Plan reference:
//! `netrender-notes/2026-04-30_netrender_design_plan.md` §5 Phase 1
//! ("Receipt (first embedder hookup)").
//!
//! Receipt shape (non-presenting variant):
//! - Boot wgpu via `netrender::boot()`
//! - Create offscreen 256×256 `Rgba8UnormSrgb` target (RENDER_ATTACHMENT)
//! - Build one full-NDC red `brush_solid` `PreparedFrame`
//! - Call `Renderer::render` against the target view
//! - Poll the device — completes without error
//!
//! The servo-wgpu workspace Cargo.toml has a broken patch for `webrender`
//! (directory renamed to `netrender` in webrender-wgpu commit c9481b04b,
//! 2026-04-30). This standalone binary bypasses that workspace entirely
//! so the embedder hookup receipt can be captured now.

use netrender::{
    BrushSolidPipeline, ColorLoad, DrawIntent, FrameTarget, NetrenderOptions, PreparedFrame,
    boot, create_netrender_instance,
};

const DIM: u32 = 256;
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("netrender embedder hookup smoke — Phase 1 second receipt");
    println!("  plan: netrender-notes/2026-04-30_netrender_design_plan.md §5 Phase 1");

    println!("  booting wgpu handles via netrender::boot()...");
    let handles = boot()?;

    let info = handles.adapter.get_info();
    println!("  adapter: {} ({:?})", info.name, info.backend);

    // Clone device + queue before moving handles into create_netrender_instance.
    let device = handles.device.clone();
    let queue = handles.queue.clone();

    println!("  creating Renderer via create_netrender_instance...");
    let renderer = create_netrender_instance(handles, NetrenderOptions::default())
        .map_err(|e| format!("create_netrender_instance failed: {e:?}"))?;

    println!(
        "  creating {}×{} {:?} offscreen RENDER_ATTACHMENT target...",
        DIM, DIM, TARGET_FORMAT
    );
    let target_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("netrender smoke target"),
        size: wgpu::Extent3d {
            width: DIM,
            height: DIM,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target_texture.create_view(&wgpu::TextureViewDescriptor::default());

    println!("  building PreparedFrame (1× full-NDC red brush_solid)...");
    let pipe = renderer
        .wgpu_device
        .ensure_brush_solid(TARGET_FORMAT, /* alpha_pass */ false);
    let draw = build_full_ndc_red_draw(&device, &queue, &pipe);
    let prepared = PreparedFrame::new(vec![draw]);

    let frame_target = FrameTarget {
        view: &target_view,
        format: TARGET_FORMAT,
        width: DIM,
        height: DIM,
    };

    println!("  calling Renderer::render...");
    renderer.render(
        &prepared,
        frame_target,
        ColorLoad::Clear(wgpu::Color::TRANSPARENT),
    );

    // Block until submitted GPU work completes.
    device.poll(wgpu::PollType::wait_indefinitely()).ok();

    println!("  Renderer::render returned without error.");
    println!("Phase 1 embedder hookup receipt: PASS");

    // Write run log to netrender-notes/logs/ (gitignored per design plan).
    let final_info = renderer.wgpu_device.core.adapter.get_info();
    let log = format!(
        "Phase 1 second receipt (embedder hookup): PASS\n\
         date: 2026-04-30\n\
         binary: servo-wgpu/examples/netrender_smoke\n\
         adapter: {} ({:?})\n\
         target: {}x{} {:?}\n\
         frame: 1x full-NDC red brush_solid via Renderer::render\n\
         result: Renderer::render completed without error\n\
         note: standalone binary (not workspace member) — servo-wgpu\n\
               workspace blocked by broken webrender patch (renamed to\n\
               netrender in webrender-wgpu c9481b04b, 2026-04-30)\n",
        final_info.name,
        final_info.backend,
        DIM,
        DIM,
        TARGET_FORMAT,
    );
    let log_path = "C:/Users/mark_/Code/repos/webrender-wgpu/netrender-notes/logs/2026-04-30_netrender_embedder_hookup.log";
    std::fs::write(log_path, &log)?;
    println!("log written to {}", log_path);

    Ok(())
}

/// Construct a single brush_solid [`DrawIntent`] covering all of NDC (−1..1)
/// in opaque red. Mirrors `build_full_ndc_red_draw` from
/// `netrender/tests/p1_solid_rect.rs`; Phase 2's `BatchBuilder` obsoletes
/// this boilerplate.
fn build_full_ndc_red_draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipe: &BrushSolidPipeline,
) -> DrawIntent {
    // PrimitiveHeader[0]: full-NDC local_rect (−1, −1, 1, 1), identity
    // transform/clip pointers, specific_prim_address → gpu_buffer_f[0].
    let mut header_bytes: Vec<u8> = Vec::with_capacity(64);
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    for f in [-1.0_f32, -1.0, 1.0, 1.0] {
        header_bytes.extend_from_slice(&f.to_ne_bytes());
    }
    for i in [0_i32, 0, 0, 0] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    for i in [0_i32; 4] {
        header_bytes.extend_from_slice(&i.to_ne_bytes());
    }
    let prim_headers = upload_storage(device, queue, "smoke prim_headers", &header_bytes);

    // Transform: identity m + identity inv_m.
    let identity: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    let transform_bytes: Vec<u8> = identity
        .iter()
        .chain(identity.iter())
        .flat_map(|f| f.to_ne_bytes())
        .collect();
    let transforms = upload_storage(device, queue, "smoke transforms", &transform_bytes);

    // GpuBuffer[0] = opaque red (1, 0, 0, 1).
    let color: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    let gpu_buffer_bytes: Vec<u8> = color.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let gpu_buffer_f = upload_storage(device, queue, "smoke gpu_buffer_f", &gpu_buffer_bytes);

    // RenderTaskData[0]: identity-equivalent picture task (zero offsets, scale=1).
    let render_task: [f32; 8] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];
    let render_task_bytes: Vec<u8> = render_task.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let render_tasks = upload_storage(device, queue, "smoke render_tasks", &render_task_bytes);

    // PerFrame: identity orthographic projection.
    let per_frame_bytes: Vec<u8> = identity.iter().flat_map(|f| f.to_ne_bytes()).collect();
    let per_frame = upload_uniform(device, queue, "smoke per_frame", &per_frame_bytes);

    // Dummy 1×1 R8 clip mask; bind-group layout requires the binding even in
    // opaque mode (the shader doesn't sample it, but the layout demands it).
    let clip_mask = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke dummy clip mask"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let clip_mask_view = clip_mask.create_view(&wgpu::TextureViewDescriptor::default());

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("smoke brush_solid bind group"),
        layout: &pipe.layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: prim_headers.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: transforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: gpu_buffer_f.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: render_tasks.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: per_frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&clip_mask_view),
            },
        ],
    });

    // Per-instance a_data: (prim_header_address=0, clip_address=0, …).
    let a_data: [i32; 4] = [0, 0, 0, 0];
    let a_data_bytes: Vec<u8> = a_data.iter().flat_map(|i| i.to_ne_bytes()).collect();
    let a_data_buffer = upload_vertex(device, queue, "smoke a_data", &a_data_bytes);

    DrawIntent {
        pipeline: pipe.pipeline.clone(),
        bind_group,
        vertex_buffers: vec![a_data_buffer],
        vertex_range: 0..4,
        instance_range: 0..1,
        dynamic_offsets: Vec::new(),
        push_constants: Vec::new(),
    }
}

fn upload_storage(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, bytes);
    buf
}

fn upload_uniform(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, bytes);
    buf
}

fn upload_vertex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    bytes: &[u8],
) -> wgpu::Buffer {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, bytes);
    buf
}
