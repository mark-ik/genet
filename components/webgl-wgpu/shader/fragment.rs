/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::normalize::{is_valid_essl_identifier, normalize_shader};
use super::*;

pub(super) struct CanonicalFragmentInfo {
    pub(super) float_precision: WebGlPrecision,
    pub(super) color: FragmentColorSource,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) enum FragmentColorSource {
    Literal(FragmentColor),
    Uniform(FragmentColorUniform),
    Varying(FragmentColorVarying),
    Texture(FragmentTextureSample),
}

impl FragmentColorSource {
    pub(super) fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        if body.starts_with(CANONICAL_FRAGMENT_COLOR_PREFIX) {
            return FragmentColor::parse(body).map(Self::Literal);
        }
        if body.starts_with(CANONICAL_FRAGMENT_VARYING_PREFIX) {
            return FragmentColorVarying::parse(body).map(Self::Varying);
        }
        if body.starts_with(CANONICAL_FRAGMENT_TEXTURE_PREFIX) {
            return FragmentTextureSample::parse(body).map(Self::Texture);
        }
        FragmentColorUniform::parse(body).map(Self::Uniform)
    }

    pub(super) fn normalized_body(&self) -> String {
        match self {
            Self::Literal(color) => color.normalized_body(),
            Self::Uniform(uniform) => uniform.normalized_body(),
            Self::Varying(varying) => varying.normalized_body(),
            Self::Texture(texture) => texture.normalized_body(),
        }
    }

    pub(super) fn naga_glsl(&self) -> String {
        match self {
            Self::Literal(color) => color.naga_glsl(),
            Self::Uniform(uniform) => uniform.naga_glsl(),
            Self::Varying(varying) => varying.naga_glsl(),
            Self::Texture(texture) => texture.naga_glsl(),
        }
    }

    pub(super) fn uniform_reflection(&self) -> Option<UniformReflection> {
        match self {
            Self::Literal(_) => None,
            Self::Uniform(uniform) => Some(UniformReflection {
                name: uniform.name.clone(),
                binding: 0,
                kind: UniformKind::Float32x4,
            }),
            Self::Varying(_) | Self::Texture(_) => None,
        }
    }

    pub(super) fn varying_name(&self) -> Option<&str> {
        match self {
            Self::Varying(varying) => Some(&varying.name),
            _ => None,
        }
    }

    pub(super) fn texture_varying_name(&self) -> Option<&str> {
        match self {
            Self::Texture(texture) => Some(&texture.varying_name),
            _ => None,
        }
    }

    pub(super) fn texture_uniform_reflection(&self) -> Option<UniformReflection> {
        match self {
            Self::Texture(texture) => Some(UniformReflection {
                name: texture.sampler_name.clone(),
                binding: 0,
                kind: UniformKind::Sampler2D,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) struct FragmentColor {
    components: [String; 4],
}

impl FragmentColor {
    pub(super) fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
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

    pub(super) fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_COLOR_PREFIX}{}{CANONICAL_FRAGMENT_COLOR_SUFFIX}",
            self.components.join(",")
        )
    }

    pub(super) fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = vec4({}, {}, {}, {});\n}}\n",
            self.components[0], self.components[1], self.components[2], self.components[3]
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) struct FragmentColorUniform {
    name: String,
}

impl FragmentColorUniform {
    pub(super) fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
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

    pub(super) fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_UNIFORM_PREFIX}{}{CANONICAL_FRAGMENT_UNIFORM_MIDDLE}{}{CANONICAL_FRAGMENT_UNIFORM_SUFFIX}",
            self.name, self.name
        )
    }

    pub(super) fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(set = 0, binding = 0) uniform WebGlUniforms {{\n    vec4 {};\n}};\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = {};\n}}\n",
            self.name, self.name
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) struct FragmentColorVarying {
    name: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(super) struct FragmentTextureSample {
    varying_name: String,
    sampler_name: String,
}

impl FragmentTextureSample {
    pub(super) fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = body.strip_prefix(CANONICAL_FRAGMENT_TEXTURE_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((varying_name, rest)) = rest.split_once(CANONICAL_FRAGMENT_TEXTURE_SAMPLER_DECL)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((sampler_name, rest)) = rest.split_once(CANONICAL_FRAGMENT_TEXTURE_MAIN_PREFIX)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((used_sampler_name, rest)) =
            rest.split_once(CANONICAL_FRAGMENT_TEXTURE_COORD_SEPARATOR)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_varying_name) = rest.strip_suffix(CANONICAL_FRAGMENT_TEXTURE_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if sampler_name != used_sampler_name
            || varying_name != used_varying_name
            || !is_valid_essl_identifier(varying_name)
            || !is_valid_essl_identifier(sampler_name)
        {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            varying_name: varying_name.to_string(),
            sampler_name: sampler_name.to_string(),
        })
    }

    pub(super) fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_TEXTURE_PREFIX}{}{CANONICAL_FRAGMENT_TEXTURE_SAMPLER_DECL}{}{CANONICAL_FRAGMENT_TEXTURE_MAIN_PREFIX}{}{CANONICAL_FRAGMENT_TEXTURE_COORD_SEPARATOR}{}{CANONICAL_FRAGMENT_TEXTURE_SUFFIX}",
            self.varying_name, self.sampler_name, self.sampler_name, self.varying_name
        )
    }

    pub(super) fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(location = 0) in vec2 {};\nlayout(set = 0, binding = 0) uniform texture2D {};\nlayout(set = 0, binding = 1) uniform sampler {}_sampler;\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = texture(sampler2D({}, {}_sampler), {});\n}}\n",
            self.varying_name,
            self.sampler_name,
            self.sampler_name,
            self.sampler_name,
            self.sampler_name,
            self.varying_name
        )
    }
}

impl FragmentColorVarying {
    pub(super) fn parse(body: &str) -> Result<Self, ShaderTranslationError> {
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

    pub(super) fn normalized_body(&self) -> String {
        format!(
            "{CANONICAL_FRAGMENT_VARYING_PREFIX}{}{CANONICAL_FRAGMENT_VARYING_MIDDLE}{}{CANONICAL_FRAGMENT_VARYING_SUFFIX}",
            self.name, self.name
        )
    }

    pub(super) fn naga_glsl(&self) -> String {
        format!(
            "#version 450\nlayout(location = 0) in vec4 {};\nlayout(location = 0) out vec4 webgl_FragColor;\nvoid main() {{\n    webgl_FragColor = {};\n}}\n",
            self.name, self.name
        )
    }
}

pub(super) fn parse_canonical_fragment(
    source: &str,
) -> Result<CanonicalFragmentInfo, ShaderTranslationError> {
    let normalized = normalize_shader(source);
    let (float_precision, rest) = parse_fragment_precision_prefix(&normalized)?;
    let color = FragmentColorSource::parse(rest)?;

    Ok(CanonicalFragmentInfo {
        float_precision,
        color,
    })
}

fn parse_fragment_precision_prefix(
    source: &str,
) -> Result<(WebGlPrecision, &str), ShaderTranslationError> {
    let mut rest = source;
    let mut float_precision = None;

    loop {
        let Some(after_precision) = rest.strip_prefix("precision ") else {
            break;
        };
        let Some((declaration, tail)) = after_precision.split_once(';') else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((precision_token, scalar_kind)) = declaration.split_once(' ') else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(precision) = WebGlPrecision::parse(precision_token) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        match scalar_kind {
            "float" if float_precision.replace(precision).is_none() => {},
            "int" => {},
            _ => return Err(ShaderTranslationError::UnsupportedCanonicalPair),
        }
        rest = tail;
    }

    let Some(float_precision) = float_precision else {
        return Err(ShaderTranslationError::UnsupportedCanonicalPair);
    };
    Ok((float_precision, rest))
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
