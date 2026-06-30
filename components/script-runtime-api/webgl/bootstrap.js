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
    // Capabilities for enable/disable/isEnabled.
    CULL_FACE: 0x0B44, DEPTH_TEST: 0x0B71, BLEND: 0x0BE2,
    DITHER: 0x0BD0, SCISSOR_TEST: 0x0C11, STENCIL_TEST: 0x0B90,
    POLYGON_OFFSET_FILL: 0x8037, SAMPLE_ALPHA_TO_COVERAGE: 0x809E,
    SAMPLE_COVERAGE: 0x80A0,
    // Textures.
    TEXTURE_2D: 0x0DE1, TEXTURE_CUBE_MAP: 0x8513, RGBA: 0x1908, RGB: 0x1907,
    TEXTURE0: 0x84C0, TEXTURE1: 0x84C1, TEXTURE2: 0x84C2,
    TEXTURE_MIN_FILTER: 0x2801, TEXTURE_MAG_FILTER: 0x2800,
    TEXTURE_WRAP_S: 0x2802, TEXTURE_WRAP_T: 0x2803,
    NEAREST: 0x2600, LINEAR: 0x2601, CLAMP_TO_EDGE: 0x812F, REPEAT: 0x2901,
  };

  // -----------------------------------------------------------------
  // Lightweight reflector classes — each wraps a host-side id.
  // -----------------------------------------------------------------
  function WebGLBuffer(id) { this._id = id; }
  function WebGLShader(id) { this._id = id; }
  function WebGLProgram(id) { this._id = id; }
  function WebGLTexture(id) { this._id = id; }
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
    this._externalTextureKey = __webgl_external_texture_key(String(this._ctx));
  }
  var P = WebGLRenderingContext.prototype;

  // State / framebuffer.
  P.clearColor = function(r, g, b, a) {
    __webgl_clear_color(String(this._ctx), String(+r), String(+g), String(+b), String(+a));
  };
  P.clear = function(mask) { __webgl_clear(String(this._ctx), String(mask >>> 0)); };
  P.enable = function(cap) { __webgl_enable(String(this._ctx), String(cap >>> 0)); };
  P.disable = function(cap) { __webgl_disable(String(this._ctx), String(cap >>> 0)); };
  P.isEnabled = function(cap) {
    return __webgl_is_enabled(String(this._ctx), String(cap >>> 0)) === '1';
  };
  P.colorMask = function(r, g, b, a) {
    __webgl_color_mask(String(this._ctx),
      r ? '1' : '0', g ? '1' : '0', b ? '1' : '0', a ? '1' : '0');
  };
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
  P.uniform1i = function(loc, v) {
    if (loc == null) return;
    __webgl_uniform1i(String(this._ctx), String(loc._loc | 0), String(v | 0));
  };

  // Textures.
  P.createTexture = function() {
    return new WebGLTexture(__webgl_create_texture(String(this._ctx)));
  };
  P.bindTexture = function(target, tex) {
    // Only TEXTURE_2D is wired through this layer; cube maps land with
    // the broader texture conformance.
    if ((target >>> 0) !== K.TEXTURE_2D) return;
    __webgl_bind_texture_2d(String(this._ctx), idOrZero(tex));
  };
  P.activeTexture = function(unit) {
    // unit is TEXTURE0 + n; the host wants the 0-based index.
    __webgl_active_texture(String(this._ctx), String(((unit >>> 0) - K.TEXTURE0) >>> 0));
  };
  // texParameteri is accepted but a no-op: webgl-wgpu samples NEAREST /
  // CLAMP_TO_EDGE today, which matches the conformance smoke's needs.
  P.texParameteri = function(target, pname, param) {};
  P.texImage2D = function() {
    // Support the 9-arg pixel-store form
    //   texImage2D(target, level, internalformat, width, height, border,
    //              format, type, pixels)
    // with an RGBA / UNSIGNED_BYTE Uint8Array (or null for an allocate).
    var a = arguments;
    if (a.length < 9) {
      throw new TypeError('texImage2D: only the 9-arg RGBA8 form is supported here');
    }
    if ((a[0] >>> 0) !== K.TEXTURE_2D) return;
    var width = a[3] >>> 0, height = a[4] >>> 0, pixels = a[8];
    var bin = '';
    if (pixels != null) {
      var n = pixels.length | 0;
      var arr = new Array(n);
      for (var i = 0; i < n; i++) arr[i] = String.fromCharCode(pixels[i] & 0xFF);
      bin = arr.join('');
    }
    __webgl_tex_image_2d_rgba8(String(this._ctx), String(width), String(height), bin);
  };

  // Draw.
  P.drawArrays = function(mode, first, count) {
    __webgl_draw_arrays(String(this._ctx), String(mode >>> 0), String(first | 0), String(count | 0));
  };

  // -----------------------------------------------------------------
  // Best-effort conveniences the Khronos test utilities reach for.
  // None have a backend effect yet; they exist so create3DContext /
  // setupProgram / setupTexturedQuad run instead of throwing on a
  // missing method.
  // -----------------------------------------------------------------
  // No extensions are exposed at this layer.
  P.getExtension = function(name) { return null; };
  P.getSupportedExtensions = function() { return []; };
  // getParameter: the conformance utils mostly read this behind feature
  // gates the smoke doesn't hit. Return null (unknown) for now; specific
  // pnames get real values as conformance needs them.
  P.getParameter = function(pname) { return null; };
  // webgl-essl assigns attribute locations in declaration order, which
  // matches how the utils bind them (0, 1, ...), so this is a safe no-op.
  P.bindAttribLocation = function(program, index, name) {};
  // Sampler/pixel-store parameters: we sample NEAREST / CLAMP and treat
  // RGBA8 tightly-packed, which covers the smoke.
  P.texParameterf = function(target, pname, param) {};
  P.pixelStorei = function(pname, param) {};
  // Object deletion: the backend reclaims on context drop; explicit
  // deletes are accepted as no-ops so teardown code runs.
  P.deleteProgram = function(p) {};
  P.deleteShader = function(s) {};
  P.deleteBuffer = function(b) {};
  P.deleteTexture = function(t) {};
  // Resource liveness predicates default to true for live handles.
  P.isProgram = function(p) { return p != null && p._id != null; };
  P.isShader = function(s) { return s != null && s._id != null; };
  P.isBuffer = function(b) { return b != null && b._id != null; };
  P.isTexture = function(t) { return t != null && t._id != null; };
  // Cooperative scheduling hooks some tests call; harmless here.
  P.flush = function() {};
  P.finish = function() {};

  // Constructors on the global so tests can `instanceof` them.
  globalThis.WebGLRenderingContext = WebGLRenderingContext;
  globalThis.WebGLBuffer = WebGLBuffer;
  globalThis.WebGLShader = WebGLShader;
  globalThis.WebGLProgram = WebGLProgram;
  globalThis.WebGLTexture = WebGLTexture;
  globalThis.WebGLUniformLocation = WebGLUniformLocation;

  // Host / test helper: a bare context at the given drawing-buffer size
  // (defaults to the HTML canvas 300x150). The host must have installed a
  // WebGlFactory via Runtime::set_webgl_factory; otherwise _ctx is -1 and
  // every sink no-ops / returns 0 / NO_ERROR.
  globalThis.__createWebGLContext = function(width, height) {
    return new WebGLRenderingContext(width, height);
  };
})();
