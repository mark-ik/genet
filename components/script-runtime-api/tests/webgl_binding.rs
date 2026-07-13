// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! End-to-end smoke for the JS `WebGLRenderingContext` surface against a real
//! `webgl-wgpu` context. The test handler is a thin GLenum-decoding wrapper
//! over `WebGlContext`; the smoke draws a uniform-colored triangle through the
//! JS WebGL API and reads pixels back through `gl.readPixels`. Proves that
//! every sink in `webgl::install_webgl_surface` rounds-trips id / data /
//! pixel values correctly.

use std::cell::RefCell;
use std::collections::HashMap;

use script_engine_api::ScriptEngine;
use script_engine_boa::BoaEngine;
use script_runtime_api::{Runtime, WebGlHandler};
use webgl_wgpu::{
    BufferTarget, BufferUsage, PrimitiveMode, ShaderStage, WebGlBufferId, WebGlCanvas,
    WebGlCanvasDescriptor, WebGlContext, WebGlError, WebGlProgramId, WebGlShaderId, WebGlTextureId,
    WebGlUniformLocation,
};

const GL_NO_ERROR: u32 = 0x0000;
const GL_INVALID_ENUM: u32 = 0x0500;
const GL_INVALID_VALUE: u32 = 0x0501;
const GL_INVALID_OPERATION: u32 = 0x0502;
const GL_INVALID_FRAMEBUFFER_OPERATION: u32 = 0x0506;
const GL_CONTEXT_LOST_WEBGL: u32 = 0x9242;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_TRIANGLES: u32 = 0x0004;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_DITHER: u32 = 0x0BD0;
const GL_SCISSOR_TEST: u32 = 0x0C11;
const TEST_EXTERNAL_TEXTURE_KEY: u64 = 9001;

/// Test-side WebGL handler. Owns a `WebGlContext` and translates the raw
/// GLenums / opaque ids the JS bootstrap hands across into webgl-wgpu's typed
/// API. Shader and program ids cross 1:1 (webgl-wgpu already uses u64-shaped
/// ids internally that we re-emit); uniform locations need an indirection
/// because `WebGlUniformLocation` is a tagged enum, not an integer.
struct WgpuWebGl {
    context: RefCell<WebGlContext>,
    /// Next id to hand out across the JS seam for `create_*`. Distinct from
    /// the WebGl*Id namespaces because webgl-wgpu's ids start at 1 per
    /// resource kind; we keep one shared counter to avoid id-collisions
    /// across kinds on the JS side.
    next_id: RefCell<u64>,
    buffers: RefCell<HashMap<u64, WebGlBufferId>>,
    shaders: RefCell<HashMap<u64, WebGlShaderId>>,
    programs: RefCell<HashMap<u64, WebGlProgramId>>,
    textures: RefCell<HashMap<u64, WebGlTextureId>>,
    /// `i32` index into this list is the opaque uniform-location id the JS
    /// wrapper holds onto. `getUniformLocation` returns the index; setters
    /// look it up by index. `-1` is reserved for "not found", per WebGL.
    uniform_locations: RefCell<Vec<WebGlUniformLocation>>,
    /// Current `gl.clearColor` state. webgl-wgpu's `clear` takes the color
    /// directly each call (no bound-color state), so we hold it here and
    /// pass it through on `clear`. WebGL's default is transparent black.
    clear_color: RefCell<[f32; 4]>,
    /// `gl.enable` / `gl.disable` state, keyed by GLenum cap. Source of
    /// truth for `isEnabled`. Only `SCISSOR_TEST` currently forwards to the
    /// backend (webgl-wgpu's scissor toggle); the rest are recorded so the
    /// round-trip is correct even though the draw effect isn't wired yet
    /// (BLEND / CULL_FACE aren't exposed; DEPTH_TEST needs a depth canvas).
    enabled_caps: RefCell<std::collections::HashSet<u32>>,
}

/// One shared wgpu device/queue for every test context. Creating a fresh
/// adapter+device per `WgpuWebGl` raced the driver under the default
/// multi-threaded `cargo test` (a STATUS_ACCESS_VIOLATION on Windows); wgpu's
/// `Device`/`Queue` are `Send + Sync + Clone`, so caching one pair and cloning
/// it per context removes the device-creation storm while keeping each context
/// an independent `WebGlCanvas` framebuffer.
fn shared_device() -> (wgpu::Device, wgpu::Queue) {
    static DEVICE: std::sync::OnceLock<(wgpu::Device, wgpu::Queue)> = std::sync::OnceLock::new();
    DEVICE
        .get_or_init(|| {
            let instance = wgpu::Instance::default();
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                }))
                .expect("wgpu adapter");
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("script-runtime-api webgl test device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
            }))
            .expect("wgpu device")
        })
        .clone()
}

impl WgpuWebGl {
    fn new(width: u32, height: u32) -> Self {
        let (device, queue) = shared_device();
        let canvas = WebGlCanvas::from_wgpu_handles(
            device,
            queue,
            WebGlCanvasDescriptor::new(width, height),
        )
        .expect("canvas");
        let context = WebGlContext::from_canvas(canvas);
        // WebGL's only default-enabled capability is DITHER.
        let mut caps = std::collections::HashSet::new();
        caps.insert(GL_DITHER);
        Self {
            context: RefCell::new(context),
            next_id: RefCell::new(1),
            buffers: RefCell::new(HashMap::new()),
            shaders: RefCell::new(HashMap::new()),
            programs: RefCell::new(HashMap::new()),
            textures: RefCell::new(HashMap::new()),
            uniform_locations: RefCell::new(Vec::new()),
            clear_color: RefCell::new([0.0, 0.0, 0.0, 0.0]),
            enabled_caps: RefCell::new(caps),
        }
    }

    fn alloc_id(&self) -> u64 {
        let mut n = self.next_id.borrow_mut();
        let id = *n;
        *n += 1;
        id
    }

    fn buffer_target(target: u32) -> Option<BufferTarget> {
        match target {
            GL_ARRAY_BUFFER => Some(BufferTarget::ArrayBuffer),
            GL_ELEMENT_ARRAY_BUFFER => Some(BufferTarget::ElementArrayBuffer),
            _ => None,
        }
    }
}

impl WebGlHandler for WgpuWebGl {
    fn external_texture_key(&self) -> Option<u64> {
        Some(TEST_EXTERNAL_TEXTURE_KEY)
    }

    fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        // webgl-wgpu's clear takes the color directly each call (no
        // bound-color state), so hold it and pass it through on `clear`.
        *self.clear_color.borrow_mut() = [r, g, b, a];
    }

    fn clear(&mut self, mask: u32) {
        if mask & GL_COLOR_BUFFER_BIT != 0 {
            let c = *self.clear_color.borrow();
            self.context.borrow_mut().clear(wgpu::Color {
                r: c[0] as f64,
                g: c[1] as f64,
                b: c[2] as f64,
                a: c[3] as f64,
            });
        }
    }

    fn enable(&mut self, cap: u32) {
        self.enabled_caps.borrow_mut().insert(cap);
        if cap == GL_SCISSOR_TEST {
            self.context.borrow_mut().set_scissor_test_enabled(true);
        }
        // DEPTH_TEST / BLEND / CULL_FACE recorded only — see struct doc.
    }

    fn disable(&mut self, cap: u32) {
        self.enabled_caps.borrow_mut().remove(&cap);
        if cap == GL_SCISSOR_TEST {
            self.context.borrow_mut().set_scissor_test_enabled(false);
        }
    }

    fn is_enabled(&mut self, cap: u32) -> bool {
        self.enabled_caps.borrow().contains(&cap)
    }

    fn color_mask(&mut self, r: bool, g: bool, b: bool, a: bool) {
        self.context.borrow_mut().set_color_mask(r, g, b, a);
    }

    fn viewport(&mut self, _x: i32, _y: i32, _w: u32, _h: u32) {
        // Viewport is a draw-state knob webgl-wgpu doesn't expose yet on
        // the WebGlContext directly; the canvas size is the implicit
        // viewport. The smoke draws full-canvas, so a no-op suffices.
    }

    fn create_buffer(&mut self) -> u64 {
        let id = self.alloc_id();
        let buffer = self.context.borrow_mut().create_buffer();
        self.buffers.borrow_mut().insert(id, buffer);
        id
    }
    fn bind_buffer(&mut self, target: u32, buffer: Option<u64>) {
        let Some(target) = Self::buffer_target(target) else {
            return;
        };
        let resolved = buffer.and_then(|id| self.buffers.borrow().get(&id).copied());
        self.context.borrow_mut().bind_buffer(target, resolved);
    }
    fn buffer_data_f32(&mut self, target: u32, data: &[f32], _usage: u32) {
        let Some(target) = Self::buffer_target(target) else {
            return;
        };
        self.context
            .borrow_mut()
            .buffer_data_f32(target, data, BufferUsage::StaticDraw);
    }

    fn create_shader(&mut self, stage: u32) -> u64 {
        let stage = match stage {
            GL_VERTEX_SHADER => ShaderStage::Vertex,
            GL_FRAGMENT_SHADER => ShaderStage::Fragment,
            _ => return 0,
        };
        let id = self.alloc_id();
        let shader = self.context.borrow_mut().create_shader(stage);
        self.shaders.borrow_mut().insert(id, shader);
        id
    }
    fn shader_source(&mut self, shader: u64, source: &str) {
        let Some(&shader_id) = self.shaders.borrow().get(&shader) else {
            return;
        };
        self.context.borrow_mut().shader_source(shader_id, source);
    }
    fn compile_shader(&mut self, shader: u64) {
        let Some(&shader_id) = self.shaders.borrow().get(&shader) else {
            return;
        };
        self.context.borrow_mut().compile_shader(shader_id);
    }
    fn get_shader_compile_status(&mut self, shader: u64) -> bool {
        let Some(&shader_id) = self.shaders.borrow().get(&shader) else {
            return false;
        };
        self.context
            .borrow_mut()
            .get_shader_compile_status(shader_id)
    }
    fn get_shader_info_log(&mut self, shader: u64) -> String {
        let Some(&shader_id) = self.shaders.borrow().get(&shader) else {
            return String::new();
        };
        self.context
            .borrow_mut()
            .get_shader_info_log(shader_id)
            .unwrap_or_default()
    }

    fn create_program(&mut self) -> u64 {
        let id = self.alloc_id();
        let program = self.context.borrow_mut().create_program();
        self.programs.borrow_mut().insert(id, program);
        id
    }
    fn attach_shader(&mut self, program: u64, shader: u64) {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return;
        };
        let Some(&shader_id) = self.shaders.borrow().get(&shader) else {
            return;
        };
        self.context
            .borrow_mut()
            .attach_shader(program_id, shader_id);
    }
    fn link_program(&mut self, program: u64) {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return;
        };
        self.context.borrow_mut().link_program(program_id);
    }
    fn get_program_link_status(&mut self, program: u64) -> bool {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return false;
        };
        self.context
            .borrow_mut()
            .get_program_link_status(program_id)
    }
    fn get_program_info_log(&mut self, program: u64) -> String {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return String::new();
        };
        self.context
            .borrow_mut()
            .get_program_info_log(program_id)
            .unwrap_or_default()
    }
    fn use_program(&mut self, program: Option<u64>) {
        let resolved = program.and_then(|id| self.programs.borrow().get(&id).copied());
        self.context.borrow_mut().use_program(resolved);
    }

    fn get_attrib_location(&mut self, program: u64, name: &str) -> i32 {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return -1;
        };
        self.context
            .borrow_mut()
            .get_attrib_location(program_id, name)
    }
    fn get_uniform_location(&mut self, program: u64, name: &str) -> i32 {
        let Some(&program_id) = self.programs.borrow().get(&program) else {
            return -1;
        };
        let Some(loc) = self
            .context
            .borrow_mut()
            .get_uniform_location(program_id, name)
        else {
            return -1;
        };
        let mut locs = self.uniform_locations.borrow_mut();
        let index = locs.len() as i32;
        locs.push(loc);
        index
    }

    fn enable_vertex_attrib_array(&mut self, index: u32) {
        self.context.borrow_mut().enable_vertex_attrib_array(index);
    }
    fn vertex_attrib_pointer_f32(
        &mut self,
        index: u32,
        size: u32,
        normalized: bool,
        stride: u32,
        offset: u32,
    ) {
        self.context.borrow_mut().vertex_attrib_pointer_f32(
            index,
            size,
            normalized,
            stride as u64,
            offset as u64,
        );
    }

    fn uniform4f(&mut self, location: i32, x: f32, y: f32, z: f32, w: f32) {
        if location < 0 {
            return;
        }
        let loc = match self.uniform_locations.borrow().get(location as usize) {
            Some(loc) => *loc,
            None => return,
        };
        self.context.borrow_mut().uniform4f(loc, x, y, z, w);
    }
    fn uniform_matrix4fv(&mut self, location: i32, _transpose: bool, value: &[f32]) {
        if location < 0 || value.len() < 16 {
            return;
        }
        let loc = match self.uniform_locations.borrow().get(location as usize) {
            Some(loc) => *loc,
            None => return,
        };
        let mut m = [0.0f32; 16];
        m.copy_from_slice(&value[..16]);
        self.context.borrow_mut().uniform_matrix4fv(loc, &m);
    }
    fn uniform1i(&mut self, location: i32, value: i32) {
        if location < 0 {
            return;
        }
        let loc = match self.uniform_locations.borrow().get(location as usize) {
            Some(loc) => *loc,
            None => return,
        };
        self.context.borrow_mut().uniform1i(loc, value);
    }

    fn create_texture(&mut self) -> u64 {
        let id = self.alloc_id();
        let texture = self.context.borrow_mut().create_texture();
        self.textures.borrow_mut().insert(id, texture);
        id
    }
    fn bind_texture_2d(&mut self, texture: Option<u64>) {
        let resolved = texture.and_then(|id| self.textures.borrow().get(&id).copied());
        self.context.borrow_mut().bind_texture_2d(resolved);
    }
    fn active_texture(&mut self, unit: u32) {
        self.context.borrow_mut().active_texture(unit);
    }
    fn tex_image_2d_rgba8(&mut self, width: u32, height: u32, pixels: &[u8]) {
        self.context
            .borrow_mut()
            .tex_image_2d_rgba8(width, height, pixels);
    }

    fn draw_arrays(&mut self, mode: u32, first: i32, count: i32) {
        let topology = match mode {
            GL_TRIANGLES => PrimitiveMode::Triangles,
            _ => return,
        };
        self.context
            .borrow_mut()
            .draw_arrays(topology, first as u32, count as u32);
    }

    fn get_error(&mut self) -> u32 {
        match self.context.borrow_mut().get_error() {
            WebGlError::NoError => GL_NO_ERROR,
            WebGlError::InvalidEnum => GL_INVALID_ENUM,
            WebGlError::InvalidValue => GL_INVALID_VALUE,
            WebGlError::InvalidOperation => GL_INVALID_OPERATION,
            WebGlError::InvalidFramebufferOperation => GL_INVALID_FRAMEBUFFER_OPERATION,
            WebGlError::ContextLostWebgl => GL_CONTEXT_LOST_WEBGL,
        }
    }

    fn read_pixels_rgba8(&mut self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        self.context
            .borrow_mut()
            .read_pixels(x as u32, y as u32, w, h)
            .unwrap_or_default()
    }
}

fn read(rt: &mut Runtime<BoaEngine>, expr: &str) -> String {
    let v = rt.eval(expr).expect("eval");
    rt.engine_mut()
        .value_to_string(&v)
        .expect("value_to_string")
}

#[test]
fn webgl_js_surface_draws_uniform_color_triangle_end_to_end() {
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.set_webgl_factory(Box::new(|w, h| Box::new(WgpuWebGl::new(w, h))));

    // The bare-context helper mints a context at the given size through
    // the factory (the getContext path is exercised separately below).
    let setup = r#"
        var gl = __createWebGLContext(32, 32);
        gl.viewport(0, 0, 32, 32);
        gl.clearColor(0, 0, 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);

        var vsrc =
          "attribute vec2 a_position; void main() {" +
          "  gl_Position = vec4(a_position, 0.0, 1.0); }";
        var fsrc =
          "precision mediump float; uniform vec4 u_color;" +
          "void main() { gl_FragColor = u_color; }";

        var vs = gl.createShader(gl.VERTEX_SHADER);
        gl.shaderSource(vs, vsrc);
        gl.compileShader(vs);
        var vsOk = gl.getShaderParameter(vs, gl.COMPILE_STATUS);

        var fs = gl.createShader(gl.FRAGMENT_SHADER);
        gl.shaderSource(fs, fsrc);
        gl.compileShader(fs);
        var fsOk = gl.getShaderParameter(fs, gl.COMPILE_STATUS);

        var prog = gl.createProgram();
        gl.attachShader(prog, vs);
        gl.attachShader(prog, fs);
        gl.linkProgram(prog);
        var linkOk = gl.getProgramParameter(prog, gl.LINK_STATUS);
        gl.useProgram(prog);

        var posLoc = gl.getAttribLocation(prog, "a_position");
        var colorLoc = gl.getUniformLocation(prog, "u_color");
        gl.uniform4f(colorLoc, 0.0, 1.0, 0.0, 1.0);

        var buf = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, buf);
        gl.bufferData(gl.ARRAY_BUFFER,
                      [-0.8, -0.8, 0.8, -0.8, 0.0, 0.8],
                      gl.STATIC_DRAW);
        gl.enableVertexAttribArray(posLoc);
        gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);
        gl.drawArrays(gl.TRIANGLES, 0, 3);

        var err = gl.getError();
        var center = gl.readPixels(16, 16, 1, 1, 0, 0);
        var bag = {
          vsOk: vsOk,
          fsOk: fsOk,
          linkOk: linkOk,
          posLoc: posLoc,
          gotColorLoc: (colorLoc !== null),
          err: err,
          r: center[0],
          g: center[1],
          b: center[2],
          a: center[3],
        };
    "#;
    rt.eval(setup).expect("setup");

    assert_eq!(read(&mut rt, "String(bag.vsOk)"), "true");
    assert_eq!(read(&mut rt, "String(bag.fsOk)"), "true");
    assert_eq!(read(&mut rt, "String(bag.linkOk)"), "true");
    assert_eq!(read(&mut rt, "String(bag.posLoc)"), "0");
    assert_eq!(read(&mut rt, "String(bag.gotColorLoc)"), "true");
    assert_eq!(read(&mut rt, "String(bag.err)"), "0");
    assert_eq!(read(&mut rt, "String(bag.r)"), "0");
    assert_eq!(read(&mut rt, "String(bag.g)"), "255");
    assert_eq!(read(&mut rt, "String(bag.b)"), "0");
    assert_eq!(read(&mut rt, "String(bag.a)"), "255");
}

#[test]
fn html_canvas_get_context_webgl_returns_a_rendering_context() {
    // The standard Web API path: createElement('canvas') ->
    // HTMLCanvasElement, .getContext('webgl') -> WebGLRenderingContext.
    // Same draw shape as the bare-helper smoke above, but the JS now
    // matches what a conformance test will write verbatim.
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.set_webgl_factory(Box::new(|w, h| Box::new(WgpuWebGl::new(w, h))));

    let setup = r#"
        var c = document.createElement('canvas');
        // Size the canvas so the factory mints a 32x32 drawing buffer and
        // the center pixel (16,16) lands inside the clip-space triangle.
        c.setAttribute('width', '32');
        c.setAttribute('height', '32');
        var isCanvas = (c instanceof HTMLCanvasElement);
        var ctx = c.getContext('webgl');
        var isCtx = (ctx instanceof WebGLRenderingContext);
        // Per spec: getContext returns the same instance on repeat calls.
        var sameTwice = (ctx === c.getContext('webgl'));
        // experimental-webgl alias resolves to the same constructor too.
        var alias = c.getContext('experimental-webgl');
        var aliasMatches = (alias === ctx);
        // Unknown contextType returns null.
        var unknown = c.getContext('webgl2');

        ctx.clearColor(0, 0, 0, 1);
        ctx.clear(ctx.COLOR_BUFFER_BIT);
        var vs = ctx.createShader(ctx.VERTEX_SHADER);
        ctx.shaderSource(vs,
          "attribute vec2 a; void main() { gl_Position = vec4(a, 0.0, 1.0); }");
        ctx.compileShader(vs);
        var fs = ctx.createShader(ctx.FRAGMENT_SHADER);
        ctx.shaderSource(fs,
          "precision mediump float; uniform vec4 u;" +
          " void main() { gl_FragColor = u; }");
        ctx.compileShader(fs);
        var prog = ctx.createProgram();
        ctx.attachShader(prog, vs);
        ctx.attachShader(prog, fs);
        ctx.linkProgram(prog);
        ctx.useProgram(prog);
        var loc = ctx.getAttribLocation(prog, 'a');
        var uloc = ctx.getUniformLocation(prog, 'u');
        ctx.uniform4f(uloc, 1.0, 0.5, 0.0, 1.0);
        var buf = ctx.createBuffer();
        ctx.bindBuffer(ctx.ARRAY_BUFFER, buf);
        ctx.bufferData(ctx.ARRAY_BUFFER, [-0.8, -0.8, 0.8, -0.8, 0.0, 0.8], ctx.STATIC_DRAW);
        ctx.enableVertexAttribArray(loc);
        ctx.vertexAttribPointer(loc, 2, ctx.FLOAT, false, 0, 0);
        ctx.drawArrays(ctx.TRIANGLES, 0, 3);
        var px = ctx.readPixels(16, 16, 1, 1, 0, 0);
        var bag = {
          isCanvas: isCanvas,
          isCtx: isCtx,
          sameTwice: sameTwice,
          aliasMatches: aliasMatches,
          unknown: (unknown === null),
          textureKey: c.getAttribute('data-genet-external-texture-key'),
          r: px[0], g: px[1], b: px[2], a: px[3],
        };
    "#;
    rt.eval(setup).expect("setup");

    assert_eq!(read(&mut rt, "String(bag.isCanvas)"), "true");
    assert_eq!(read(&mut rt, "String(bag.isCtx)"), "true");
    assert_eq!(read(&mut rt, "String(bag.sameTwice)"), "true");
    assert_eq!(read(&mut rt, "String(bag.aliasMatches)"), "true");
    assert_eq!(read(&mut rt, "String(bag.unknown)"), "true");
    assert_eq!(
        read(&mut rt, "String(bag.textureKey)"),
        TEST_EXTERNAL_TEXTURE_KEY.to_string()
    );
    assert_eq!(read(&mut rt, "String(bag.r)"), "255");
    // 0.5 → u8 round-trip lands at 128 under wgpu's UNORM conversion.
    assert_eq!(read(&mut rt, "String(bag.g)"), "128");
    assert_eq!(read(&mut rt, "String(bag.b)"), "0");
    assert_eq!(read(&mut rt, "String(bag.a)"), "255");
}

#[test]
fn html_canvas_get_context_returns_null_without_webgl_constructor() {
    // If the runtime is missing the webgl bootstrap (e.g. an alternate
    // install_host_surface), HTMLCanvasElement.getContext falls back to
    // returning null rather than throwing — matches Web API behavior
    // for an unsupported contextType.
    //
    // In the standard runtime the bootstrap IS installed, so this test
    // proves the negative path via a non-existent contextType.
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.eval(
        r#"
        var c = document.createElement('canvas');
        var webgpu = c.getContext('webgpu');
        var twod = c.getContext('2d');
        var bag = { webgpu: webgpu, twod: twod };
        "#,
    )
    .expect("eval");
    assert_eq!(read(&mut rt, "String(bag.webgpu)"), "null");
    assert_eq!(read(&mut rt, "String(bag.twod)"), "null");
}

#[test]
fn two_canvases_get_independent_contexts() {
    // Each getContext('webgl') invokes the factory afresh, so two canvases
    // hold distinct contexts with distinct registry indices and distinct
    // underlying wgpu framebuffers. Drawing a different color into each and
    // reading both back proves they don't alias.
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.set_webgl_factory(Box::new(|w, h| Box::new(WgpuWebGl::new(w, h))));

    let setup = r#"
        function makeCanvas() {
            var c = document.createElement('canvas');
            c.setAttribute('width', '16');
            c.setAttribute('height', '16');
            return c.getContext('webgl');
        }
        function drawColor(gl, r, g, b) {
            var vs = gl.createShader(gl.VERTEX_SHADER);
            gl.shaderSource(vs, "attribute vec2 a; void main(){ gl_Position = vec4(a,0.0,1.0); }");
            gl.compileShader(vs);
            var fs = gl.createShader(gl.FRAGMENT_SHADER);
            gl.shaderSource(fs, "precision mediump float; uniform vec4 u; void main(){ gl_FragColor = u; }");
            gl.compileShader(fs);
            var p = gl.createProgram();
            gl.attachShader(p, vs); gl.attachShader(p, fs); gl.linkProgram(p);
            gl.useProgram(p);
            var loc = gl.getAttribLocation(p, 'a');
            gl.uniform4f(gl.getUniformLocation(p, 'u'), r, g, b, 1.0);
            var buf = gl.createBuffer();
            gl.bindBuffer(gl.ARRAY_BUFFER, buf);
            gl.bufferData(gl.ARRAY_BUFFER, [-1.0,-1.0, 3.0,-1.0, -1.0,3.0], gl.STATIC_DRAW);
            gl.enableVertexAttribArray(loc);
            gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
            gl.drawArrays(gl.TRIANGLES, 0, 3);
        }
        var a = makeCanvas();
        var b = makeCanvas();
        var distinct = (a._ctx !== b._ctx);
        // Big covering triangle so the center pixel is always inside.
        drawColor(a, 1.0, 0.0, 0.0);  // red into context A
        drawColor(b, 0.0, 0.0, 1.0);  // blue into context B
        var pa = a.readPixels(8, 8, 1, 1, 0, 0);
        var pb = b.readPixels(8, 8, 1, 1, 0, 0);
        var bag = {
            distinct: distinct,
            aCtx: a._ctx, bCtx: b._ctx,
            ar: pa[0], ag: pa[1], ab: pa[2],
            br: pb[0], bg: pb[1], bb: pb[2],
        };
    "#;
    rt.eval(setup).expect("setup");

    assert_eq!(read(&mut rt, "String(bag.distinct)"), "true");
    assert_eq!(read(&mut rt, "String(bag.aCtx)"), "0");
    assert_eq!(read(&mut rt, "String(bag.bCtx)"), "1");
    // Context A is red, context B is blue — no aliasing.
    assert_eq!(read(&mut rt, "String(bag.ar)"), "255");
    assert_eq!(read(&mut rt, "String(bag.ab)"), "0");
    assert_eq!(read(&mut rt, "String(bag.br)"), "0");
    assert_eq!(read(&mut rt, "String(bag.bb)"), "255");
}

#[test]
fn webgl_js_surface_with_no_handler_no_ops_safely() {
    // Without `set_webgl_factory`, context creation mints index -1 and
    // every sink returns the default value (0 / NO_ERROR / empty pixel
    // bytes). The JS surface must not throw.
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.eval(
        r#"
        var gl = __createWebGLContext(4, 4);
        gl.viewport(0, 0, 4, 4);
        gl.clearColor(1, 0, 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        var err = gl.getError();
        var px = gl.readPixels(0, 0, 1, 1, 0, 0);
        var noBuf = (gl.createBuffer() instanceof WebGLBuffer);
        var noShader = (gl.createShader(gl.VERTEX_SHADER) instanceof WebGLShader);
        var noProg = (gl.createProgram() instanceof WebGLProgram);
        "#,
    )
    .expect("setup");
    assert_eq!(read(&mut rt, "String(err)"), "0");
    assert_eq!(read(&mut rt, "String(px.length)"), "0");
    assert_eq!(read(&mut rt, "String(noBuf)"), "true");
    assert_eq!(read(&mut rt, "String(noShader)"), "true");
    assert_eq!(read(&mut rt, "String(noProg)"), "true");
}

// =====================================================================
// Conformance-shaped receipt: the clear portion of the Khronos WebGL
// conformance test `conformance/rendering/gl-clear.html`, written the
// way that test writes it (getContext, clearColor + clear + colorMask,
// read back and compare). We don't load the upstream HTML — it pulls in
// `webgl-test-utils.js` + `js-test-pre.js` (thousands of lines). This
// covers the clear-color + colorMask cases now that the backend honors
// masked clears; the remaining upstream case (drawing a sampled
// textured quad) is the next gap — JS-side texImage2D.
//
// Source: tests/wpt/webgl/tests/conformance/rendering/gl-clear.html
// =====================================================================

#[test]
fn conformance_gl_clear_color_cases() {
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.set_webgl_factory(Box::new(|w, h| Box::new(WgpuWebGl::new(w, h))));

    // gl-clear.html uses a 1x1 canvas. `checkCanvas` mirrors wtu's:
    // read the single pixel and stringify it for an exact compare.
    let setup = r#"
        var c = document.createElement('canvas');
        c.setAttribute('width', '1');
        c.setAttribute('height', '1');
        var gl = c.getContext('webgl');

        function checkCanvas() {
            var p = gl.readPixels(0, 0, 1, 1, 0, 0);
            return p[0] + ',' + p[1] + ',' + p[2] + ',' + p[3];
        }

        var results = {};

        // gl-clear: disable depth + blend before the color work (as the
        // upstream test does). isEnabled must round-trip.
        gl.disable(gl.DEPTH_TEST);
        gl.disable(gl.BLEND);
        results.depthOff = gl.isEnabled(gl.DEPTH_TEST);   // false
        results.blendOff = gl.isEnabled(gl.BLEND);        // false
        results.ditherOn = gl.isEnabled(gl.DITHER);       // true (default)

        // clearColor(1,1,1,1) -> white.
        gl.clearColor(1, 1, 1, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        results.white = checkCanvas();

        // clearColor(0,0,0,0) -> transparent black.
        gl.clearColor(0, 0, 0, 0);
        gl.clear(gl.COLOR_BUFFER_BIT);
        results.black = checkCanvas();

        // A mid-channel clear: green only.
        gl.clearColor(0, 1, 0, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        results.green = checkCanvas();

        // gl-clear's colorMask case: from transparent black, mask to
        // alpha-only and clear to white -> [0,0,0,255] (RGB stays 0).
        gl.clearColor(0, 0, 0, 0);
        gl.clear(gl.COLOR_BUFFER_BIT);
        gl.colorMask(false, false, false, true);
        gl.clearColor(1, 1, 1, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        results.alphaOnly = checkCanvas();

        // Restore full mask, clear blue -> every channel written.
        gl.colorMask(true, true, true, true);
        gl.clearColor(0, 0, 1, 1);
        gl.clear(gl.COLOR_BUFFER_BIT);
        results.restored = checkCanvas();

        results.err = gl.getError();   // no errors across the run
    "#;
    rt.eval(setup).expect("setup");

    assert_eq!(read(&mut rt, "String(results.depthOff)"), "false");
    assert_eq!(read(&mut rt, "String(results.blendOff)"), "false");
    assert_eq!(read(&mut rt, "String(results.ditherOn)"), "true");
    assert_eq!(read(&mut rt, "results.white"), "255,255,255,255");
    assert_eq!(read(&mut rt, "results.black"), "0,0,0,0");
    assert_eq!(read(&mut rt, "results.green"), "0,255,0,255");
    assert_eq!(read(&mut rt, "results.alphaOnly"), "0,0,0,255");
    assert_eq!(read(&mut rt, "results.restored"), "0,0,255,255");
    assert_eq!(read(&mut rt, "String(results.err)"), "0");
}

#[test]
fn conformance_textured_quad_samples_uploaded_pixel() {
    // The remaining gl-clear shape: upload a texture and draw a sampled
    // quad (gl-clear uses wtu.setupTexturedQuad + texImage2D). A 1x1
    // texture sampled NEAREST over a covering quad paints the whole
    // canvas its single texel color. Exercises createTexture /
    // bindTexture / texImage2D / activeTexture / uniform1i end to end.
    //
    // Cf. tests/wpt/webgl/tests/conformance/rendering/gl-clear.html and
    // the broader conformance/textures/* family this unblocks.
    let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
    rt.set_webgl_factory(Box::new(|w, h| Box::new(WgpuWebGl::new(w, h))));

    let setup = r#"
        var c = document.createElement('canvas');
        c.setAttribute('width', '16');
        c.setAttribute('height', '16');
        var gl = c.getContext('webgl');

        var vs = gl.createShader(gl.VERTEX_SHADER);
        gl.shaderSource(vs,
          "attribute vec2 a_pos; attribute vec2 a_uv; varying vec2 v_uv;" +
          "void main(){ v_uv = a_uv; gl_Position = vec4(a_pos, 0.0, 1.0); }");
        gl.compileShader(vs);
        var fs = gl.createShader(gl.FRAGMENT_SHADER);
        gl.shaderSource(fs,
          "precision mediump float; varying vec2 v_uv; uniform sampler2D u_tex;" +
          "void main(){ gl_FragColor = texture2D(u_tex, v_uv); }");
        gl.compileShader(fs);
        var prog = gl.createProgram();
        gl.attachShader(prog, vs);
        gl.attachShader(prog, fs);
        gl.linkProgram(prog);
        gl.useProgram(prog);
        var linkOk = gl.getProgramParameter(prog, gl.LINK_STATUS);

        // 1x1 RGBA texture with a distinctive color.
        var tex = gl.createTexture();
        gl.activeTexture(gl.TEXTURE0);
        gl.bindTexture(gl.TEXTURE_2D, tex);
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0,
                      gl.RGBA, gl.UNSIGNED_BYTE,
                      new Uint8Array([128, 64, 200, 255]));
        gl.uniform1i(gl.getUniformLocation(prog, 'u_tex'), 0);

        // Covering triangle: positions + uvs in two buffers.
        var posLoc = gl.getAttribLocation(prog, 'a_pos');
        var uvLoc = gl.getAttribLocation(prog, 'a_uv');
        var posBuf = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, posBuf);
        gl.bufferData(gl.ARRAY_BUFFER, [-1.0,-1.0, 3.0,-1.0, -1.0,3.0], gl.STATIC_DRAW);
        gl.enableVertexAttribArray(posLoc);
        gl.vertexAttribPointer(posLoc, 2, gl.FLOAT, false, 0, 0);
        var uvBuf = gl.createBuffer();
        gl.bindBuffer(gl.ARRAY_BUFFER, uvBuf);
        gl.bufferData(gl.ARRAY_BUFFER, [0.0,0.0, 2.0,0.0, 0.0,2.0], gl.STATIC_DRAW);
        gl.enableVertexAttribArray(uvLoc);
        gl.vertexAttribPointer(uvLoc, 2, gl.FLOAT, false, 0, 0);

        gl.drawArrays(gl.TRIANGLES, 0, 3);
        var px = gl.readPixels(8, 8, 1, 1, 0, 0);
        var bag = {
            linkOk: linkOk,
            isTexture: (tex instanceof WebGLTexture),
            r: px[0], g: px[1], b: px[2], a: px[3],
            err: gl.getError(),
        };
    "#;
    rt.eval(setup).expect("setup");

    assert_eq!(read(&mut rt, "String(bag.linkOk)"), "true");
    assert_eq!(read(&mut rt, "String(bag.isTexture)"), "true");
    // The 1x1 texel sampled across the quad — exact round-trip through
    // linear Rgba8Unorm.
    assert_eq!(read(&mut rt, "String(bag.r)"), "128");
    assert_eq!(read(&mut rt, "String(bag.g)"), "64");
    assert_eq!(read(&mut rt, "String(bag.b)"), "200");
    assert_eq!(read(&mut rt, "String(bag.a)"), "255");
    assert_eq!(read(&mut rt, "String(bag.err)"), "0");
}
