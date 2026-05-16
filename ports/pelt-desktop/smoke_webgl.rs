/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebGlWgpuSmokeOutcome {
    pub width: u32,
    pub height: u32,
    pub painted_pixels: usize,
    pub canvas_center: [u8; 4],
    pub overlay_center: [u8; 4],
}

#[derive(Default)]
struct CaptureMasterCompositor {
    master: Option<wgpu::Texture>,
}

#[cfg(feature = "netrender")]
impl netrender::Compositor for CaptureMasterCompositor {
    fn declare_surface(&mut self, _key: netrender::SurfaceKey, _world_bounds: [f32; 4]) {}

    fn destroy_surface(&mut self, _key: netrender::SurfaceKey) {}

    fn present_frame(&mut self, frame: netrender::PresentedFrame<'_>) {
        self.master = Some(frame.master.clone());
    }
}

#[cfg(feature = "netrender")]
fn draw_webgl_smoke_triangle(
    device: wgpu::Device,
    queue: wgpu::Queue,
) -> Result<webgl_wgpu::WebGlContext, String> {
    use webgl_wgpu::{
        BufferTarget, BufferUsage, CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        CANONICAL_TRIANGLE_VERTEX_SHADER, PrimitiveMode, WebGlCanvasDescriptor, WebGlError,
    };

    let mut context = webgl_wgpu::WebGlContext::from_wgpu_handles(
        device,
        queue,
        WebGlCanvasDescriptor::new(32, 32),
    )
    .map_err(|error| format!("webgl-wgpu context init failed: {error:?}"))?;
    context.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    });

    let program = context
        .create_program_from_essl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .ok_or_else(|| "webgl-wgpu canonical shader pair was rejected".to_owned())?;
    context.use_program(Some(program));
    let position_location = context.get_attrib_location(program, "a_position");
    if position_location != 0 {
        return Err(format!(
            "webgl-wgpu a_position location drifted: expected 0, got {position_location}"
        ));
    }

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
    if context.get_error() != WebGlError::NoError {
        return Err("webgl-wgpu draw_arrays reported an unexpected WebGL error".into());
    }

    Ok(context)
}

#[cfg(feature = "netrender")]
fn read_smoke_pixel(bytes: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * width + x) * 4) as usize;
    [
        bytes[index],
        bytes[index + 1],
        bytes[index + 2],
        bytes[index + 3],
    ]
}

/// Non-presenting Pelt-owned W4 smoke. This is intentionally shaped
/// like `run_netrender_smoke`: boot the host renderer, build one frame,
/// capture the compositor master, and inspect pixels. The extra proof
/// is that the frame includes a real WebGL-over-wgpu producer texture
/// interleaved between ordinary NetRender scene ops.
#[cfg(feature = "netrender")]
pub fn run_webgl_wgpu_smoke() -> Result<WebGlWgpuSmokeOutcome, String> {
    const DIM: u32 = 64;

    let handles =
        netrender::boot().map_err(|error| format!("netrender wgpu boot failed: {error}"))?;
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    let renderer = netrender::create_netrender_instance(
        handles,
        netrender::NetrenderOptions {
            tile_cache_size: Some(32),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|error| format!("netrender renderer init failed: {error:?}"))?;
    let webgl = draw_webgl_smoke_triangle(device, queue)?;
    let canvas_view = webgl
        .texture()
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let mut scene = netrender::Scene::new(DIM, DIM);
    scene.push_rect(0.0, 0.0, DIM as f32, DIM as f32, [1.0, 0.0, 0.0, 1.0]);
    scene.push_rect(28.0, 28.0, 36.0, 36.0, [0.0, 0.0, 1.0, 1.0]);

    let external = [netrender::ExternalTextureComposite::new(
        &canvas_view,
        netrender::ExternalTexturePlacement::new([16.0, 16.0, 48.0, 48.0]),
    )
    .with_scene_op_boundary(1)];
    let mut compositor = CaptureMasterCompositor::default();
    renderer.render_with_compositor_and_external_textures(
        &scene,
        wgpu::TextureFormat::Rgba8Unorm,
        &mut compositor,
        netrender::peniko::Color::new([0.0, 0.0, 0.0, 0.0]),
        &external,
    );

    let master = compositor
        .master
        .ok_or_else(|| "netrender compositor did not present a master texture".to_owned())?;
    let bytes = renderer.wgpu_device.read_rgba8_texture(&master, DIM, DIM);
    let canvas_center = read_smoke_pixel(&bytes, DIM, 32, 26);
    let overlay_center = read_smoke_pixel(&bytes, DIM, 32, 32);
    if canvas_center != [0, 255, 0, 255] {
        return Err(format!(
            "webgl-wgpu canvas sample mismatch: expected [0, 255, 0, 255], got {canvas_center:?}"
        ));
    }
    if overlay_center != [0, 0, 255, 255] {
        return Err(format!(
            "webgl-wgpu overlay sample mismatch: expected [0, 0, 255, 255], got {overlay_center:?}"
        ));
    }

    let painted_pixels = bytes
        .chunks_exact(4)
        .filter(|rgba| rgba[0] != 0 || rgba[1] != 0 || rgba[2] != 0 || rgba[3] != 0)
        .count();

    Ok(WebGlWgpuSmokeOutcome {
        width: DIM,
        height: DIM,
        painted_pixels,
        canvas_center,
        overlay_center,
    })
}
