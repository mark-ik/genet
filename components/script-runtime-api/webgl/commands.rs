// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! WebGL native command sinks (one unit struct + `NativeFn` impl each).

use super::*;

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
        cx.make_string(
            &id.map(|i| i.to_string())
                .unwrap_or_else(|| "-1".to_string()),
        )
    }
}

pub(crate) struct ExternalTextureKey;
impl<E: ScriptEngine> NativeFn<E> for ExternalTextureKey {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let key = with_webgl_ctx::<E, _, _>(cx, ctx, None, |h| h.external_texture_key());
        cx.make_string(&key.map(|k| k.to_string()).unwrap_or_default())
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

pub(crate) struct Enable;
impl<E: ScriptEngine> NativeFn<E> for Enable {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let cap = parse_u32::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.enable(cap));
        Ok(cx.undefined())
    }
}

pub(crate) struct Disable;
impl<E: ScriptEngine> NativeFn<E> for Disable {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let cap = parse_u32::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.disable(cap));
        Ok(cx.undefined())
    }
}

pub(crate) struct IsEnabled;
impl<E: ScriptEngine> NativeFn<E> for IsEnabled {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let cap = parse_u32::<E>(cx, 1)?;
        let on = with_webgl_ctx::<E, _, _>(cx, ctx, false, |h| h.is_enabled(cap));
        cx.make_string(if on { "1" } else { "0" })
    }
}

pub(crate) struct ColorMask;
impl<E: ScriptEngine> NativeFn<E> for ColorMask {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let r = parse_bool::<E>(cx, 1)?;
        let g = parse_bool::<E>(cx, 2)?;
        let b = parse_bool::<E>(cx, 3)?;
        let a = parse_bool::<E>(cx, 4)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.color_mask(r, g, b, a));
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
        let log =
            with_webgl_ctx::<E, _, _>(cx, ctx, String::new(), |h| h.get_shader_info_log(shader));
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
        let log =
            with_webgl_ctx::<E, _, _>(cx, ctx, String::new(), |h| h.get_program_info_log(program));
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
        let loc =
            with_webgl_ctx::<E, _, _>(cx, ctx, -1, |h| h.get_uniform_location(program, &name));
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

pub(crate) struct Uniform1i;
impl<E: ScriptEngine> NativeFn<E> for Uniform1i {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let loc = parse_i32::<E>(cx, 1)?;
        let value = parse_i32::<E>(cx, 2)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.uniform1i(loc, value));
        Ok(cx.undefined())
    }
}

pub(crate) struct CreateTexture;
impl<E: ScriptEngine> NativeFn<E> for CreateTexture {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let id = with_webgl_ctx::<E, _, _>(cx, ctx, 0, |h| h.create_texture());
        cx.make_string(&id.to_string())
    }
}

pub(crate) struct BindTexture2d;
impl<E: ScriptEngine> NativeFn<E> for BindTexture2d {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let texture = parse_optional_u64::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.bind_texture_2d(texture));
        Ok(cx.undefined())
    }
}

pub(crate) struct ActiveTexture;
impl<E: ScriptEngine> NativeFn<E> for ActiveTexture {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let unit = parse_u32::<E>(cx, 1)?;
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| h.active_texture(unit));
        Ok(cx.undefined())
    }
}

pub(crate) struct TexImage2dRgba8;
impl<E: ScriptEngine> NativeFn<E> for TexImage2dRgba8 {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let ctx = parse_ctx::<E>(cx)?;
        let width = parse_u32::<E>(cx, 1)?;
        let height = parse_u32::<E>(cx, 2)?;
        // Pixels cross as a binary string (one JS char code per byte), the
        // same encoding readPixels uses in the other direction.
        let pixels_str = parse_string::<E>(cx, 3)?;
        let pixels: Vec<u8> = pixels_str.chars().map(|c| c as u8).collect();
        with_webgl_ctx::<E, _, _>(cx, ctx, (), |h| {
            h.tex_image_2d_rgba8(width, height, &pixels)
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
