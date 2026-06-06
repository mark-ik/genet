// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `WebGLRenderingContext` host seam.
//!
//! The runtime exposes a [`WebGlHandler`] trait — one instance is one WebGL
//! context — plus a *factory* the host installs ([`crate::Runtime::set_webgl_factory`])
//! that mints a fresh handler per `<canvas>.getContext('webgl')`. The JS
//! `WebGLRenderingContext` surface is a bootstrap over a set of native sinks
//! (`__webgl_*`). No graphics dependency enters this crate; only the trait does.
//!
//! Multi-context: each `getContext` call invokes the factory with the canvas's
//! width/height and pushes the resulting handler into a per-runtime registry;
//! the JS context object carries the registry index (`_ctx`), and every sink
//! call passes it as its first argument so the sink routes to the right
//! handler. A negative / out-of-range index (no factory installed, or a stale
//! context) makes the sink no-op or return its default.

use std::cell::RefCell;

use script_engine_api::{CallCx, NativeFn, ScriptEngine};

use crate::HostState;

/// Mints a fresh [`WebGlHandler`] (one WebGL context) at the given drawing-buffer
/// `width` x `height`. The host installs one via
/// [`crate::Runtime::set_webgl_factory`]; each `getContext('webgl')` calls it.
pub type WebGlFactory = Box<dyn FnMut(u32, u32) -> Box<dyn WebGlHandler>>;

/// What a Triangle-class WebGL smoke needs. Each method maps to one
/// `WebGLRenderingContext` JS method; arguments come pre-decoded (GLenum
/// constants are still raw `u32` so the host owns the meaning of, e.g.,
/// `gl.ARRAY_BUFFER`).
///
/// Resource ids cross the JS/host seam as `u64`. The host owns the allocation
/// (each `create_*` returns a fresh id) and is responsible for translating them
/// back into its native `wgpu` handles. An `Option<u64>` argument means
/// "`null`" from JS — typically "unbind".
pub trait WebGlHandler {
    fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32);
    fn clear(&mut self, mask: u32);
    fn viewport(&mut self, x: i32, y: i32, width: u32, height: u32);

    fn create_buffer(&mut self) -> u64;
    fn bind_buffer(&mut self, target: u32, buffer: Option<u64>);
    fn buffer_data_f32(&mut self, target: u32, data: &[f32], usage: u32);

    fn create_shader(&mut self, stage: u32) -> u64;
    fn shader_source(&mut self, shader: u64, source: &str);
    fn compile_shader(&mut self, shader: u64);
    fn get_shader_compile_status(&mut self, shader: u64) -> bool;
    fn get_shader_info_log(&mut self, shader: u64) -> String;

    fn create_program(&mut self) -> u64;
    fn attach_shader(&mut self, program: u64, shader: u64);
    fn link_program(&mut self, program: u64);
    fn get_program_link_status(&mut self, program: u64) -> bool;
    fn get_program_info_log(&mut self, program: u64) -> String;
    fn use_program(&mut self, program: Option<u64>);

    fn get_attrib_location(&mut self, program: u64, name: &str) -> i32;
    /// Returns an opaque non-negative location id (the host's own encoding) or
    /// `-1` if `name` does not name a uniform. The JS wrapper translates that
    /// into `WebGLUniformLocation` / `null`.
    fn get_uniform_location(&mut self, program: u64, name: &str) -> i32;

    fn enable_vertex_attrib_array(&mut self, index: u32);
    fn vertex_attrib_pointer_f32(
        &mut self,
        index: u32,
        size: u32,
        normalized: bool,
        stride: u32,
        offset: u32,
    );

    fn uniform4f(&mut self, location: i32, x: f32, y: f32, z: f32, w: f32);
    fn uniform_matrix4fv(&mut self, location: i32, transpose: bool, value: &[f32]);

    fn draw_arrays(&mut self, mode: u32, first: i32, count: i32);

    /// `0` is `gl.NO_ERROR`; non-zero is a GLenum (`INVALID_ENUM` etc.). The
    /// host clears its pending error on read, like WebGL.
    fn get_error(&mut self) -> u32;

    /// Read `width * height` RGBA8 pixels from the default framebuffer at
    /// `(x, y)`. Bytes-per-pixel = 4. The returned `Vec` is `width * height * 4`
    /// long.
    fn read_pixels_rgba8(&mut self, x: i32, y: i32, width: u32, height: u32) -> Vec<u8>;
}

/// Invoke the installed factory to mint a context at `width` x `height`, push it
/// into the registry, and return its index. `None` if no factory is installed.
fn create_webgl_context<E: ScriptEngine>(
    cx: &mut E::CallCx<'_>,
    width: u32,
    height: u32,
) -> Option<usize> {
    let data = cx.host_data()?;
    let cell = data.downcast_ref::<RefCell<HostState>>()?;
    let mut host = cell.borrow_mut();
    // Split the borrow so the FnMut factory and the registry Vec can be held
    // mutably at the same time.
    let HostState {
        webgl_factory,
        webgl_contexts,
        ..
    } = &mut *host;
    let factory = webgl_factory.as_mut()?;
    let handler = factory(width, height);
    webgl_contexts.push(handler);
    Some(webgl_contexts.len() - 1)
}

/// Route to the context at registry index `ctx_id`. A negative index (no
/// factory / stale context) or an out-of-range one yields `default` — the sink
/// no-ops, matching the "no handler" behavior of the singleton era.
fn with_webgl_ctx<E: ScriptEngine, F, R>(
    cx: &mut E::CallCx<'_>,
    ctx_id: i64,
    default: R,
    f: F,
) -> R
where
    F: FnOnce(&mut dyn WebGlHandler) -> R,
{
    if ctx_id < 0 {
        return default;
    }
    let Some(data) = cx.host_data() else { return default };
    let Some(cell) = data.downcast_ref::<RefCell<HostState>>() else { return default };
    let mut host = cell.borrow_mut();
    match host.webgl_contexts.get_mut(ctx_id as usize) {
        Some(h) => f(h.as_mut()),
        None => default,
    }
}

// =====================================================================
// Native sinks. Each is a unit struct + a NativeFn<E> impl; the JS
// bootstrap calls them as `__webgl_<name>`. Every sink takes the
// context registry index as arg 0; the remaining args follow.
// =====================================================================

pub(crate) struct CreateContext;
impl<E: ScriptEngine> NativeFn<E> for CreateContext {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let width = parse_u32::<E>(cx, 0)?;
        let height = parse_u32::<E>(cx, 1)?;
        let id = create_webgl_context::<E>(cx, width, height);
        cx.make_string(&id.map(|i| i.to_string()).unwrap_or_else(|| "-1".to_string()))
    }
}

pub(crate) struct ClearColor;
impl<E: ScriptEngine> NativeFn<E> for ClearColor {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let r = parse_f32::<E>(cx, 1)?;
        let g = parse_f32::<E>(cx, 2)?;
        let b = parse_f32::<E>(cx, 3)?;
        let a = parse_f32::<E>(cx, 4)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.clear_color(r, g, b, a));
        Ok(cx.undefined())
    }
}

pub(crate) struct Clear;
impl<E: ScriptEngine> NativeFn<E> for Clear {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let mask = parse_u32::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.clear(mask));
        Ok(cx.undefined())
    }
}

pub(crate) struct Viewport;
impl<E: ScriptEngine> NativeFn<E> for Viewport {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let x = parse_i32::<E>(cx, 1)?;
        let y = parse_i32::<E>(cx, 2)?;
        let w = parse_u32::<E>(cx, 3)?;
        let h = parse_u32::<E>(cx, 4)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |handler| handler.viewport(x, y, w, h));
        Ok(cx.undefined())
    }
}

pub(crate) struct CreateBuffer;
impl<E: ScriptEngine> NativeFn<E> for CreateBuffer {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let id = with_webgl_ctx::<E, _, _>(cx, ctx, 0, |h| h.create_buffer());
        cx.make_string(&id.to_string())
    }
}

pub(crate) struct BindBuffer;
impl<E: ScriptEngine> NativeFn<E> for BindBuffer {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let target = parse_u32::<E>(cx, 1)?;
        let buffer = parse_optional_u64::<E>(cx, 2)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.bind_buffer(target, buffer));
        Ok(cx.undefined())
    }
}

pub(crate) struct BufferDataF32;
impl<E: ScriptEngine> NativeFn<E> for BufferDataF32 {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let target = parse_u32::<E>(cx, 1)?;
        // The JS wrapper serializes the Float32Array as a comma-separated
        // list (small enough for the conformance smoke; the production path
        // can switch to a binary-string when large geometry hits).
        let data_str = parse_string::<E>(cx, 2)?;
        let data = parse_f32_list(&data_str);
        let usage = parse_u32::<E>(cx, 3)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.buffer_data_f32(target, &data, usage));
        Ok(cx.undefined())
    }
}

pub(crate) struct CreateShader;
impl<E: ScriptEngine> NativeFn<E> for CreateShader {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let stage = parse_u32::<E>(cx, 1)?;
        let id = with_webgl_ctx::<E, _, _>(cx, ctx, 0, |h| h.create_shader(stage));
        cx.make_string(&id.to_string())
    }
}

pub(crate) struct ShaderSource;
impl<E: ScriptEngine> NativeFn<E> for ShaderSource {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let shader = parse_u64::<E>(cx, 1)?;
        let source = parse_string::<E>(cx, 2)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.shader_source(shader, &source));
        Ok(cx.undefined())
    }
}

pub(crate) struct CompileShader;
impl<E: ScriptEngine> NativeFn<E> for CompileShader {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let shader = parse_u64::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.compile_shader(shader));
        Ok(cx.undefined())
    }
}

pub(crate) struct GetShaderCompileStatus;
impl<E: ScriptEngine> NativeFn<E> for GetShaderCompileStatus {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let shader = parse_u64::<E>(cx, 1)?;
        let ok = with_webgl_ctx::<E, _, _>(cx, ctx, false, |h| h.get_shader_compile_status(shader));
        cx.make_string(if ok { "1" } else { "0" })
    }
}

pub(crate) struct GetShaderInfoLog;
impl<E: ScriptEngine> NativeFn<E> for GetShaderInfoLog {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let shader = parse_u64::<E>(cx, 1)?;
        let log = with_webgl_ctx::<E, _, _>(cx, ctx, String::new(), |h| {
            h.get_shader_info_log(shader)
        });
        cx.make_string(&log)
    }
}

pub(crate) struct CreateProgram;
impl<E: ScriptEngine> NativeFn<E> for CreateProgram {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let id = with_webgl_ctx::<E, _, _>(cx, ctx, 0, |h| h.create_program());
        cx.make_string(&id.to_string())
    }
}

pub(crate) struct AttachShader;
impl<E: ScriptEngine> NativeFn<E> for AttachShader {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        let shader = parse_u64::<E>(cx, 2)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.attach_shader(program, shader));
        Ok(cx.undefined())
    }
}

pub(crate) struct LinkProgram;
impl<E: ScriptEngine> NativeFn<E> for LinkProgram {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.link_program(program));
        Ok(cx.undefined())
    }
}

pub(crate) struct GetProgramLinkStatus;
impl<E: ScriptEngine> NativeFn<E> for GetProgramLinkStatus {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        let ok = with_webgl_ctx::<E, _, _>(cx, ctx, false, |h| h.get_program_link_status(program));
        cx.make_string(if ok { "1" } else { "0" })
    }
}

pub(crate) struct GetProgramInfoLog;
impl<E: ScriptEngine> NativeFn<E> for GetProgramInfoLog {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        let log = with_webgl_ctx::<E, _, _>(cx, ctx, String::new(), |h| {
            h.get_program_info_log(program)
        });
        cx.make_string(&log)
    }
}

pub(crate) struct UseProgram;
impl<E: ScriptEngine> NativeFn<E> for UseProgram {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_optional_u64::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.use_program(program));
        Ok(cx.undefined())
    }
}

pub(crate) struct GetAttribLocation;
impl<E: ScriptEngine> NativeFn<E> for GetAttribLocation {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        let name = parse_string::<E>(cx, 2)?;
        let loc = with_webgl_ctx::<E, _, _>(cx, ctx, -1, |h| h.get_attrib_location(program, &name));
        cx.make_string(&loc.to_string())
    }
}

pub(crate) struct GetUniformLocation;
impl<E: ScriptEngine> NativeFn<E> for GetUniformLocation {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let program = parse_u64::<E>(cx, 1)?;
        let name = parse_string::<E>(cx, 2)?;
        let loc = with_webgl_ctx::<E, _, _>(cx, ctx, -1, |h| h.get_uniform_location(program, &name));
        cx.make_string(&loc.to_string())
    }
}

pub(crate) struct EnableVertexAttribArray;
impl<E: ScriptEngine> NativeFn<E> for EnableVertexAttribArray {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let index = parse_u32::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.enable_vertex_attrib_array(index));
        Ok(cx.undefined())
    }
}

pub(crate) struct VertexAttribPointer;
impl<E: ScriptEngine> NativeFn<E> for VertexAttribPointer {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let index = parse_u32::<E>(cx, 1)?;
        let size = parse_u32::<E>(cx, 2)?;
        let normalized = parse_bool::<E>(cx, 3)?;
        let stride = parse_u32::<E>(cx, 4)?;
        let offset = parse_u32::<E>(cx, 5)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |handler| {
            handler.vertex_attrib_pointer_f32(index, size, normalized, stride, offset)
        });
        Ok(cx.undefined())
    }
}

pub(crate) struct Uniform4f;
impl<E: ScriptEngine> NativeFn<E> for Uniform4f {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let loc = parse_i32::<E>(cx, 1)?;
        let x = parse_f32::<E>(cx, 2)?;
        let y = parse_f32::<E>(cx, 3)?;
        let z = parse_f32::<E>(cx, 4)?;
        let w = parse_f32::<E>(cx, 5)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.uniform4f(loc, x, y, z, w));
        Ok(cx.undefined())
    }
}

pub(crate) struct UniformMatrix4fv;
impl<E: ScriptEngine> NativeFn<E> for UniformMatrix4fv {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let loc = parse_i32::<E>(cx, 1)?;
        let transpose = parse_bool::<E>(cx, 2)?;
        let values_str = parse_string::<E>(cx, 3)?;
        let values = parse_f32_list(&values_str);
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |handler| {
            handler.uniform_matrix4fv(loc, transpose, &values)
        });
        Ok(cx.undefined())
    }
}

pub(crate) struct DrawArrays;
impl<E: ScriptEngine> NativeFn<E> for DrawArrays {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let mode = parse_u32::<E>(cx, 1)?;
        let first = parse_i32::<E>(cx, 2)?;
        let count = parse_i32::<E>(cx, 3)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.draw_arrays(mode, first, count));
        Ok(cx.undefined())
    }
}

pub(crate) struct GetError;
impl<E: ScriptEngine> NativeFn<E> for GetError {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let err = with_webgl_ctx::<E, _, _>(cx, ctx, 0, |h| h.get_error());
        cx.make_string(&err.to_string())
    }
}

pub(crate) struct ReadPixelsRgba8;
impl<E: ScriptEngine> NativeFn<E> for ReadPixelsRgba8 {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let x = parse_i32::<E>(cx, 1)?;
        let y = parse_i32::<E>(cx, 2)?;
        let w = parse_u32::<E>(cx, 3)?;
        let h = parse_u32::<E>(cx, 4)?;
        let pixels = with_webgl_ctx::<E, _, _>(cx, ctx, Vec::new(), |handler| {
            handler.read_pixels_rgba8(x, y, w, h)
        });
        // Cross as a binary string: one JS char code per byte (0-255). The
        // JS wrapper unpacks into the caller's Uint8Array.
        cx.make_string(&binary_string(&pixels))
    }
}

// ---------------------------------------------------------------------
// arg parsing helpers
// ---------------------------------------------------------------------

/// Parse the context registry index from arg 0. Negative / unparseable yields
/// `-1`, which `with_webgl_ctx` treats as "no context".
fn parse_ctx<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Result<i64, E::Error> {
    Ok(parse_string::<E>(cx, 0)?.parse::<i64>().unwrap_or(-1))
}

fn parse_string<E: ScriptEngine>(
    cx: &mut E::CallCx<'_>,
    index: usize,
) -> Result<String, E::Error> {
    let a = cx.arg(index);
    cx.value_to_string(&a)
}

fn parse_f32<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<f32, E::Error> {
    Ok(parse_string::<E>(cx, index)?.parse().unwrap_or(0.0))
}

fn parse_i32<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<i32, E::Error> {
    Ok(parse_string::<E>(cx, index)?
        .parse::<f64>()
        .map(|v| v as i32)
        .unwrap_or(0))
}

fn parse_u32<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<u32, E::Error> {
    Ok(parse_string::<E>(cx, index)?
        .parse::<f64>()
        .map(|v| v.max(0.0) as u32)
        .unwrap_or(0))
}

fn parse_u64<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<u64, E::Error> {
    Ok(parse_string::<E>(cx, index)?.parse().unwrap_or(0))
}

fn parse_optional_u64<E: ScriptEngine>(
    cx: &mut E::CallCx<'_>,
    index: usize,
) -> Result<Option<u64>, E::Error> {
    let s = parse_string::<E>(cx, index)?;
    if s.is_empty() || s == "0" {
        Ok(None)
    } else {
        Ok(s.parse().ok())
    }
}

fn parse_bool<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<bool, E::Error> {
    let s = parse_string::<E>(cx, index)?;
    Ok(matches!(s.as_str(), "1" | "true"))
}

fn parse_f32_list(input: &str) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    input.split(',').filter_map(|s| s.parse().ok()).collect()
}

fn binary_string(bytes: &[u8]) -> String {
    bytes.iter().map(|b| char::from(*b)).collect()
}

/// Install `__webgl_*` sinks and the `WebGLRenderingContext` JS bootstrap.
/// Every sink's arg count includes the leading context-id argument.
pub(crate) fn install_webgl_surface<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<CreateContext>("__webgl_create_context", 2)?;
    engine.set_function::<ClearColor>("__webgl_clear_color", 5)?;
    engine.set_function::<Clear>("__webgl_clear", 2)?;
    engine.set_function::<Viewport>("__webgl_viewport", 5)?;
    engine.set_function::<CreateBuffer>("__webgl_create_buffer", 1)?;
    engine.set_function::<BindBuffer>("__webgl_bind_buffer", 3)?;
    engine.set_function::<BufferDataF32>("__webgl_buffer_data_f32", 4)?;
    engine.set_function::<CreateShader>("__webgl_create_shader", 2)?;
    engine.set_function::<ShaderSource>("__webgl_shader_source", 3)?;
    engine.set_function::<CompileShader>("__webgl_compile_shader", 2)?;
    engine.set_function::<GetShaderCompileStatus>("__webgl_get_shader_compile_status", 2)?;
    engine.set_function::<GetShaderInfoLog>("__webgl_get_shader_info_log", 2)?;
    engine.set_function::<CreateProgram>("__webgl_create_program", 1)?;
    engine.set_function::<AttachShader>("__webgl_attach_shader", 3)?;
    engine.set_function::<LinkProgram>("__webgl_link_program", 2)?;
    engine.set_function::<GetProgramLinkStatus>("__webgl_get_program_link_status", 2)?;
    engine.set_function::<GetProgramInfoLog>("__webgl_get_program_info_log", 2)?;
    engine.set_function::<UseProgram>("__webgl_use_program", 2)?;
    engine.set_function::<GetAttribLocation>("__webgl_get_attrib_location", 3)?;
    engine.set_function::<GetUniformLocation>("__webgl_get_uniform_location", 3)?;
    engine.set_function::<EnableVertexAttribArray>("__webgl_enable_vertex_attrib_array", 2)?;
    engine.set_function::<VertexAttribPointer>("__webgl_vertex_attrib_pointer", 6)?;
    engine.set_function::<Uniform4f>("__webgl_uniform4f", 6)?;
    engine.set_function::<UniformMatrix4fv>("__webgl_uniform_matrix4fv", 4)?;
    engine.set_function::<DrawArrays>("__webgl_draw_arrays", 4)?;
    engine.set_function::<GetError>("__webgl_get_error", 1)?;
    engine.set_function::<ReadPixelsRgba8>("__webgl_read_pixels_rgba8", 5)?;
    engine.eval(WEBGL_BOOTSTRAP)?;
    Ok(())
}

/// The WebGL JS surface. Defines `WebGLRenderingContext`, `WebGLBuffer`,
/// `WebGLShader`, `WebGLProgram`, `WebGLUniformLocation` constructors + the
/// GLenum constants documented in the WebGL 1.0 spec (the subset the
/// Triangle-class smoke uses; broader conformance lands as the trait grows).
///
/// Each `WebGLRenderingContext` carries a registry index (`_ctx`) minted by
/// `__webgl_create_context(width, height)`; every method threads it as the
/// leading sink argument. `HTMLCanvasElement.getContext('webgl')` (in dom.rs)
/// constructs one with the canvas's drawing-buffer size; the
/// `__createWebGLContext(w, h)` helper is for host code / tests that want a
/// bare context.
const WEBGL_BOOTSTRAP: &str = r#"
(function() {
  // -----------------------------------------------------------------
  // GLenum constants. Numbers only — JS spec values; the host
  // re-maps them to its own enums.
  // -----------------------------------------------------------------
  var K = {
    DEPTH_BUFFER_BIT: 0x0100, STENCIL_BUFFER_BIT: 0x0400, COLOR_BUFFER_BIT: 0x4000,
    POINTS: 0x0000, LINES: 0x0001, LINE_LOOP: 0x0002, LINE_STRIP: 0x0003,
    TRIANGLES: 0x0004, TRIANGLE_STRIP: 0x0005, TRIANGLE_FAN: 0x0006,
    ARRAY_BUFFER: 0x8892, ELEMENT_ARRAY_BUFFER: 0x8893,
    STATIC_DRAW: 0x88E4, DYNAMIC_DRAW: 0x88E8, STREAM_DRAW: 0x88E0,
    FLOAT: 0x1406, INT: 0x1404, UNSIGNED_BYTE: 0x1401, UNSIGNED_SHORT: 0x1403,
    VERTEX_SHADER: 0x8B31, FRAGMENT_SHADER: 0x8B30,
    COMPILE_STATUS: 0x8B81, LINK_STATUS: 0x8B82,
    NO_ERROR: 0x0000, INVALID_ENUM: 0x0500, INVALID_VALUE: 0x0501,
    INVALID_OPERATION: 0x0502, INVALID_FRAMEBUFFER_OPERATION: 0x0506,
    CONTEXT_LOST_WEBGL: 0x9242,
  };

  // -----------------------------------------------------------------
  // Lightweight reflector classes — each wraps a host-side id.
  // -----------------------------------------------------------------
  function WebGLBuffer(id) { this._id = id; }
  function WebGLShader(id) { this._id = id; }
  function WebGLProgram(id) { this._id = id; }
  function WebGLUniformLocation(loc) { this._loc = loc; }

  // -----------------------------------------------------------------
  // Float32Array helpers (the conformance JS uses typed arrays
  // throughout). The "is it typed?" check is duck-typed because
  // we may not have Float32Array constructor identity preserved
  // across the engine boundary.
  // -----------------------------------------------------------------
  function asFloatList(v) {
    if (v == null) return '';
    var n = v.length | 0;
    var parts = new Array(n);
    for (var i = 0; i < n; i++) parts[i] = String(v[i]);
    return parts.join(',');
  }
  function unpackBinary(s) {
    var n = s.length | 0;
    var out = new Uint8Array(n);
    for (var i = 0; i < n; i++) out[i] = s.charCodeAt(i) & 0xFF;
    return out;
  }
  function idOrZero(o) {
    if (o == null) return '0';
    if (typeof o === 'object' && o._id != null) return String(o._id);
    return '0';
  }

  // -----------------------------------------------------------------
  // WebGLRenderingContext: thin, the Triangle-class subset. Each
  // instance carries `_ctx` (the host registry index) threaded as
  // arg 0 of every sink. Methods that don't fit the surface throw
  // (so test failures are loud).
  // -----------------------------------------------------------------
  function WebGLRenderingContext(width, height) {
    // GLenum constants on the instance, per the WebGL IDL.
    for (var k in K) { if (K.hasOwnProperty(k)) this[k] = K[k]; }
    var w = (width >>> 0) || 300;   // HTML canvas default drawing-buffer size
    var h = (height >>> 0) || 150;
    this._ctx = parseInt(__webgl_create_context(String(w), String(h)), 10) | 0;
    this.drawingBufferWidth = w;
    this.drawingBufferHeight = h;
  }
  var P = WebGLRenderingContext.prototype;

  // State / framebuffer.
  P.clearColor = function(r, g, b, a) {
    __webgl_clear_color(String(this._ctx), String(+r), String(+g), String(+b), String(+a));
  };
  P.clear = function(mask) { __webgl_clear(String(this._ctx), String(mask >>> 0)); };
  P.viewport = function(x, y, w, h) {
    __webgl_viewport(String(this._ctx), String(x|0), String(y|0), String(w>>>0), String(h>>>0));
  };
  P.getError = function() { return parseInt(__webgl_get_error(String(this._ctx)), 10) | 0; };
  P.readPixels = function(x, y, w, h, format, type, dst) {
    // The smoke uses RGBA / UNSIGNED_BYTE only — a richer pixel-pack
    // path lands with the broader read-back conformance.
    var packed = __webgl_read_pixels_rgba8(
      String(this._ctx), String(x|0), String(y|0), String(w>>>0), String(h>>>0));
    var bytes = unpackBinary(packed);
    if (dst) {
      var n = Math.min(dst.length | 0, bytes.length | 0);
      for (var i = 0; i < n; i++) dst[i] = bytes[i];
    }
    return bytes;
  };

  // Buffers.
  P.createBuffer = function() {
    return new WebGLBuffer(__webgl_create_buffer(String(this._ctx)));
  };
  P.bindBuffer = function(target, buf) {
    __webgl_bind_buffer(String(this._ctx), String(target >>> 0), idOrZero(buf));
  };
  P.bufferData = function(target, srcOrSize, usage) {
    var floats;
    if (typeof srcOrSize === 'number') {
      floats = new Array(srcOrSize >>> 2).fill(0);
    } else {
      floats = srcOrSize;
    }
    __webgl_buffer_data_f32(
      String(this._ctx),
      String(target >>> 0),
      asFloatList(floats),
      String((usage >>> 0) || K.STATIC_DRAW)
    );
  };

  // Shaders.
  P.createShader = function(stage) {
    return new WebGLShader(__webgl_create_shader(String(this._ctx), String(stage >>> 0)));
  };
  P.shaderSource = function(shader, source) {
    __webgl_shader_source(String(this._ctx), idOrZero(shader), String(source || ''));
  };
  P.compileShader = function(shader) {
    __webgl_compile_shader(String(this._ctx), idOrZero(shader));
  };
  P.getShaderParameter = function(shader, pname) {
    if (pname === K.COMPILE_STATUS) {
      return __webgl_get_shader_compile_status(String(this._ctx), idOrZero(shader)) === '1';
    }
    return null;
  };
  P.getShaderInfoLog = function(shader) {
    return __webgl_get_shader_info_log(String(this._ctx), idOrZero(shader));
  };

  // Programs.
  P.createProgram = function() {
    return new WebGLProgram(__webgl_create_program(String(this._ctx)));
  };
  P.attachShader = function(program, shader) {
    __webgl_attach_shader(String(this._ctx), idOrZero(program), idOrZero(shader));
  };
  P.linkProgram = function(program) {
    __webgl_link_program(String(this._ctx), idOrZero(program));
  };
  P.getProgramParameter = function(program, pname) {
    if (pname === K.LINK_STATUS) {
      return __webgl_get_program_link_status(String(this._ctx), idOrZero(program)) === '1';
    }
    return null;
  };
  P.getProgramInfoLog = function(program) {
    return __webgl_get_program_info_log(String(this._ctx), idOrZero(program));
  };
  P.useProgram = function(program) {
    __webgl_use_program(String(this._ctx), idOrZero(program));
  };

  // Attributes / uniforms.
  P.getAttribLocation = function(program, name) {
    var s = __webgl_get_attrib_location(String(this._ctx), idOrZero(program), String(name || ''));
    return parseInt(s, 10) | 0;
  };
  P.getUniformLocation = function(program, name) {
    var s = __webgl_get_uniform_location(String(this._ctx), idOrZero(program), String(name || ''));
    var loc = parseInt(s, 10) | 0;
    return loc < 0 ? null : new WebGLUniformLocation(loc);
  };
  P.enableVertexAttribArray = function(index) {
    __webgl_enable_vertex_attrib_array(String(this._ctx), String(index >>> 0));
  };
  P.vertexAttribPointer = function(index, size, type, normalized, stride, offset) {
    // type is always FLOAT for the Triangle-class smoke; the broader
    // surface gates on it.
    if ((type >>> 0) !== K.FLOAT) {
      throw new TypeError('vertexAttribPointer: only FLOAT supported at this layer');
    }
    __webgl_vertex_attrib_pointer(
      String(this._ctx), String(index >>> 0), String(size >>> 0),
      normalized ? '1' : '0',
      String(stride >>> 0), String(offset >>> 0)
    );
  };
  P.uniform4f = function(loc, x, y, z, w) {
    if (loc == null) return;
    __webgl_uniform4f(String(this._ctx), String(loc._loc | 0),
      String(+x), String(+y), String(+z), String(+w));
  };
  P.uniformMatrix4fv = function(loc, transpose, value) {
    if (loc == null) return;
    __webgl_uniform_matrix4fv(String(this._ctx), String(loc._loc | 0),
      transpose ? '1' : '0', asFloatList(value));
  };

  // Draw.
  P.drawArrays = function(mode, first, count) {
    __webgl_draw_arrays(String(this._ctx), String(mode >>> 0), String(first | 0), String(count | 0));
  };

  // Constructors on the global so tests can `instanceof` them.
  globalThis.WebGLRenderingContext = WebGLRenderingContext;
  globalThis.WebGLBuffer = WebGLBuffer;
  globalThis.WebGLShader = WebGLShader;
  globalThis.WebGLProgram = WebGLProgram;
  globalThis.WebGLUniformLocation = WebGLUniformLocation;

  // Host / test helper: a bare context at the given drawing-buffer size
  // (defaults to the HTML canvas 300x150). The host must have installed a
  // WebGlFactory via Runtime::set_webgl_factory; otherwise _ctx is -1 and
  // every sink no-ops / returns 0 / NO_ERROR.
  globalThis.__createWebGLContext = function(width, height) {
    return new WebGLRenderingContext(width, height);
  };
})();
"#;
