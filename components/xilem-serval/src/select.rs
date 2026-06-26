/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A self-positioning dropdown [`select`] control.
//!
//! The T2 form control the serval-as-host plan flagged as needing "simple
//! overlay/popup positioning" — now available via serval's inline-style support.
//! Unlike the host-driven [`overlay_at`](crate::overlay_at) (whose `(x, y)` only
//! the host knows post-layout), `select` positions its own option list with **no
//! host plumbing**: the list is `position: absolute; top: 100%` inside a
//! `position: relative` select box, so it lands directly below the box. That
//! makes `select` a fully self-contained, [`lens`](crate::lens)-composable
//! control like [`checkbox`](crate::checkbox) / [`text_field`](crate::text_field).
//!
//! Stacking: the option list is `position: absolute`, so it auto-lifts above
//! in-flow content (serval-layout's CSS 2.1 Appendix E stacking + z-index). To sit
//! above a *later positioned* sibling, give the open list (or the select) a higher
//! `z-index`; the old "place the select last" workaround is no longer required.

use crate::pod::ServalElement;
use crate::{ServalCtx, View, el, on_click};

/// The state of a [`select`]: which option is chosen, and whether the option
/// list is open. Composes onto an app field via [`lens`](crate::lens), like the
/// other controls' state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectState {
    /// Index of the chosen option in the `options` slice passed to [`select`].
    /// Out of range (e.g. empty options) renders an empty box.
    pub selected: usize,
    /// Whether the option list is showing.
    pub open: bool,
}

impl SelectState {
    /// A closed select with `selected` chosen.
    pub fn new(selected: usize) -> Self {
        Self { selected, open: false }
    }

    /// The chosen option's label from `options`, or `""` if out of range.
    pub fn label<'a>(&self, options: &[&'a str]) -> &'a str {
        options.get(self.selected).copied().unwrap_or("")
    }
}

/// A dropdown select over a [`SelectState`] and a list of option labels.
///
/// Renders a `select-box` showing the chosen label; clicking it toggles the
/// `select-list` of `select-option`s. Clicking an option chooses it (sets
/// `selected`) and closes the list. The box/list/option carry classes for host
/// styling and `role="listbox"`/`"option"` for the a11y tree; the *positioning*
/// (relative box + `top: 100%` absolute list) rides inline styles, so it works
/// regardless of the app stylesheet.
///
/// `+ use<>` keeps the opaque type from capturing the `state`/`options` borrows:
/// the view owns its strings (each label is cloned in), so it is a single `V`
/// usable as `FnMut(&_) -> V` app logic.
pub fn select(
    state: &SelectState,
    options: &[&str],
) -> impl View<SelectState, (), ServalCtx, Element = ServalElement> + use<> {
    // The closed box: the selected label; clicking toggles the list.
    let toggle: fn(&mut SelectState, crate::PointerClick) = |s, _| s.open = !s.open;
    let box_view = on_click(
        el::<_, SelectState, ()>("div", state.label(options).to_string())
            .attr("class", "select-box"),
        toggle,
    );

    // The option list (only when open): an absolute box at `top: 100%` of the
    // relative select root, so it sits directly below the box. Each option sets
    // `selected` to its index and closes the list. The per-option closures all
    // share one type (one closure definition, capturing a `usize`), so the `Vec`
    // is homogeneous.
    let list = state.open.then(|| {
        let items: Vec<_> = options
            .iter()
            .enumerate()
            .map(|(i, label)| {
                on_click(
                    el::<_, SelectState, ()>("div", label.to_string())
                        .attr("class", "select-option")
                        .attr("role", "option"),
                    move |s: &mut SelectState, _| {
                        s.selected = i;
                        s.open = false;
                    },
                )
            })
            .collect();
        el::<_, SelectState, ()>("div", items)
            .attr("class", "select-list")
            .attr("style", "position: absolute; top: 100%; left: 0;")
    });

    el::<_, SelectState, ()>("div", (box_view, list))
        .attr("class", "select")
        .attr("role", "listbox")
        .attr("style", "position: relative;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_reads_selected_or_empty() {
        let opts = ["red", "green", "blue"];
        assert_eq!(SelectState::new(1).label(&opts), "green");
        // Out of range → empty.
        assert_eq!(SelectState::new(9).label(&opts), "");
        // Empty options → empty.
        assert_eq!(SelectState::new(0).label(&[]), "");
    }
}
