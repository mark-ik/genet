/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use super::normalize::{is_valid_essl_identifier, normalize_shader};
use super::*;

pub(super) struct CanonicalVertexInfo {
    pub(super) position_attribute_name: String,
    pub(super) color_attribute_name: Option<String>,
    pub(super) varying_color_name: Option<String>,
    pub(super) texcoord_attribute_name: Option<String>,
    pub(super) varying_texcoord_name: Option<String>,
}

impl CanonicalVertexInfo {
    pub(super) fn parse(source: &str) -> Result<Self, ShaderTranslationError> {
        let normalized = normalize_shader(source);
        if let Ok(vertex) = Self::parse_position_only(&normalized) {
            return Ok(vertex);
        }
        if let Ok(vertex) = Self::parse_varying_color(&normalized) {
            return Ok(vertex);
        }
        Self::parse_texture_coords(&normalized)
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
            texcoord_attribute_name: None,
            varying_texcoord_name: None,
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
            texcoord_attribute_name: None,
            varying_texcoord_name: None,
        })
    }

    fn parse_texture_coords(normalized: &str) -> Result<Self, ShaderTranslationError> {
        let Some(rest) = normalized.strip_prefix(CANONICAL_TEXTURE_VERTEX_PREFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((position_name, rest)) = rest.split_once(CANONICAL_TEXTURE_VERTEX_UV_DECL) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((texcoord_name, rest)) = rest.split_once(CANONICAL_TEXTURE_VERTEX_VARYING_DECL)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((varying_name, rest)) = rest.split_once(CANONICAL_TEXTURE_VERTEX_MAIN_PREFIX)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((assigned_varying, rest)) =
            rest.split_once(CANONICAL_TEXTURE_VERTEX_ASSIGN_MIDDLE)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some((assigned_texcoord, rest)) =
            rest.split_once(CANONICAL_TEXTURE_VERTEX_POSITION_PREFIX)
        else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        let Some(used_position_name) = rest.strip_suffix(CANONICAL_TEXTURE_VERTEX_SUFFIX) else {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        };
        if position_name != used_position_name
            || texcoord_name != assigned_texcoord
            || varying_name != assigned_varying
            || !is_valid_essl_identifier(position_name)
            || !is_valid_essl_identifier(texcoord_name)
            || !is_valid_essl_identifier(varying_name)
        {
            return Err(ShaderTranslationError::UnsupportedCanonicalPair);
        }
        Ok(Self {
            position_attribute_name: position_name.to_string(),
            color_attribute_name: None,
            varying_color_name: None,
            texcoord_attribute_name: Some(texcoord_name.to_string()),
            varying_texcoord_name: Some(varying_name.to_string()),
        })
    }

    pub(super) fn normalized_source(&self) -> String {
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
        } else if let (Some(texcoord_name), Some(varying_name)) = (
            self.texcoord_attribute_name.as_ref(),
            self.varying_texcoord_name.as_ref(),
        ) {
            format!(
                "{CANONICAL_TEXTURE_VERTEX_PREFIX}{}{CANONICAL_TEXTURE_VERTEX_UV_DECL}{}{CANONICAL_TEXTURE_VERTEX_VARYING_DECL}{}{CANONICAL_TEXTURE_VERTEX_MAIN_PREFIX}{}{CANONICAL_TEXTURE_VERTEX_ASSIGN_MIDDLE}{}{CANONICAL_TEXTURE_VERTEX_POSITION_PREFIX}{}{CANONICAL_TEXTURE_VERTEX_SUFFIX}",
                self.position_attribute_name,
                texcoord_name,
                varying_name,
                varying_name,
                texcoord_name,
                self.position_attribute_name
            )
        } else {
            format!(
                "{CANONICAL_VERTEX_PREFIX}{}{CANONICAL_VERTEX_MIDDLE}{}{CANONICAL_VERTEX_SUFFIX}",
                self.position_attribute_name, self.position_attribute_name
            )
        }
    }

    pub(super) fn naga_glsl(&self) -> String {
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
        } else if let (Some(texcoord_name), Some(varying_name)) = (
            self.texcoord_attribute_name.as_ref(),
            self.varying_texcoord_name.as_ref(),
        ) {
            format!(
                "#version 450\nlayout(location = 0) in vec2 {};\nlayout(location = 1) in vec2 {};\nlayout(location = 0) out vec2 {};\nvoid main() {{\n    {} = {};\n    gl_Position = vec4({}, 0.0, 1.0);\n}}\n",
                self.position_attribute_name,
                texcoord_name,
                varying_name,
                varying_name,
                texcoord_name,
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
