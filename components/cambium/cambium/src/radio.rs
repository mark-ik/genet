/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A radio-button group with component-owned selection and keyboard behavior.
//!
//! State is which option is selected; clicking one selects it and (since the
//! group is a single index) deselects the rest. The group owns the WAI-ARIA
//! radio pattern's Space activation, wrapping Arrow-key movement, roving
//! `tabindex`, and focus transfer. `Home` and `End` are retained as Cambium
//! extensions. ARIA roles expose the component's semantics; the generic runner
//! does not infer behavior from them.

use crate::pod::GenetElement;
use crate::{GenetCtx, Key, NamedKey, View, el, on_click, on_key, request_focus};

/// The state of a radio group: the index of the selected option in the
/// `options` slice passed to [`radio_group`]. Composable via [`lens`](crate::lens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioGroup {
    /// Index of the chosen option. Out of range renders all options unselected.
    pub selected: usize,
    /// Accessible name announced for the group.
    pub label: String,
    /// Retained target used to move DOM focus after keyboard navigation.
    pub focus_request: Option<usize>,
}

impl RadioGroup {
    /// A group with `selected` chosen.
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            label: "Options".into(),
            focus_request: None,
        }
    }

    /// Set the accessible name announced for the group.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

impl Default for RadioGroup {
    fn default() -> Self {
        Self::new(0)
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
) -> impl View<RadioGroup, (), GenetCtx, Element = GenetElement> + use<> {
    let option_count = options.len();
    let active = if state.selected < option_count {
        state.selected
    } else {
        0
    };
    // One clickable row per option. The per-option closures share one type (one
    // closure definition capturing a `usize`), so the `Vec` is homogeneous.
    let items: Vec<_> = options
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let selected = i == state.selected;
            let active = i == active;
            let indicator = if selected { "(o) " } else { "( ) " };
            let item = on_click(
                el::<_, RadioGroup, ()>("div", format!("{indicator}{label}"))
                    .attr("role", "radio")
                    .attr("aria-checked", if selected { "true" } else { "false" })
                    .attr("tabindex", if active { "0" } else { "-1" })
                    .attr("class", if selected { "radio selected" } else { "radio" }),
                move |s: &mut RadioGroup, _| {
                    s.selected = i;
                    s.focus_request = None;
                },
            );
            let keyboard = on_key(item, move |state: &mut RadioGroup, event| {
                let next = match event.key {
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                        (i + option_count - 1) % option_count
                    },
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                        (i + 1) % option_count
                    },
                    // Cambium extensions retained from the original runner behavior.
                    Key::Named(NamedKey::Home) => 0,
                    Key::Named(NamedKey::End) => option_count - 1,
                    Key::Named(NamedKey::Space) => i,
                    _ => return,
                };
                state.selected = next;
                state.focus_request = Some(next);
                event.prevent_default();
            })
            .focusable(active);
            request_focus(keyboard, state.focus_request == Some(i))
        })
        .collect();
    el::<_, RadioGroup, ()>("div", items)
        .attr("role", "radiogroup")
        .attr("aria-label", state.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_holds_selection() {
        assert_eq!(RadioGroup::new(2).selected, 2);
        assert_eq!(RadioGroup::default().selected, 0);
        assert_eq!(RadioGroup::default().label, "Options");
        assert_eq!(RadioGroup::default().focus_request, None);
    }
}
