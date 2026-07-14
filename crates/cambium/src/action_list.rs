/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A shared, searchable action list for menus, palettes, and choosers.

use crate::pod::GenetElement;
use crate::{Action, GenetCtx, Key, NamedKey, View, el, on_click, on_key};

/// One action shown by [`action_list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionItem {
    pub label: String,
    pub shortcut: Option<String>,
    pub disabled: bool,
}

impl ActionItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            shortcut: None,
            disabled: false,
        }
    }

    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Retained interaction state for an [`action_list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionListState {
    pub query: String,
    pub selected: usize,
    pub label: String,
    pub id: String,
}

impl Default for ActionListState {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            label: "Actions".into(),
            id: "cambium-action-list".into(),
        }
    }
}

impl ActionListState {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Set a DOM id prefix. Use a distinct value when a view contains multiple
    /// action lists so `aria-controls` and `aria-activedescendant` stay unique.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

/// Events emitted by an [`action_list`]. The index refers to the original,
/// unfiltered `items` slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionListEvent {
    Activate(usize),
    Dismiss,
}

impl Action for ActionListEvent {}

/// Build a searchable action list with one Tab stop.
///
/// Typing filters by label, Backspace edits the query, arrows and Home/End move
/// selection, Enter activates, and Escape emits [`ActionListEvent::Dismiss`].
/// Pointer activation follows the same event path. Disabled items remain
/// visible but are skipped by keyboard navigation and emit nothing on click.
pub fn action_list(
    state: &ActionListState,
    items: &[ActionItem],
) -> impl View<ActionListState, ActionListEvent, GenetCtx, Element = GenetElement> + use<> {
    let query = state.query.to_lowercase();
    let filtered: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.label.to_lowercase().contains(&query))
        .map(|(index, item)| (index, item.clone()))
        .collect();
    let enabled: Vec<_> = filtered
        .iter()
        .enumerate()
        .filter_map(|(position, (_, item))| (!item.disabled).then_some(position))
        .collect();
    let selected = nearest_enabled(state.selected, &enabled);
    let active_id = selected
        .and_then(|position| filtered.get(position))
        .map(|(index, _)| format!("{}-option-{index}", state.id))
        .unwrap_or_default();

    let rows: Vec<_> = filtered
        .iter()
        .enumerate()
        .map(|(position, (index, item))| {
            let original_index = *index;
            let disabled = item.disabled;
            let selected = Some(position) == selected;
            let content = match &item.shortcut {
                Some(shortcut) => format!("{}\t{shortcut}", item.label),
                None => item.label.clone(),
            };
            on_click(
                el::<_, ActionListState, ActionListEvent>("div", content)
                    .attr("id", format!("{}-option-{index}", state.id))
                    .attr(
                        "class",
                        if selected {
                            "action-item selected"
                        } else {
                            "action-item"
                        },
                    )
                    .attr("role", "option")
                    .attr("aria-selected", if selected { "true" } else { "false" })
                    .attr("aria-disabled", if disabled { "true" } else { "false" }),
                move |s: &mut ActionListState, _| {
                    if disabled {
                        None
                    } else {
                        s.selected = position;
                        Some(ActionListEvent::Activate(original_index))
                    }
                },
            )
        })
        .collect();

    let list_id = format!("{}-options", state.id);
    let query_text = if state.query.is_empty() {
        state.label.clone()
    } else {
        format!("{}: {}", state.label, state.query)
    };
    let enabled_for_key = enabled.clone();
    let original_indices: Vec<_> = filtered.iter().map(|(index, _)| *index).collect();
    on_key(
        el::<_, ActionListState, ActionListEvent>(
            "div",
            (
                el::<_, ActionListState, ActionListEvent>("div", query_text)
                    .attr("class", "action-list-query")
                    .attr("aria-hidden", "true"),
                el::<_, ActionListState, ActionListEvent>("div", rows)
                    .attr("id", list_id.clone())
                    .attr("class", "action-list-options")
                    .attr("role", "listbox")
                    .attr("aria-label", state.label.clone()),
            ),
        )
        .attr("class", "action-list")
        .attr("role", "combobox")
        .attr("aria-label", state.label.clone())
        .attr("aria-autocomplete", "list")
        .attr("aria-expanded", "true")
        .attr("aria-controls", list_id)
        .attr("aria-activedescendant", active_id)
        .attr("tabindex", "0"),
        move |s: &mut ActionListState, event| {
            let current = nearest_enabled(s.selected, &enabled_for_key);
            let output = match &event.key {
                Key::Named(NamedKey::ArrowDown) => {
                    s.selected = next_enabled(current, &enabled_for_key, true);
                    None
                }
                Key::Named(NamedKey::ArrowUp) => {
                    s.selected = next_enabled(current, &enabled_for_key, false);
                    None
                }
                Key::Named(NamedKey::Home) => {
                    s.selected = enabled_for_key.first().copied().unwrap_or(0);
                    None
                }
                Key::Named(NamedKey::End) => {
                    s.selected = enabled_for_key.last().copied().unwrap_or(0);
                    None
                }
                Key::Named(NamedKey::Enter) => current
                    .and_then(|position| original_indices.get(position).copied())
                    .map(ActionListEvent::Activate),
                Key::Named(NamedKey::Escape) => Some(ActionListEvent::Dismiss),
                Key::Named(NamedKey::Backspace) => {
                    s.query.pop();
                    s.selected = 0;
                    None
                }
                Key::Named(NamedKey::Space) => {
                    s.query.push(' ');
                    s.selected = 0;
                    None
                }
                Key::Character(text) if !event.mods.ctrl && !event.mods.alt && !event.mods.meta => {
                    s.query.push_str(text);
                    s.selected = 0;
                    None
                }
                _ => return None,
            };
            event.prevent_default();
            output
        },
    )
}

fn nearest_enabled(selected: usize, enabled: &[usize]) -> Option<usize> {
    enabled
        .iter()
        .copied()
        .find(|&position| position >= selected)
        .or_else(|| enabled.first().copied())
}

fn next_enabled(current: Option<usize>, enabled: &[usize], forward: bool) -> usize {
    let Some(current) = current else {
        return enabled.first().copied().unwrap_or(0);
    };
    let index = enabled
        .iter()
        .position(|&position| position == current)
        .unwrap_or(0);
    if forward {
        enabled
            .get(index + 1)
            .or_else(|| enabled.first())
            .copied()
            .unwrap_or(0)
    } else {
        index
            .checked_sub(1)
            .and_then(|previous| enabled.get(previous))
            .or_else(|| enabled.last())
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DomHandle, GenetAppRunner, KeyEvent};
    use serval_scripted_dom::ScriptedDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn disabled_positions_are_skipped_and_wrap() {
        let enabled = [0, 2, 4];
        assert_eq!(nearest_enabled(1, &enabled), Some(2));
        assert_eq!(next_enabled(Some(4), &enabled, true), 0);
        assert_eq!(next_enabled(Some(0), &enabled, false), 4);
    }

    #[test]
    fn keyboard_filters_moves_and_activates_original_index() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom,
            |state: &ActionListState| {
                action_list(
                    state,
                    &[
                        ActionItem::new("Open"),
                        ActionItem::new("Close").disabled(true),
                        ActionItem::new("Copy"),
                    ],
                )
            },
            ActionListState::default(),
        );
        runner.set_focus(Some(runner.root()));

        runner.dispatch_key(KeyEvent::new(Key::Character("o".into())));
        assert_eq!(runner.state().query, "o");

        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        assert!(actions.is_empty());
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(actions, [ActionListEvent::Activate(2)]);
    }
}
