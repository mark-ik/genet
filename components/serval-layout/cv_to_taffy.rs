/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `style::ComputedValues` → [`taffy::Style`] via the maintained
//! [`stylo_taffy`] converters.
//!
//! This is a thin adapter, not a hand-rolled mapping. The per-property
//! logic (calc, intrinsic-sizing keywords, flex, **float / clear**)
//! lives in `stylo_taffy::convert::*`; this function assembles those
//! into the `taffy::Style` shape serval's `TaffyTree` stores.
//!
//! ## Why not call `stylo_taffy::to_taffy_style` directly
//!
//! `stylo_taffy::to_taffy_style` returns `taffy::Style<Atom>` — the
//! `Atom` type parameter carries CSS grid *template line names*. serval's
//! `TaffyTree<InlineContent>` stores the default `taffy::Style`
//! (`Style<DefaultCheapStr>`), and `TaffyTree` isn't generic over the
//! ident type, so the two `Style<S>` instantiations don't unify. The
//! grid line-name fields are the *only* place `Atom` appears, and serval
//! doesn't lay out named grid lines yet — so we assemble a default-ident
//! `Style` from the (ident-free) per-property converters and leave the
//! grid-template-name fields at their default. When serval grows named
//! grid support, revisit (carry `Atom` through `TaffyTree`, or map the
//! line-name sets).
//!
//! Cf. `docs/2026-05-20_stylo_taffy_adoption_plan.md`.

use style::properties::ComputedValues;
use stylo_taffy::convert as c;

/// Convert `ComputedValues` into serval's `taffy::Style`, delegating
/// every property to `stylo_taffy::convert`. Grid *template* tracks and
/// line-names are left at default (they carry the `Atom` ident type that
/// serval's `TaffyTree` Style instantiation doesn't use); everything
/// else — display, box-sizing, position + inset, overflow, float/clear,
/// sizing (incl. min/max + aspect-ratio), margin/padding/border, gap,
/// and the flexbox properties — comes from the maintained converters.
pub fn to_taffy_style(values: &ComputedValues) -> taffy::Style {
    let pos = values.get_position();
    let margin = values.get_margin();
    let padding = values.get_padding();
    let border = values.get_border();

    let mut s = taffy::Style::default();

    s.display = c::display(values.clone_display());
    s.box_sizing = c::box_sizing(values.clone_box_sizing());
    s.position = c::position(values.clone_position());
    s.overflow = taffy::Point {
        x: c::overflow(values.clone_overflow_x()),
        y: c::overflow(values.clone_overflow_y()),
    };

    // Floats (stylo_taffy `floats` feature → taffy `float_layout`).
    s.float = c::float(values.clone_float());
    s.clear = c::clear(values.clone_clear());

    s.size = taffy::Size {
        width: c::dimension(&pos.width),
        height: c::dimension(&pos.height),
    };
    s.min_size = taffy::Size {
        width: c::dimension(&pos.min_width),
        height: c::dimension(&pos.min_height),
    };
    s.max_size = taffy::Size {
        width: c::max_size_dimension(&pos.max_width),
        height: c::max_size_dimension(&pos.max_height),
    };
    s.aspect_ratio = c::aspect_ratio(pos.aspect_ratio);

    s.inset = taffy::Rect {
        left: c::inset(&pos.left),
        right: c::inset(&pos.right),
        top: c::inset(&pos.top),
        bottom: c::inset(&pos.bottom),
    };
    s.margin = taffy::Rect {
        left: c::margin(&margin.margin_left),
        right: c::margin(&margin.margin_right),
        top: c::margin(&margin.margin_top),
        bottom: c::margin(&margin.margin_bottom),
    };
    s.padding = taffy::Rect {
        left: c::length_percentage(&padding.padding_left.0),
        right: c::length_percentage(&padding.padding_right.0),
        top: c::length_percentage(&padding.padding_top.0),
        bottom: c::length_percentage(&padding.padding_bottom.0),
    };
    s.border = taffy::Rect {
        left: c::border(&border.border_left_width, border.border_left_style),
        right: c::border(&border.border_right_width, border.border_right_style),
        top: c::border(&border.border_top_width, border.border_top_style),
        bottom: c::border(&border.border_bottom_width, border.border_bottom_style),
    };

    s.gap = taffy::Size {
        width: c::gap(&pos.column_gap),
        height: c::gap(&pos.row_gap),
    };

    s.align_content = c::content_alignment(pos.align_content);
    s.justify_content = c::content_alignment(pos.justify_content);
    s.align_items = c::item_alignment(pos.align_items.0);
    s.align_self = c::item_alignment(pos.align_self.0);

    s.flex_direction = c::flex_direction(pos.flex_direction);
    s.flex_wrap = c::flex_wrap(pos.flex_wrap);
    s.flex_grow = pos.flex_grow.0;
    s.flex_shrink = pos.flex_shrink.0;
    s.flex_basis = c::flex_basis(&pos.flex_basis);

    s
}
