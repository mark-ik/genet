/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Focused `style::ComputedValues` → [`taffy::Style`] converter.
//!
//! Probe-scope subset: display, size (width/height), margin, padding,
//! border-widths, and positioning (`position` + `top/right/bottom/left`
//! insets — relative + absolute, which Taffy models natively). Enough
//! to drive box-model semantics (`FragmentQuery::box_model` returning
//! real content/padding/border/margin rects), `DrawBorder` emission,
//! and offset/out-of-flow boxes. Stylo's richer property surface
//! (flex/grid track sizing, transforms, floats — which Taffy has no
//! model for) is covered by `linebender/blitz`'s `stylo_taffy` crate —
//! switch to it when our probe outgrows the subset here.
//!
//! Cf. `docs/2026-05-17_serval_layout_planes_architecture.md`.

use style::properties::ComputedValues;
use style::values::computed::{LengthPercentage as ComputedLengthPercentage, Percentage};
use style::values::generics::length::GenericMargin;
use style::values::generics::NonNegative;
use style::values::specified::box_::{DisplayInside, DisplayOutside, PositionProperty};
use style::values::specified::border::BorderStyle;
use taffy::prelude::TaffyAuto;
use taffy::style::{Dimension, Display, LengthPercentage, LengthPercentageAuto, Position, Style};
use taffy::geometry::Rect;

/// Convert `ComputedValues` into a [`taffy::Style`] subset.
///
/// Properties resolved (the rest left at `Default::default()`):
/// - `display` (Block / Flex / Grid / None; anything else falls back
///   to Block — Taffy doesn't model inline flow, so inline elements
///   collapse to block for the probe)
/// - `size.width`, `size.height` (Auto / Length / Percent)
/// - `margin.{top,right,bottom,left}` (Auto / Length / Percent)
/// - `padding.{top,right,bottom,left}` (Length / Percent)
/// - `border.{top,right,bottom,left}` (width × style; `none`/`hidden`
///   collapse to zero)
pub fn to_taffy_style(values: &ComputedValues) -> Style {
    let mut s = Style::default();
    s.display = convert_display(values);
    s.box_sizing = convert_box_sizing(values);
    s.position = convert_position(values);

    let pos = values.get_position();
    s.size = taffy::Size {
        width: dimension_from_size(&pos.width),
        height: dimension_from_size(&pos.height),
    };

    // Inset (top/right/bottom/left). Taffy applies it to relatively-
    // positioned boxes (offset from in-flow position) and to absolutely-
    // positioned boxes (offset from the containing block's padding edge).
    // For `static` boxes the cascade leaves these `auto`, so they have
    // no effect — matching CSS, which ignores inset on static elements.
    s.inset = Rect {
        top: inset_val(&pos.top),
        right: inset_val(&pos.right),
        bottom: inset_val(&pos.bottom),
        left: inset_val(&pos.left),
    };

    let margin = values.get_margin();
    s.margin = Rect {
        top: margin_val(&margin.margin_top),
        right: margin_val(&margin.margin_right),
        bottom: margin_val(&margin.margin_bottom),
        left: margin_val(&margin.margin_left),
    };

    let padding = values.get_padding();
    s.padding = Rect {
        top: length_percentage(&padding.padding_top.0),
        right: length_percentage(&padding.padding_right.0),
        bottom: length_percentage(&padding.padding_bottom.0),
        left: length_percentage(&padding.padding_left.0),
    };

    let border = values.get_border();
    s.border = Rect {
        top: border_width(border.border_top_width.0.to_f32_px(), border.border_top_style),
        right: border_width(
            border.border_right_width.0.to_f32_px(),
            border.border_right_style,
        ),
        bottom: border_width(
            border.border_bottom_width.0.to_f32_px(),
            border.border_bottom_style,
        ),
        left: border_width(
            border.border_left_width.0.to_f32_px(),
            border.border_left_style,
        ),
    };

    s
}

fn convert_box_sizing(values: &ComputedValues) -> taffy::style::BoxSizing {
    use style::properties::generated::longhands::box_sizing::computed_value::T as StyloBoxSizing;
    match values.get_position().box_sizing {
        StyloBoxSizing::BorderBox => taffy::style::BoxSizing::BorderBox,
        StyloBoxSizing::ContentBox => taffy::style::BoxSizing::ContentBox,
    }
}

/// Map Stylo's `position` to Taffy's. Taffy models only `Relative`
/// (in-flow, inset-offset) and `Absolute` (out-of-flow). `static`,
/// `relative`, and `sticky` all stay in normal flow → `Relative`;
/// `absolute` and `fixed` are out-of-flow → `Absolute`. (Taffy has
/// no separate `fixed` containing-block semantics; `fixed` falls back
/// to `absolute` against the nearest positioned ancestor — a known
/// approximation until a viewport-anchored pass lands.)
fn convert_position(values: &ComputedValues) -> Position {
    match values.get_box().position {
        PositionProperty::Absolute | PositionProperty::Fixed => Position::Absolute,
        _ => Position::Relative,
    }
}

fn convert_display(values: &ComputedValues) -> Display {
    let d = values.get_box().display;
    match d.outside() {
        DisplayOutside::None => return Display::None,
        _ => {}
    }
    match d.inside() {
        DisplayInside::None => Display::None,
        DisplayInside::Flex => Display::Flex,
        DisplayInside::Grid => Display::Grid,
        // Block flow + flow-root + inline (since Taffy doesn't model
        // inline flow, inline collapses to block — visible regression
        // when a real inline-aware layout backend lands).
        _ => Display::Block,
    }
}

/// `Position.width` / `.height` shape: `GenericSize<NonNegative<LengthPercentage>>`.
/// Taffy's `Dimension` accepts Auto / Length / Percent.
fn dimension_from_size(
    size: &style::values::generics::length::GenericSize<NonNegative<ComputedLengthPercentage>>,
) -> Dimension {
    use style::values::generics::length::GenericSize;
    match size {
        GenericSize::Auto => Dimension::AUTO,
        GenericSize::LengthPercentage(NonNegative(lp)) => match length_percentage_to_dimension(lp) {
            Some(d) => d,
            None => Dimension::AUTO,
        },
        // Min/max/fit-content / stretch / fill-available are intrinsic
        // sizing modes Taffy doesn't expose at the Dimension level —
        // fall back to Auto for the probe.
        _ => Dimension::AUTO,
    }
}

/// `Margin.margin_*` is `GenericMargin<LengthPercentage>` (Auto |
/// LengthPercentage | anchor-positioning variants).
fn margin_val(m: &GenericMargin<ComputedLengthPercentage>) -> LengthPercentageAuto {
    match m {
        GenericMargin::Auto => LengthPercentageAuto::AUTO,
        GenericMargin::LengthPercentage(lp) => match unpack_length_percentage(lp) {
            UnpackResult::Length(px) => LengthPercentageAuto::length(px),
            UnpackResult::Percent(p) => LengthPercentageAuto::percent(p),
            UnpackResult::Calc => LengthPercentageAuto::AUTO,
        },
        _ => LengthPercentageAuto::AUTO,
    }
}

/// `Position.{top,right,bottom,left}` is `computed::Inset` —
/// `GenericInset<Percentage, LengthPercentage>` (Auto | LengthPercentage
/// | anchor-positioning variants). Anchor variants fall back to `auto`.
fn inset_val(v: &style::values::computed::Inset) -> LengthPercentageAuto {
    use style::values::generics::position::GenericInset;
    match v {
        GenericInset::Auto => LengthPercentageAuto::AUTO,
        GenericInset::LengthPercentage(lp) => match unpack_length_percentage(lp) {
            UnpackResult::Length(px) => LengthPercentageAuto::length(px),
            UnpackResult::Percent(p) => LengthPercentageAuto::percent(p),
            UnpackResult::Calc => LengthPercentageAuto::AUTO,
        },
        _ => LengthPercentageAuto::AUTO,
    }
}

/// `Padding.padding_*` is `NonNegativeLengthPercentage` — `NonNegative<LengthPercentage>`.
fn length_percentage(lp: &ComputedLengthPercentage) -> LengthPercentage {
    match unpack_length_percentage(lp) {
        UnpackResult::Length(px) => LengthPercentage::length(px),
        UnpackResult::Percent(p) => LengthPercentage::percent(p),
        UnpackResult::Calc => LengthPercentage::length(0.0),
    }
}

/// Border width: zero for `none` / `hidden` styles, the literal width
/// otherwise. Matches CSS spec — `border: none` paints no border
/// regardless of width.
fn border_width(width_px: f32, style: BorderStyle) -> LengthPercentage {
    if style.none_or_hidden() {
        return LengthPercentage::length(0.0);
    }
    LengthPercentage::length(width_px)
}

enum UnpackResult {
    Length(f32),
    Percent(f32),
    Calc,
}

fn unpack_length_percentage(lp: &ComputedLengthPercentage) -> UnpackResult {
    use style::values::computed::length_percentage::Unpacked;
    match lp.unpack() {
        Unpacked::Length(l) => UnpackResult::Length(l.px()),
        Unpacked::Percentage(Percentage(p)) => UnpackResult::Percent(p),
        Unpacked::Calc(_) => UnpackResult::Calc,
    }
}

/// LengthPercentage in dimension-context (size). Returns None for
/// values that can't sensibly map (e.g., calc, which we collapse to
/// Auto rather than guessing).
fn length_percentage_to_dimension(lp: &ComputedLengthPercentage) -> Option<Dimension> {
    match unpack_length_percentage(lp) {
        UnpackResult::Length(px) => Some(Dimension::length(px)),
        UnpackResult::Percent(p) => Some(Dimension::percent(p)),
        UnpackResult::Calc => None,
    }
}

