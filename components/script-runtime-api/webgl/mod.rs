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
    /// Host-compositor texture key for this context's default framebuffer, when
    /// the embedder has registered the matching texture with the paint/compositor
    /// side. `None` means the context is script-visible only.
    fn external_texture_key(&self) -> Option<u64> {
        None
    }

    fn clear_color(&mut self, r: f32, g: f32, b: f32, a: f32);
    fn clear(&mut self, mask: u32);
    fn viewport(&mut self, x: i32, y: i32, width: u32, height: u32);

    /// `gl.enable(cap)` / `gl.disable(cap)`. `cap` is the raw GLenum
    /// (`DEPTH_TEST` = 0x0B71, `BLEND` = 0x0BE2, `CULL_FACE` = 0x0B44,
    /// `SCISSOR_TEST` = 0x0C11, ...). The host enables what its backend
    /// supports and records the rest so `is_enabled` round-trips.
    fn enable(&mut self, cap: u32);
    fn disable(&mut self, cap: u32);
    /// `gl.isEnabled(cap)` — the current enable/disable state of `cap`.
    fn is_enabled(&mut self, cap: u32) -> bool;
    /// `gl.colorMask(r, g, b, a)` — per-channel write enable for draws and
    /// clears.
    fn color_mask(&mut self, r: bool, g: bool, b: bool, a: bool);

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
    /// `gl.uniform1i` — used to bind a sampler uniform to a texture unit.
    fn uniform1i(&mut self, location: i32, value: i32);

    fn create_texture(&mut self) -> u64;
    fn bind_texture_2d(&mut self, texture: Option<u64>);
    fn active_texture(&mut self, unit: u32);
    /// `gl.texImage2D` for a 2D RGBA8 texture. `pixels` is `width * height *
    /// 4` bytes; an empty slice means "allocate, contents undefined".
    fn tex_image_2d_rgba8(&mut self, width: u32, height: u32, pixels: &[u8]);

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
fn with_webgl_ctx<E: ScriptEngine, F, R>(cx: &mut E::CallCx<'_>, ctx_id: i64, default: R, f: F) -> R
where
    F: FnOnce(&mut dyn WebGlHandler) -> R,
{
    if ctx_id < 0 {
        return default;
    }
    let Some(data) = cx.host_data() else {
        return default;
    };
    let Some(cell) = data.downcast_ref::<RefCell<HostState>>() else {
        return default;
    };
    let mut host = cell.borrow_mut();
    match host.webgl_contexts.get_mut(ctx_id as usize) {
        Some(h) => f(h.as_mut()),
        None => default,
    }
}

// =====================================================================

mod commands;

use commands::*;

// ---------------------------------------------------------------------
// arg parsing helpers
// ---------------------------------------------------------------------

/// Parse the context registry index from arg 0. Negative / unparseable yields
/// `-1`, which `with_webgl_ctx` treats as "no context".
fn parse_ctx<E: ScriptEngine>(cx: &mut E::CallCx<'_>) -> Result<i64, E::Error> {
    Ok(parse_string::<E>(cx, 0)?.parse::<i64>().unwrap_or(-1))
}

fn parse_string<E: ScriptEngine>(cx: &mut E::CallCx<'_>, index: usize) -> Result<String, E::Error> {
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
    engine.set_function::<ExternalTextureKey>("__webgl_external_texture_key", 1)?;
    engine.set_function::<ClearColor>("__webgl_clear_color", 5)?;
    engine.set_function::<Clear>("__webgl_clear", 2)?;
    engine.set_function::<Enable>("__webgl_enable", 2)?;
    engine.set_function::<Disable>("__webgl_disable", 2)?;
    engine.set_function::<IsEnabled>("__webgl_is_enabled", 2)?;
    engine.set_function::<ColorMask>("__webgl_color_mask", 5)?;
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
    engine.set_function::<Uniform1i>("__webgl_uniform1i", 3)?;
    engine.set_function::<CreateTexture>("__webgl_create_texture", 1)?;
    engine.set_function::<BindTexture2d>("__webgl_bind_texture_2d", 2)?;
    engine.set_function::<ActiveTexture>("__webgl_active_texture", 2)?;
    engine.set_function::<TexImage2dRgba8>("__webgl_tex_image_2d_rgba8", 4)?;
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
const WEBGL_BOOTSTRAP: &str = include_str!("bootstrap.js");
