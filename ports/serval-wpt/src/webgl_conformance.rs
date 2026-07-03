/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Run the Khronos WebGL conformance suite through the serval-wpt harness.
//!
//! This is the real thing, not a hand-port: the upstream `.html` files load
//! `js-test-pre.js` + `webgl-test-utils.js` + `js-test-post.js` (resolved by
//! the disk loader), draw against a wgpu-backed WebGL context wired through
//! `script_runtime_api::WebGlHandler`, and report pass/fail via the
//! testharness `test()` shim that js-test-pre installs — which the harness's
//! results bridge already collects.
//!
//! The `WgpuWebGl` bridge below mirrors the one in
//! `script-runtime-api/tests/webgl_binding.rs` (that one is the binding's own
//! unit-test fixture); here it's the conformance harness's GPU wiring. Both are
//! thin GLenum-decoding wrappers over `webgl_wgpu::WebGlContext`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use script_runtime_api::WebGlHandler;
use webgl_wgpu::{
    BufferTarget, BufferUsage, PrimitiveMode, ShaderStage, WebGlBufferId, WebGlCanvas,
    WebGlCanvasDescriptor, WebGlContext, WebGlError, WebGlProgramId, WebGlShaderId, WebGlTextureId,
    WebGlUniformLocation,
};

use crate::harness::{DiskLoader, Engine, HarnessOutcome, run_test_with_webgl};

// GLenum subset the bridge decodes.
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

/// One shared wgpu device/queue for every context the conformance run mints,
/// for the same reason as the binding's unit test: per-context device creation
/// races the driver under multi-threaded test execution. `Device`/`Queue` are
/// `Send + Sync + Clone`.
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
                label: Some("serval-wpt webgl conformance device"),
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

struct WgpuWebGl {
    context: RefCell<WebGlContext>,
    next_id: RefCell<u64>,
    buffers: RefCell<HashMap<u64, WebGlBufferId>>,
    shaders: RefCell<HashMap<u64, WebGlShaderId>>,
    programs: RefCell<HashMap<u64, WebGlProgramId>>,
    textures: RefCell<HashMap<u64, WebGlTextureId>>,
    uniform_locations: RefCell<Vec<WebGlUniformLocation>>,
    clear_color: RefCell<[f32; 4]>,
    enabled_caps: RefCell<HashSet<u32>>,
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
        let mut caps = HashSet::new();
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
    fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
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
    fn viewport(&mut self, _x: i32, _y: i32, _w: u32, _h: u32) {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Absolute path to the upstream WPT checkout (`tests/wpt`) relative to
    /// this crate.
    fn wpt_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("wpt")
    }

    fn webgl_factory() -> script_runtime_api::WebGlFactory {
        Box::new(|w, h| Box::new(WgpuWebGl::new(w, h)))
    }

    /// Run one conformance HTML (path relative to `tests/wpt/webgl/tests`)
    /// through the harness with a real WebGL context, returning the outcome.
    fn run_conformance(rel: &str) -> HarnessOutcome {
        let wpt = wpt_root();
        let testharness_js = fs::read_to_string(wpt.join("tests/resources/testharness.js"))
            .expect("read testharness.js");
        let test_path = wpt.join("webgl/tests").join(rel);
        let html = fs::read_to_string(&test_path).expect("read conformance html");
        let base_dir = test_path.parent().unwrap().to_path_buf();
        let tests_root = wpt.join("webgl/tests");
        let loader = DiskLoader {
            base_dir: base_dir.as_path(),
            tests_root: tests_root.as_path(),
        };
        run_test_with_webgl(
            &testharness_js,
            &html,
            &loader,
            None,
            None,
            None,
            Some(webgl_factory()),
            Engine::Boa,
        )
    }

    #[test]
    #[ignore = "diagnostic: prints where the Khronos helpers throw"]
    fn diagnostic_eval_helpers_in_order() {
        use script_engine_api::ScriptEngine;
        use script_engine_boa::BoaEngine;
        use script_runtime_api::Runtime;

        let wpt = wpt_root();
        let read = |p: &str| fs::read_to_string(wpt.join(p)).expect(p);
        let testharness = read("tests/resources/testharness.js");
        let pre = read("webgl/tests/js/js-test-pre.js");
        let wtu = read("webgl/tests/js/webgl-test-utils.js");

        let mut rt = Runtime::<BoaEngine>::new().expect("runtime");
        rt.set_webgl_factory(webgl_factory());

        let mut step = |rt: &mut Runtime<BoaEngine>, label: &str, src: &str| match rt.eval(src) {
            Ok(_) => println!("OK   {label}"),
            Err(e) => println!("FAIL {label}: {e:?}"),
        };
        step(&mut rt, "testharness.js", &testharness);
        step(&mut rt, "setup", "setup({ output: false });");
        step(&mut rt, "js-test-pre.js", &pre);
        step(&mut rt, "webgl-test-utils.js", &wtu);
        step(
            &mut rt,
            "create3DContext",
            r#"
            var c = document.createElement('canvas');
            c.setAttribute('width','1'); c.setAttribute('height','1');
            var gl = WebGLTestUtils.create3DContext(c);
            var wtu = WebGLTestUtils;
            debug('ctx ok: ' + (gl != null));
            "#,
        );
        step(
            &mut rt,
            "setupTexturedQuad",
            "var program = wtu.setupTexturedQuad(gl);",
        );
        step(
            &mut rt,
            "glErrorShouldBe",
            "wtu.glErrorShouldBe(gl, gl.NO_ERROR, 'setup');",
        );
        step(
            &mut rt,
            "checkCanvas",
            "wtu.checkCanvas(gl, [0,0,0,0], 'cleared');",
        );
        step(
            &mut rt,
            "clear-white",
            "gl.clearColor(1,1,1,1); gl.clear(gl.COLOR_BUFFER_BIT); wtu.checkCanvas(gl, [255,255,255,255], 'white');",
        );
        step(
            &mut rt,
            "colorMask",
            "gl.colorMask(false,false,false,true); gl.clearColor(1,1,1,1); gl.clear(gl.COLOR_BUFFER_BIT); wtu.checkCanvas(gl, [0,0,0,255], 'masked');",
        );
        step(
            &mut rt,
            "texImage2D+draw",
            r#"
            var tex = gl.createTexture();
            gl.bindTexture(gl.TEXTURE_2D, tex);
            gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([128,128,128,192]));
            gl.disable(gl.DEPTH_TEST); gl.disable(gl.BLEND);
            gl.colorMask(true,true,true,true);
            gl.drawArrays(gl.TRIANGLES, 0, 6);
            wtu.checkCanvas(gl, [128,128,128,192], 'drawn');
        "#,
        );
    }

    #[test]
    fn gl_clear_conformance_runs_through_the_harness() {
        let outcome = run_conformance("conformance/rendering/gl-clear.html");
        match outcome {
            HarnessOutcome::Ran(results) => {
                // The real Khronos gl-clear test, run end to end: js-test-pre +
                // webgl-test-utils + js-test-post loaded from disk, drawing
                // against wgpu, reporting via the testharness shim. We want it
                // to produce subtest results and for them to pass.
                assert!(
                    !results.is_empty(),
                    "gl-clear reported no subtests (harness ran but the test \
                     never called testPassed/testFailed)"
                );
                let failures: Vec<_> = results.iter().filter(|r| !r.passed()).collect();
                assert!(
                    failures.is_empty(),
                    "gl-clear had {} failing subtest(s): {:#?}",
                    failures.len(),
                    failures
                );
            },
            HarnessOutcome::Threw(msg) => {
                panic!("gl-clear threw before reporting: {msg}");
            },
        }
    }
}
