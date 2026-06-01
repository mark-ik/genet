/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A radio-button group: the simplest of the remaining T2 controls, the
//! `select` pattern without the dropdown.
//!
//! State is which option is selected; clicking one selects it and (since the
//! group is a single index) deselects the rest. No engine work, no overlay, no
//! drag: one `on_click` element per option, reflecting the selection via a
//! class plus `role="radio"` / `aria-checked`. Composable onto an app field via
//! [`lens`](crate::lens), like [`checkbox`](crate::checkbox) /
//! [`select`](crate::select).

use crate::pod::ServalElement;
use crate::{ServalCtx, View, el, on_click};

/// The state of a radio group: the index of the selected option in the
/// `options` slice passed to [`radio_group`]. Composable via [`lens`](crate::lens).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RadioGroup {
    /// Index of the chosen option. Out of range renders all options unselected.
    pub selected: usize,
}

impl RadioGroup {
    /// A group with `selected` chosen.
    pub fn new(selected: usize) -> Self {
        Self { selected }
    }
}

/// A radio group over a [`RadioGroup`] and option labels: one row per option,
/// clicking a row selects it (and only it).
///
/// Each row is a `radio` (or `radio selected`) element with `role="radio"` and
/// `aria-checked`, an ASCII `(o)` / `( )` indicator before the label (so it
/// reads without special fonts), inside a `role="radiogroup"` container. The
/// host styles the classes. `+ use<>` keeps the opaque type from borrowing
/// `state` / `options` (the labels are cloned in).
pub fn radio_group(
    state: &RadioGroup,
    options: &[&str],
) -> impl View<RadioGroup, (), ServalCtx, Element = ServalElement> + use<> {
    // One clickable row per option. The per-option closures share one type (one
    // closure definition capturing a `usize`), so the `Vec` is homogeneous.
    let items: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let selected = i == state.selected;
            let indicator = if selected { "(o) " } else { "( ) " };
            on_click(
                el::<_, RadioGroup, ()>("div", format!("{indicator}{label}"))
                    .attr("role", "radio")
                    .attr("aria-checked", if selected { "true" } else { "false" })
                    .attr("class", if selected { "radio selected" } else { "radio" }),
                move |s: &mut RadioGroup, _| s.selected = i,
            )
        })
        .collect();
    el::<_, RadioGroup, ()>("div", items).attr("role", "radiogroup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_holds_selection() {
        assert_eq!(RadioGroup::new(2).selected, 2);
        assert_eq!(RadioGroup::default().selected, 0);
    }
}
