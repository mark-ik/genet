/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`checkbox`] / [`toggle`]: a `bool`-backed control flipped on click.

use crate::pod::GenetElement;
use crate::{El, OnClick, PointerClick, GenetCtx, View, el, on_click};

/// The toggle handler for [`checkbox`] / [`toggle`]: flip the bool on click.
fn flip(checked: &mut bool, _: PointerClick) {
    *checked = !*checked;
}

/// The concrete view type a checkbox / toggle produces: an `on_click`-wrapped
/// element reflecting the checked state.
pub type Checkbox = OnClick<El<&'static str, bool, ()>, bool, (), fn(&mut bool, PointerClick)>;

/// Build a checkbox-style control with the given `kind` class (`"checkbox"` /
/// `"toggle"`), reflecting `checked` as a textual indicator, an ARIA state, and
/// a `checked` class for styling.
fn build_check(kind: &'static str, checked: bool) -> Checkbox {
    // ASCII indicator (reliably renders without special fonts); the host styles
    // the `kind` / `checked` classes for the real look.
    let indicator = if checked { "[x]" } else { "[ ]" };
    let class = if checked { kind_checked(kind) } else { kind };
    let aria = if checked { "true" } else { "false" };
    let handler: fn(&mut bool, PointerClick) = flip;
    on_click(
        el::<_, bool, ()>("span", indicator)
            .attr("role", "checkbox")
            .attr("aria-checked", aria)
            .attr("class", class),
        handler,
    )
}

/// `"checkbox checked"` / `"toggle checked"` — the class string for a checked
/// control of the given `kind`.
fn kind_checked(kind: &'static str) -> &'static str {
    match kind {
        "toggle" => "toggle checked",
        _ => "checkbox checked",
    }
}

/// A reusable checkbox whose state *is* a `bool`. Clicking it toggles the bool;
/// it reflects the state as `role="checkbox"` + `aria-checked` (for the a11y
/// tree) and a `checkbox` / `checkbox checked` class (for host styling), with an
/// ASCII `[x]` / `[ ]` fallback indicator. Composes onto an app's bool field via
/// [`lens`](crate::lens), like [`text_field`](crate::text_field) onto a
/// [`TextInput`](crate::TextInput).
pub fn checkbox(checked: bool) -> impl View<bool, (), GenetCtx, Element = GenetElement> + use<> {
    build_check("checkbox", checked)
}

/// [`checkbox`] with its concrete return type named (for a host storing the
/// runner in a struct field; see
/// [`text_field_typed`](crate::text_field_typed)).
pub fn checkbox_typed(checked: bool) -> Checkbox {
    build_check("checkbox", checked)
}

/// A toggle switch — behaviourally a [`checkbox`] (a `bool`, flipped on click),
/// distinguished only by a `toggle` class so the host styles it as a switch.
pub fn toggle(checked: bool) -> impl View<bool, (), GenetCtx, Element = GenetElement> + use<> {
    build_check("toggle", checked)
}
