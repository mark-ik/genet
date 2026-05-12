/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::fmt;

/// Canonical ESSL 1.00 vertex shader accepted by the W3 smoke.
pub const CANONICAL_TRIANGLE_VERTEX_SHADER: &str = r#"
attribute vec2 a_position;
void main() {
    gl_Position = vec4(a_position, 0.0, 1.0);
}
"#;

/// Canonical ESSL 1.00 fragment shader accepted by the W3 smoke.
pub const CANONICAL_TRIANGLE_FRAGMENT_SHADER: &str = r#"
precision mediump float;
void main() {
    gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
}
"#;

const CANONICAL_VERTEX_PREFIX: &str = "attribute vec2 ";
const CANONICAL_VERTEX_MIDDLE: &str = ";void main(){gl_Position=vec4(";
const CANONICAL_VERTEX_SUFFIX: &str = ",0.0,1.0);}";
const CANONICAL_VARYING_VERTEX_PREFIX: &str = "attribute vec2 ";
const CANONICAL_VARYING_VERTEX_COLOR_DECL: &str = ";attribute vec4 ";
const CANONICAL_VARYING_VERTEX_VARYING_DECL: &str = ";varying vec4 ";
const CANONICAL_VARYING_VERTEX_MAIN_PREFIX: &str = ";void main(){";
const CANONICAL_VARYING_VERTEX_ASSIGN_MIDDLE: &str = "=";
const CANONICAL_VARYING_VERTEX_POSITION_PREFIX: &str = ";gl_Position=vec4(";
const CANONICAL_VARYING_VERTEX_SUFFIX: &str = ",0.0,1.0);}";
const CANONICAL_FRAGMENT_COLOR_PREFIX: &str = "void main(){gl_FragColor=vec4(";
const CANONICAL_FRAGMENT_COLOR_SUFFIX: &str = ");}";
const CANONICAL_FRAGMENT_UNIFORM_PREFIX: &str = "uniform vec4 ";
const CANONICAL_FRAGMENT_UNIFORM_MIDDLE: &str = ";void main(){gl_FragColor=";
const CANONICAL_FRAGMENT_UNIFORM_SUFFIX: &str = ";}";
const CANONICAL_FRAGMENT_VARYING_PREFIX: &str = "varying vec4 ";
const CANONICAL_FRAGMENT_VARYING_MIDDLE: &str = ";void main(){gl_FragColor=";
const CANONICAL_FRAGMENT_VARYING_SUFFIX: &str = ";}";

#[derive(Clone)]
pub(crate) struct TranslatedProgram {
    pub(crate) vertex_wgsl: String,
    pub(crate) fragment_wgsl: String,
    pub(crate) reflection: ProgramReflection,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) struct ProgramCacheKey {
    vertex: String,
    fragment: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ProgramReflection {
    pub(crate) position_attribute: VertexAttributeReflection,
    pub(crate) color_attribute: Option<VertexAttributeReflection>,
    pub(crate) fragment_color_uniform: Option<UniformReflection>,
    pub(crate) fragment_float_precision: WebGlPrecision,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct VertexAttributeReflection {
    pub(crate) name: String,
    pub(crate) location: u32,
    pub(crate) kind: VertexAttributeKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum VertexAttributeKind {
    Float32x2,
    Float32x4,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct UniformReflection {
    pub(crate) name: String,
    pub(crate) binding: u32,
    pub(crate) kind: UniformKind,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum UniformKind {
    Float32x4,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum ShaderTranslationError {
    UnsupportedCanonicalPair,
    ThreadSpawn(String),
    ThreadJoin(String),
    NagaPanic(String),
    Parse(String),
    Validate(String),
    Emit(String),
}

impl fmt::Display for ShaderTranslationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCanonicalPair => formatter.write_str("unsupported ESSL shader pair"),
            Self::ThreadSpawn(message) => {
                write!(formatter, "failed to spawn naga thread: {message}")
            },
            Self::ThreadJoin(message) => {
                write!(formatter, "naga thread panicked at join: {message}")
            },
            Self::NagaPanic(message) => write!(formatter, "naga panicked: {message}"),
            Self::Parse(message) => write!(formatter, "GLSL->naga parse failed: {message}"),
            Self::Validate(message) => write!(formatter, "naga validation failed: {message}"),
            Self::Emit(message) => write!(formatter, "WGSL emit failed: {message}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum WebGlShaderStage {
    Vertex,
    Fragment,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum WebGlPrecision {
    Low,
    Medium,
    High,
}

impl WebGlPrecision {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "lowp" => Some(Self::Low),
            "mediump" => Some(Self::Medium),
            "highp" => Some(Self::High),
            _ => None,
        }
    }

    fn essl_token(self) -> &'static str {
        match self {
            Self::Low => "lowp",
            Self::Medium => "mediump",
            Self::High => "highp",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct CanonicalVertexInfo {
    position_attribute_name: String,
    color_attribute_name: Option<String>,
    varying_color_name: Option<String>,
}

impl CanonicalVertexInfo {
    fn parse(source: &str) -> Result<Self, ShaderTranslationError> {
        let normalized = normalize_shader(source);
        if let Ok(vertex) = Self::parse_position_only(&normalized) {
            return Ok(vertex);
        }
        Self::parse_varying_color(&normalized)
    }

    fn parse_position_only(normalized: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = normalized.strip_prefix(CANONICAL_VERTEX_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((declared_name, rest)) = rest.split_once(CANONICAL_VERTEX_MIDDLE) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_name) = rest.strip_suffix(CANONICAL_VERTEX_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if declared_name != used_name || !is_valid_essl_identifier(declared_name) {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            position_attribute_name: declared_name.to_string(),
            color_attribute_name: None,
            varying_color_name: None,
        })
    }

    fn parse_varying_color(normalized: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = normalized.strip_prefix(CANONICAL_VARYING_VERTEX_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((position_name, rest)) = rest.split_once(CANONICAL_VARYING_VERTEX_COLOR_DECL)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((color_name, rest)) = rest.split_once(CANONICAL_VARYING_VERTEX_VARYING_DECL)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((varying_name, rest)) = rest.split_once(CANONICAL_VARYING_VERTEX_MAIN_PREFIX)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((assigned_varying, rest)) =
            rest.split_once(CANONICAL_VARYING_VERTEX_ASSIGN_MIDDLE)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((assigned_color, rest)) =
            rest.split_once(CANONICAL_VARYING_VERTEX_POSITION_PREFIX)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_position_name) = rest.strip_suffix(CANONICAL_VARYING_VERTEX_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if position_name != used_position_name
            || color_name != assigned_color
            || varying_name != assigned_varying
            || !is_valid_essl_identifier(position_name)
            || !is_valid_essl_identifier(color_name)
            || !is_valid_essl_identifier(varying_name)
        {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            position_attribute_name: position_name.to_string(),
            color_attribute_name: Some(color_name.to_string()),
            varying_color_name: Some(varying_name.to_string()),
        })
    }

    fn normalized_source(&self) -> String {
        if let (Some(color_name), Some(varying_name)) = (
            self.color_attribute_name.as_ref(),
            self.varying_color_name.as_ref(),
        ) {
            format!(
                "{CANONICAL_VARYING_VERTEX_PREFIX}{}{CANONICAL_VARYING_VERTEX_COLOR_DECL}{}{CANONICAL_VARYING_VERTEX_VARYING_DECL}{}{CANONICAL_VARYING_VERTEX_MAIN_PREFIX}{}{CANONICAL_VARYING_VERTEX_ASSIGN_MIDDLE}{}{CANONICAL_VARYING_VERTEX_POSITION_PREFIX}{}{CANONICAL_VARYING_VERTEX_SUFFIX}",
                self.position_attribute_name,
                color_name,
                varying_name,
                varying_name,
                color_name,
                self.position_attribute_name
            )
        } else {
            format!(
                "{CANONICAL_VERTEX_PREFIX}{}{CANONICAL_VERTEX_MIDDLE}{}{CANONICAL_VERTEX_SUFFIX}",
                self.position_attribute_name, self.position_attribute_name
            )
        }
    }

    fn naga_glsl(&self) -> String {
        if let (Some(color_name), Some(varying_name)) = (
            self.color_attribute_name.as_ref(),
            self.varying_color_name.as_ref(),
        ) {
            format!(
                "#version 450\nlayout(location = 0) in vec2 {};\nlayout(location = 1) in vec4 {};\nlayout(location = 0) out vec4 {};\nvoid main() {{\n    {} = {};\n    gl_Position = vec4({}, 0.0, 1.0);\n}}\n",
                self.position_attribute_name,
                color_name,
                varying_name,
                varying_name,
                color_name,
                self.position_attribute_name
            )
        } else {
            format!(
                "#version 450\nlayout(location = 0) in vec2 {};\nvoid main() {{\n    gl_Position = vec4({}, 0.0, 1.0);\n}}\n",
                self.position_attribute_name, self.position_attribute_name
            )
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct CanonicalFragmentInfo {
    float_precision: WebGlPrecision,
    color: FragmentColorSource,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum FragmentColorSource {
    Literal(FragmentColor),
    Uniform(FragmentColorUniform),
    Varying(FragmentColorVarying),
}

impl FragmentColorSource {
    fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        if body.starts_with(CANONICAL_FRAGMENT_COLOR_PREFIX) {
            return FragmentColor::parse(body).map(Self::Literal);
        }
        if body.starts_with(CANONICAL_FRAGMENT_VARYING_PREFIX) {
            return FragmentColorVarying::parse(body).map(Self::Varying);
        }
        FragmentColorUniform::parse(body).map(Self::Uniform)
    }

    fn normalized_body(&self) -> String {
        match self {
            Self::Literal(color) => color.normalized_body(),
            Self::Uniform(uniform) => uniform.normalized_body(),
            Self::Varying(varying) => varying.normalized_body(),
        }
    }

    fn naga_glsl(&self) -> String {
        match self {
            Self::Literal(color) => color.naga_glsl(),
            Self::Uniform(uniform) => uniform.naga_glsl(),
            Self::Varying(varying) => varying.naga_glsl(),
        }
    }

    fn uniform_reflection(&self) -> Option<UniformReflection> {
        match self {
            Self::Literal(_) => None,
            Self::Uniform(uniform) => Some(UniformReflection {
                name: uniform.name.clone(),
                binding: 0,
                kind: UniformKind::Float32x4,
            }),
            Self::Varying(_) => None,
        }
    }

    fn varying_name(&self) -> Option<&str> {
        match self {
            Self::Varying(varying) => Some(&varying.name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct FragmentColor {
    components: [String; 4],
}

impl FragmentColor {
    fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        let Some(components) = body.strip_prefix(CANONICAL_FRAGMENT_COLOR_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(components) = components.strip_suffix(CANONICAL_FRAGMENT_COLOR_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let components = components
            .split(',')
            .map(validate_fragment_color_component)
            .collect::<Result<Vec<_>, _>>()?;
        let Ok(components) = components.try_into() else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        Ok(Self { components })
    }

    fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_COLOR_PREFIX}{}{CANONICAL_FRAGMENT_COLOR_SUFFIX}",
            self.components.join(",")
        )
    }

    fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = vec4({}, {}, {}, {});\n}}\n",
            self.components[0], self.components[1], self.components[2], self.components[3]
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct FragmentColorUniform {
    name: String,
}

impl FragmentColorUniform {
    fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = body.strip_prefix(CANONICAL_FRAGMENT_UNIFORM_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((declared_name, rest)) = rest.split_once(CANONICAL_FRAGMENT_UNIFORM_MIDDLE) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_name) = rest.strip_suffix(CANONICAL_FRAGMENT_UNIFORM_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if declared_name != used_name || !is_valid_essl_identifier(declared_name) {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            name: declared_name.to_string(),
        })
    }

    fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_UNIFORM_PREFIX}{}{CANONICAL_FRAGMENT_UNIFORM_MIDDLE}{}{CANONICAL_FRAGMENT_UNIFORM_SUFFIX}",
            self.name, self.name
        )
    }

    fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(set = 0, binding = 0) uniform WebGlUniforms {{\n    vec4 {};\n}};\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = {};\n}}\n",
            self.name, self.name
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct FragmentColorVarying {
    name: String,
}

impl FragmentColorVarying {
    fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = body.strip_prefix(CANONICAL_FRAGMENT_VARYING_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((declared_name, rest)) = rest.split_once(CANONICAL_FRAGMENT_VARYING_MIDDLE) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_name) = rest.strip_suffix(CANONICAL_FRAGMENT_VARYING_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if declared_name != used_name || !is_valid_essl_identifier(declared_name) {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            name: declared_name.to_string(),
        })
    }

    fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_VARYING_PREFIX}{}{CANONICAL_FRAGMENT_VARYING_MIDDLE}{}{CANONICAL_FRAGMENT_VARYING_SUFFIX}",
            self.name, self.name
        )
    }

    fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(location = 0) in vec4 {};\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = {};\n}}\n",
            self.name, self.name
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct CanonicalProgramInfo {
    vertex: CanonicalVertexInfo,
    fragment: CanonicalFragmentInfo,
}

struct NagaGlslShader {
    stage: WebGlShaderStage,
    name: &'static str,
    source: String,
    float_precision: Option<WebGlPrecision>,
}

struct NagaGlslProgram {
    vertex: NagaGlslShader,
    fragment: NagaGlslShader,
    reflection: ProgramReflection,
}

pub(crate) fn translate_canonical_essl_pair(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<TranslatedProgram, ShaderTranslationError> {
    let lowered = lower_canonical_pair_to_naga_glsl(vertex_source, fragment_source)?;

    Ok(TranslatedProgram {
        vertex_wgsl: translate_to_wgsl(lowered.vertex)?,
        fragment_wgsl: translate_to_wgsl(lowered.fragment)?,
        reflection: lowered.reflection,
    })
}

pub(crate) fn canonical_essl_cache_key(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<ProgramCacheKey, ShaderTranslationError> {
    let info = validate_canonical_essl_pair(vertex_source, fragment_source)?;
    Ok(ProgramCacheKey {
        vertex: info.vertex.normalized_source(),
        fragment: format!(
            "precision {} float;{}",
            info.fragment.float_precision.essl_token(),
            info.fragment.color.normalized_body()
        ),
    })
}

fn lower_canonical_pair_to_naga_glsl(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<NagaGlslProgram, ShaderTranslationError> {
    let info = validate_canonical_essl_pair(vertex_source, fragment_source)?;

    Ok(NagaGlslProgram {
        vertex: NagaGlslShader {
            stage: WebGlShaderStage::Vertex,
            name: "canonical_triangle_vertex",
            source: info.vertex.naga_glsl(),
            float_precision: None,
        },
        fragment: NagaGlslShader {
            stage: WebGlShaderStage::Fragment,
            name: "canonical_triangle_fragment",
            source: info.fragment.color.naga_glsl(),
            float_precision: Some(info.fragment.float_precision),
        },
        reflection: ProgramReflection {
            position_attribute: VertexAttributeReflection {
                name: info.vertex.position_attribute_name,
                location: 0,
                kind: VertexAttributeKind::Float32x2,
            },
            color_attribute: info.vertex.color_attribute_name.map(|name| {
                VertexAttributeReflection {
                    name,
                    location: 1,
                    kind: VertexAttributeKind::Float32x4,
                }
            }),
            fragment_color_uniform: info.fragment.color.uniform_reflection(),
            fragment_float_precision: info.fragment.float_precision,
        },
    })
}

fn validate_canonical_essl_pair(
    vertex_source: &str,
    fragment_source: &str,
) -> Result<CanonicalProgramInfo, ShaderTranslationError> {
    let vertex = CanonicalVertexInfo::parse(vertex_source)?;
    let fragment = parse_canonical_fragment(fragment_source)?;
    match (
        vertex.varying_color_name.as_deref(),
        fragment.color.varying_name(),
    ) {
        (Some(vertex_varying), Some(fragment_varying)) if vertex_varying == fragment_varying => {},
        (None, None) => {},
        _ => return Err(ShaderTranslationError::UnsupportedCanonicalPair),
    }
    Ok(CanonicalProgramInfo { vertex, fragment })
}

fn parse_canonical_fragment(source: &str) -> Result<CanonicalFragmentInfo, ShaderTranslationError> {
    let normalized = normalize_shader(source);
    let Some(rest) = normalized.strip_prefix("precision ") else {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    };
    let Some((precision_token, rest)) = rest.split_once(" float;") else {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    };
    let Some(float_precision) = WebGlPrecision::parse(precision_token) else {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    };
    let color = FragmentColorSource::parse(rest)?;

    Ok(CanonicalFragmentInfo {
        float_precision,
        color,
    })
}

fn validate_fragment_color_component(component: &str) -> Result<String, ShaderTranslationError> {
    if component.is_empty()
        || !component.contains('.')
        || component.chars().filter(|ch| *ch == '.').count() != 1
        || !component.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
    {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    }
    let value = component
        .parse::<f32>()
        .map_err(|_| ShaderTranslationError::UnsupportedCanonicalPair)?;
    if !(0.0..=1.0).contains(&value) {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    }
    Ok(component.to_string())
}

fn is_valid_essl_identifier(identifier: &str) -> bool {
    let mut chars = identifier.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        return false;
    }
    !matches!(
        identifier,
        "attribute" | "gl_FragColor" | "gl_Position" | "main" | "precision" | "vec2" | "vec4"
    )
}

fn translate_to_wgsl(shader: NagaGlslShader) -> Result<String, ShaderTranslationError> {
    use naga::{
        back::wgsl,
        front::glsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    let glsl_owned = shader.source;
    let name = shader.name;
    let _float_precision = shader.float_precision;
    let stage: naga::ShaderStage = shader.stage.into();
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            std::panic::catch_unwind(move || {
                let mut frontend = glsl::Frontend::default();
                let module = frontend
                    .parse(&glsl::Options::from(stage), &glsl_owned)
                    .map_err(|error| {
                        ShaderTranslationError::Parse(format!("[{name}]: {error:?}"))
                    })?;
                let info = Validator::new(ValidationFlags::all(), Capabilities::all())
                    .validate(&module)
                    .map_err(|error| {
                        ShaderTranslationError::Validate(format!("[{name}]: {error:?}"))
                    })?;
                wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
                    .map_err(|error| ShaderTranslationError::Emit(format!("[{name}]: {error:?}")))
            })
        })
        .map_err(|error| ShaderTranslationError::ThreadSpawn(format!("[{name}]: {error}")))?;

    match handle.join().map_err(|panic| {
        ShaderTranslationError::ThreadJoin(format!("[{name}]: {}", panic_message(&*panic)))
    })? {
        Ok(result) => result,
        Err(panic) => Err(ShaderTranslationError::NagaPanic(format!(
            "[{name}]: {}",
            panic_message(&*panic)
        ))),
    }
}

impl From<WebGlShaderStage> for naga::ShaderStage {
    fn from(stage: WebGlShaderStage) -> Self {
        match stage {
            WebGlShaderStage::Vertex => Self::Vertex,
            WebGlShaderStage::Fragment => Self::Fragment,
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "unknown panic".to_string()
    }
}

fn normalize_shader(source: &str) -> String {
    let source = strip_comments(source);
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if is_glsl_punctuation(ch) {
            tokens.push(ch.to_string());
            continue;
        }

        let mut token = String::from(ch);
        while let Some(&next) = chars.peek() {
            if next.is_whitespace() || is_glsl_punctuation(next) {
                break;
            }
            token.push(next);
            chars.next();
        }
        tokens.push(token);
    }

    let mut normalized = String::new();
    for token in tokens {
        if let Some(previous) = normalized.chars().last() {
            if needs_token_space(previous, &token) {
                normalized.push(' ');
            }
        }
        normalized.push_str(&token);
    }
    normalized
}

fn is_glsl_punctuation(ch: char) -> bool {
    matches!(ch, '(' | ')' | '{' | '}' | ',' | ';' | '=')
}

fn needs_token_space(previous: char, token: &str) -> bool {
    !is_glsl_punctuation(previous) && !token.chars().next().is_some_and(is_glsl_punctuation)
}

fn strip_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for ch in chars.by_ref() {
                if ch == '\n' {
                    output.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for ch in chars.by_ref() {
                if previous == '*' && ch == '/' {
                    output.push(' ');
                    break;
                }
                previous = ch;
            }
        } else {
            output.push(ch);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_essl_pair_translates_through_naga() {
        let translated = translate_canonical_essl_pair(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical pair translates");

        assert!(translated.vertex_wgsl.contains("fn main"));
        assert!(translated.vertex_wgsl.contains("@location(0)"));
        assert!(translated.fragment_wgsl.contains("fn main"));
        assert!(translated.fragment_wgsl.contains("webgl_FragColor"));
        assert_eq!(
            translated.reflection.position_attribute,
            VertexAttributeReflection {
                name: "a_position".to_string(),
                location: 0,
                kind: VertexAttributeKind::Float32x2,
            }
        );
        assert_eq!(translated.reflection.color_attribute, None);
        assert_eq!(translated.reflection.fragment_color_uniform, None);
        assert_eq!(
            translated.reflection.fragment_float_precision,
            WebGlPrecision::Medium
        );
    }

    #[test]
    fn noncanonical_essl_pair_is_not_translated() {
        let result = translate_canonical_essl_pair(
            "attribute vec2 a_position; void main() { gl_Position = vec4(0.0); }",
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        );

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }

    #[test]
    fn canonical_lowering_targets_naga_glsl_boundaries() {
        let lowered = lower_canonical_pair_to_naga_glsl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical pair lowers");

        assert_eq!(lowered.vertex.stage, WebGlShaderStage::Vertex);
        assert!(lowered.vertex.source.contains("#version 450"));
        assert!(
            lowered
                .vertex
                .source
                .contains("layout(location = 0) in vec2 a_position")
        );
        assert_eq!(lowered.fragment.stage, WebGlShaderStage::Fragment);
        assert_eq!(
            lowered.fragment.float_precision,
            Some(WebGlPrecision::Medium)
        );
        assert_eq!(
            lowered.reflection.position_attribute.kind,
            VertexAttributeKind::Float32x2
        );
        assert_eq!(lowered.reflection.position_attribute.location, 0);
        assert_eq!(lowered.reflection.position_attribute.name, "a_position");
        assert_eq!(lowered.reflection.color_attribute, None);
        assert_eq!(lowered.reflection.fragment_color_uniform, None);
        assert_eq!(
            lowered.reflection.fragment_float_precision,
            WebGlPrecision::Medium
        );
        assert!(
            lowered
                .fragment
                .source
                .contains("layout(location = 0) out vec4 webgl_FragColor")
        );
    }

    #[test]
    fn canonical_fragment_accepts_float_precision_variants() {
        for (precision, expected) in [
            ("lowp", WebGlPrecision::Low),
            ("mediump", WebGlPrecision::Medium),
            ("highp", WebGlPrecision::High),
        ] {
            let fragment = format!(
                "precision {precision} float; void main() {{ gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }}"
            );
            let lowered =
                lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, &fragment)
                    .expect("precision variant lowers");

            assert_eq!(lowered.fragment.float_precision, Some(expected));
        }
    }

    #[test]
    fn canonical_fragment_lowers_literal_color() {
        let fragment = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(1.0, 0.0, 0.5, 1.0);
            }
        "#;

        let lowered = lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
            .expect("literal color lowers");

        assert!(
            lowered
                .fragment
                .source
                .contains("webgl_FragColor = vec4(1.0, 0.0, 0.5, 1.0)")
        );
    }

    #[test]
    fn canonical_fragment_lowers_uniform_color() {
        let fragment = r#"
            precision mediump float;
            uniform vec4 u_color;
            void main() {
                gl_FragColor = u_color;
            }
        "#;

        let lowered = lower_canonical_pair_to_naga_glsl(CANONICAL_TRIANGLE_VERTEX_SHADER, fragment)
            .expect("uniform color lowers");

        assert!(
            lowered
                .fragment
                .source
                .contains("layout(set = 0, binding = 0) uniform WebGlUniforms")
        );
        assert!(lowered.fragment.source.contains("vec4 u_color"));
        assert_eq!(
            lowered.reflection.fragment_color_uniform,
            Some(UniformReflection {
                name: "u_color".to_string(),
                binding: 0,
                kind: UniformKind::Float32x4,
            })
        );
    }

    #[test]
    fn canonical_fragment_rejects_uniform_name_mismatch() {
        let result = lower_canonical_pair_to_naga_glsl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            "precision mediump float; uniform vec4 u_color; void main() { gl_FragColor = color; }",
        );

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }

    #[test]
    fn canonical_pair_lowers_varying_color() {
        let vertex = r#"
            attribute vec2 a_position;
            attribute vec4 a_color;
            varying vec4 v_color;
            void main() {
                v_color = a_color;
                gl_Position = vec4(a_position, 0.0, 1.0);
            }
        "#;
        let fragment = r#"
            precision mediump float;
            varying vec4 v_color;
            void main() {
                gl_FragColor = v_color;
            }
        "#;

        let lowered =
            lower_canonical_pair_to_naga_glsl(vertex, fragment).expect("varying color pair lowers");

        assert!(
            lowered
                .vertex
                .source
                .contains("layout(location = 1) in vec4 a_color")
        );
        assert!(
            lowered
                .vertex
                .source
                .contains("layout(location = 0) out vec4 v_color")
        );
        assert!(
            lowered
                .fragment
                .source
                .contains("layout(location = 0) in vec4 v_color")
        );
        assert_eq!(
            lowered.reflection.color_attribute,
            Some(VertexAttributeReflection {
                name: "a_color".to_string(),
                location: 1,
                kind: VertexAttributeKind::Float32x4,
            })
        );
    }

    #[test]
    fn canonical_pair_rejects_varying_link_mismatch() {
        let vertex = r#"
            attribute vec2 a_position;
            attribute vec4 a_color;
            varying vec4 v_color;
            void main() {
                v_color = a_color;
                gl_Position = vec4(a_position, 0.0, 1.0);
            }
        "#;
        let fragment = r#"
            precision mediump float;
            varying vec4 other_color;
            void main() {
                gl_FragColor = other_color;
            }
        "#;

        let result = lower_canonical_pair_to_naga_glsl(vertex, fragment);

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }

    #[test]
    fn canonical_vertex_reflects_attribute_name() {
        let vertex = r#"
            attribute vec2 position;
            void main() {
                gl_Position = vec4(position, 0.0, 1.0);
            }
        "#;

        let lowered = lower_canonical_pair_to_naga_glsl(vertex, CANONICAL_TRIANGLE_FRAGMENT_SHADER)
            .expect("renamed canonical vertex lowers");

        assert_eq!(lowered.reflection.position_attribute.name, "position");
        assert!(
            lowered
                .vertex
                .source
                .contains("layout(location = 0) in vec2 position")
        );
        assert!(
            lowered
                .vertex
                .source
                .contains("gl_Position = vec4(position, 0.0, 1.0)")
        );
    }

    #[test]
    fn canonical_vertex_rejects_attribute_name_mismatch() {
        let result = lower_canonical_pair_to_naga_glsl(
            "attribute vec2 position; void main() { gl_Position = vec4(a_position, 0.0, 1.0); }",
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        );

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }

    #[test]
    fn canonical_fragment_rejects_nonliteral_color() {
        let result = lower_canonical_pair_to_naga_glsl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            "precision mediump float; void main() { gl_FragColor = vec4(1.0, 0.0, 2.0, 1.0); }",
        );

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }

    #[test]
    fn canonical_pair_accepts_comments_and_whitespace() {
        let vertex = r#"
            // WebGL-facing ESSL stays the input contract.
            attribute   vec2   a_position;
            void main() {
                gl_Position = vec4(
                    a_position, /* z */ 0.0,
                    1.0
                );
            }
        "#;
        let fragment = r#"
            precision highp float;
            void main() {
                // canonical smoke color
                gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0);
            }
        "#;

        let lowered = lower_canonical_pair_to_naga_glsl(vertex, fragment)
            .expect("commented canonical shaders lower");

        assert_eq!(lowered.fragment.float_precision, Some(WebGlPrecision::High));
    }

    #[test]
    fn canonical_cache_key_uses_validated_shape() {
        let formatted = r#"
            precision mediump float;
            void main() {
                gl_FragColor = vec4(
                    0.0, 1.0,
                    0.0, 1.0
                );
            }
        "#;

        let canonical = canonical_essl_cache_key(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            CANONICAL_TRIANGLE_FRAGMENT_SHADER,
        )
        .expect("canonical cache key");
        let reformatted = canonical_essl_cache_key(CANONICAL_TRIANGLE_VERTEX_SHADER, formatted)
            .expect("formatted cache key");

        assert_eq!(canonical, reformatted);
    }

    #[test]
    fn canonical_fragment_requires_float_precision() {
        let result = lower_canonical_pair_to_naga_glsl(
            CANONICAL_TRIANGLE_VERTEX_SHADER,
            "void main() { gl_FragColor = vec4(0.0, 1.0, 0.0, 1.0); }",
        );

        assert!(matches!(
            result,
            Err(ShaderTranslationError::UnsupportedCanonicalPair)
        ));
    }
}
