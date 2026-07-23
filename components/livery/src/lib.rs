//! Generated CSS property and cascade engine.
//!
//! The generated catalog is the executable contract for Livery's first lane:
//! Cambium structural UI. Value parsing and cascade behavior grow against this
//! bounded property set.

#![forbid(unsafe_code)]

pub mod cascade;
pub mod custom;
pub mod media;
pub mod selector;
pub mod stylesheet;
pub mod values;

include!(concat!(env!("OUT_DIR"), "/properties.rs"));

/// Canonicalize one implemented longhand's specified value.
///
/// `None` means Livery cannot safely classify the value yet. Callers at a
/// shared CSSOM boundary must preserve it rather than treating a bounded
/// grammar as proof that full-web CSS is invalid.
pub fn canonicalize_specified_longhand(name: &str, value: &str) -> Option<String> {
    if custom::contains_var(value) {
        return None;
    }
    let property = PropertyId::from_css_name(&name.to_ascii_lowercase())?;
    match value.trim().to_ascii_lowercase().as_str() {
        "initial" => Some("initial".to_string()),
        "inherit" => Some("inherit".to_string()),
        "unset" => Some("unset".to_string()),
        _ => PropertyValue::parse(property, value)
            .ok()
            .map(|parsed| parsed.to_css_string()),
    }
}

/// Canonicalize one specified CSSOM value covered by Livery's retained value
/// model.
///
/// Longhands use the generated property catalog. The border shorthand has one
/// additional reconstruction path so harvested `calc()` widths serialize
/// through the same value grammar while the authored style and color tokens
/// retain their CSSOM spelling.
pub fn canonicalize_specified_value(name: &str, value: &str) -> Option<String> {
    canonicalize_specified_longhand(name, value).or_else(|| {
        name.eq_ignore_ascii_case("border")
            .then(|| canonicalize_border(value))
            .flatten()
    })
}

fn canonicalize_border(value: &str) -> Option<String> {
    use values::{BorderStyle, BorderWidth, Color, LengthPercentage};

    if custom::contains_var(value) {
        return None;
    }
    let components = top_level_components(value)?;
    let mut width = false;
    let mut style = false;
    let mut color = false;
    let mut canonical = Vec::with_capacity(components.len());
    for component in components {
        if !width {
            if let Ok(parsed) = component.parse::<BorderWidth>() {
                width = true;
                canonical.push(parsed.to_string());
                continue;
            }
            if let Ok(parsed) = component.parse::<LengthPercentage>()
                && !parsed.has_percentage()
            {
                width = true;
                canonical.push(parsed.to_string());
                continue;
            }
        }
        if !style && component.parse::<BorderStyle>().is_ok() {
            style = true;
            canonical.push(component.to_string());
            continue;
        }
        if !color && component.parse::<Color>().is_ok() {
            color = true;
            canonical.push(component.to_string());
            continue;
        }
        return None;
    }
    (!canonical.is_empty()).then(|| canonical.join(" "))
}

fn top_level_components(value: &str) -> Option<Vec<&str>> {
    let mut components = Vec::new();
    let mut start = None;
    let mut depth = 0_u32;
    for (index, character) in value.char_indices() {
        match character {
            '(' => {
                depth = depth.checked_add(1)?;
                start.get_or_insert(index);
            },
            ')' => {
                depth = depth.checked_sub(1)?;
            },
            _ if character.is_ascii_whitespace() && depth == 0 => {
                if let Some(component_start) = start.take() {
                    components.push(&value[component_start..index]);
                }
            },
            _ => {
                start.get_or_insert(index);
            },
        }
    }
    if depth != 0 {
        return None;
    }
    if let Some(component_start) = start {
        components.push(&value[component_start..]);
    }
    Some(components)
}
