/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `getComputedStyle` support: serialize a node's computed value for a single CSS
//! longhand to its CSS string.
//!
//! A curated first cut over the common longhands (extended as needed). These are
//! **computed** values, not fully resolved/used values: layout-dependent
//! properties (`width`, `height`, …) report their computed value (e.g. `auto`),
//! not the used pixel length — that resolved-value path is deeper work. An
//! unsupported property returns `None`, which the host's `getComputedStyle`
//! surfaces as `""`.

use std::hash::Hash;

use style_traits::ToCss;

use crate::style::StylePlane;

/// Serialize the computed value of `property` (a CSS longhand name) for `node`,
/// or `None` if the node has no computed style or the property is unsupported.
pub fn computed_value_string<NodeId: Copy + Eq + Hash>(
    styles: &StylePlane<NodeId>,
    node: NodeId,
    property: &str,
) -> Option<String> {
    let entry = styles.get(node)?;
    let data = entry.borrow_data()?;
    let cv = data.styles.primary();
    Some(match property {
        "color" => cv.clone_color().to_css_string(),
        "background-color" => cv.clone_background_color().to_css_string(),
        "display" => cv.clone_display().to_css_string(),
        "visibility" => cv.clone_visibility().to_css_string(),
        "opacity" => cv.clone_opacity().to_css_string(),
        "position" => cv.clone_position().to_css_string(),
        // Box insets: what CSS-animation interpolation tests assert most (an
        // animated `left` read back mid-flight via getComputedStyle).
        "left" => cv.clone_left().to_css_string(),
        "right" => cv.clone_right().to_css_string(),
        "top" => cv.clone_top().to_css_string(),
        "bottom" => cv.clone_bottom().to_css_string(),
        "transform" => cv.clone_transform().to_css_string(),
        "overflow-x" => cv.clone_overflow_x().to_css_string(),
        "overflow-y" => cv.clone_overflow_y().to_css_string(),
        "font-size" => cv.clone_font_size().to_css_string(),
        "font-family" => cv.clone_font_family().to_css_string(),
        "font-weight" => cv.clone_font_weight().to_css_string(),
        "font-style" => cv.clone_font_style().to_css_string(),
        "line-height" => cv.clone_line_height().to_css_string(),
        "text-align" => cv.clone_text_align().to_css_string(),
        "white-space-collapse" => cv.clone_white_space_collapse().to_css_string(),
        "width" => cv.clone_width().to_css_string(),
        "height" => cv.clone_height().to_css_string(),
        "margin-top" => cv.clone_margin_top().to_css_string(),
        "margin-right" => cv.clone_margin_right().to_css_string(),
        "margin-bottom" => cv.clone_margin_bottom().to_css_string(),
        "margin-left" => cv.clone_margin_left().to_css_string(),
        "padding-top" => cv.clone_padding_top().to_css_string(),
        "padding-right" => cv.clone_padding_right().to_css_string(),
        "padding-bottom" => cv.clone_padding_bottom().to_css_string(),
        "padding-left" => cv.clone_padding_left().to_css_string(),
        _ => return None,
    })
}
