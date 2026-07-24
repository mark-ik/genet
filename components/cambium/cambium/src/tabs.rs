/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A tab strip: one strip of labelled tabs, one active — `radio_group`'s
//! one-of-N shape wearing the ARIA tabs pattern.
//!
//! Consumer-pull (merecat, 2026-07-15): three surfaces want the same widget and
//! were each about to hand-roll it — the Roster's data tabs (Nodes / Links /
//! Graphlets / Fields over one `data_grid`), the workbench's tile tabs, and a
//! stacked pane's tabs. One strip, one active index, click or arrow keys to
//! switch.
//!
//! Selection is the caller's state (like the grid's sort and scroll), so the
//! strip renders whatever the caller says is active and reports the change; what
//! a tab *shows* is the caller's business — the strip owns the strip.
//!
//! Roving tabindex per the ARIA tabs pattern: only the active tab is in the tab
//! order, and Left/Right move between tabs (wrapping), so a keyboard reaches the
//! strip once and then arrows within it. Home/End jump to the ends.
//!
//! The host styles the `tablist` container and the `tab` / `tab selected` tabs;
//! this sets no geometry. A strip is a row or a column or a scrolling overflow
//! depending on where it sits, and only the host knows which — so the strip
//! names its parts and leaves their shape alone, per the sheet contract.

use crate::pod::GenetElement;
use crate::{GenetCtx, Key, NamedKey, View, el, focusable_if, on_click, on_key};

/// The state of a tab strip: which tab is active, plus the group's accessible
/// name. Composable onto an app field via [`lens`](crate::lens), like
/// [`RadioGroup`](crate::RadioGroup).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabStrip {
    /// Index of the active tab in the `tabs` slice passed to [`tab_strip`]. Out
    /// of range renders every tab inactive.
    pub selected: usize,
    /// Accessible name announced for the strip.
    pub label: String,
}

impl TabStrip {
    /// A strip with `selected` active.
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            label: "Tabs".into(),
        }
    }

    /// Set the accessible name announced for the strip.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Move the active tab by `delta`, wrapping, over `len` tabs. The arrow-key
    /// step, exposed so a caller can drive the same motion from its own
    /// shortcut. A `len` of 0 leaves the selection alone.
    pub fn step(&mut self, delta: isize, len: usize) {
        if len == 0 {
            return;
        }
        let cur = self.selected.min(len - 1) as isize;
        self.selected = (cur + delta).rem_euclid(len as isize) as usize;
    }
}

impl Default for TabStrip {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A tab strip over a [`TabStrip`] and tab labels: one tab per label, clicking
/// one activates it.
///
/// Each tab is a `tab` (or `tab selected`) element with `role="tab"` and
/// `aria-selected`, inside a `tablist` container (`role="tablist"`). Left/Right
/// (and Home/End) move the active tab from the keyboard. `+ use<Action>` keeps
/// the opaque type from borrowing `state` / `tabs` (the labels are cloned in).
///
/// Generic over `Action` (like [`data_grid`](crate::data_grid), unlike the
/// `()`-actioned controls): the strip switching a tab is a state change, not an
/// action, so it emits none — and staying generic lets it sit in a view tree
/// whose siblings DO bubble actions without the caller reaching for
/// [`map_action`](crate::map_action). Compose onto an app field with
/// [`lens`](crate::lens).
pub fn tab_strip<Action>(
    state: &TabStrip,
    tabs: &[&str],
) -> impl View<TabStrip, Action, GenetCtx, Element = GenetElement> + use<Action>
where
    Action: 'static,
{
    let len = tabs.len();
    // One clickable tab per label. The per-tab closures share one type (one
    // closure definition capturing a `usize`), so the `Vec` is homogeneous.
    let items: Vec<_> = tabs
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let selected = i == state.selected;
            focusable_if(
                on_key(
                    on_click(
                        el::<_, TabStrip, Action>("div", (*label).to_string())
                            .attr("role", "tab")
                            .attr("aria-selected", if selected { "true" } else { "false" })
                            .attr("tabindex", if selected { "0" } else { "-1" })
                            .attr("class", if selected { "tab selected" } else { "tab" }),
                        move |s: &mut TabStrip, _| s.selected = i,
                    ),
                    move |s: &mut TabStrip, event| match event.key {
                        Key::Named(NamedKey::ArrowRight) => {
                            s.step(1, len);
                            event.prevent_default();
                        }
                        Key::Named(NamedKey::ArrowLeft) => {
                            s.step(-1, len);
                            event.prevent_default();
                        }
                        Key::Named(NamedKey::Home) => {
                            s.selected = 0;
                            event.prevent_default();
                        }
                        Key::Named(NamedKey::End) if len > 0 => {
                            s.selected = len - 1;
                            event.prevent_default();
                        }
                        _ => {}
                    },
                ),
                selected,
            )
        })
        .collect();
    el::<_, TabStrip, Action>("div", items)
        .attr("role", "tablist")
        .attr("class", "tablist")
        .attr("aria-label", state.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_holds_selection() {
        assert_eq!(TabStrip::new(2).selected, 2);
        assert_eq!(TabStrip::default().selected, 0);
        assert_eq!(TabStrip::default().label, "Tabs");
        assert_eq!(TabStrip::new(0).with_label("Roster").label, "Roster");
    }

    #[test]
    fn step_wraps_both_ways() {
        let mut s = TabStrip::new(0);
        s.step(-1, 4);
        assert_eq!(s.selected, 3, "left from the first wraps to the last");
        s.step(1, 4);
        assert_eq!(s.selected, 0, "right from the last wraps to the first");
        s.step(2, 4);
        assert_eq!(s.selected, 2);
    }

    #[test]
    fn step_is_inert_without_tabs_and_clamps_a_stale_selection() {
        let mut s = TabStrip::new(3);
        s.step(1, 0);
        assert_eq!(s.selected, 3, "no tabs: the selection is left alone");
        // A selection past the end (the caller shrank the tab set) clamps before
        // stepping rather than wrapping from a bogus index.
        let mut s = TabStrip::new(9);
        s.step(1, 3);
        assert_eq!(s.selected, 0);
    }
}
