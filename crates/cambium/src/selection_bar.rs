/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! One roving-focus engine for tabs, segmented controls, and filter chips.

use crate::{
    GenetCtx, GenetElement, Key, NamedKey, View, el, focusable_if, on_click, on_key, request_focus,
};

/// One choice in a [`selection_bar`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionItem {
    pub id: String,
    pub label: String,
    pub disabled: bool,
    pub disabled_reason: Option<String>,
    /// Panel controlled by this item when rendered as a tab.
    pub panel_id: Option<String>,
}

impl SelectionItem {
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        Self {
            id: label.clone(),
            label,
            disabled: false,
            disabled_reason: None,
            panel_id: None,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn controls(mut self, panel_id: impl Into<String>) -> Self {
        self.panel_id = Some(panel_id.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        if !disabled {
            self.disabled_reason = None;
        }
        self
    }

    pub fn disabled_because(mut self, reason: impl Into<String>) -> Self {
        self.disabled = true;
        self.disabled_reason = Some(reason.into());
        self
    }
}

/// Shared focus and selection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionState {
    pub active: usize,
    pub selected: Vec<usize>,
    pub label: String,
    pub id: String,
    /// Retained edge used to move DOM focus after keyboard navigation.
    pub focus_active: bool,
}

impl SelectionState {
    pub fn single(selected: usize) -> Self {
        Self {
            active: selected,
            selected: vec![selected],
            label: "Selection".into(),
            id: "cambium-selection".into(),
            focus_active: false,
        }
    }

    pub fn multiple(selected: impl IntoIterator<Item = usize>) -> Self {
        let mut unique = Vec::new();
        for index in selected {
            if !unique.contains(&index) {
                unique.push(index);
            }
        }
        Self {
            active: unique.first().copied().unwrap_or(0),
            selected: unique,
            label: "Filters".into(),
            id: "cambium-selection".into(),
            focus_active: false,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    fn select_one(&mut self, index: usize) {
        self.selected.clear();
        self.selected.push(index);
    }

    fn toggle(&mut self, index: usize) {
        if let Some(position) = self.selected.iter().position(|selected| *selected == index) {
            self.selected.remove(position);
        } else {
            self.selected.push(index);
            self.selected.sort_unstable();
        }
    }
}

impl Default for SelectionState {
    fn default() -> Self {
        Self::single(0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

impl Orientation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// Whether moving focus in a tab list also selects the newly focused tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabActivation {
    #[default]
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionBarKind {
    Tabs { activation: TabActivation },
    Segmented,
    FilterChips,
}

/// Pattern and axis configuration for [`selection_bar`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionBarConfig {
    pub kind: SelectionBarKind,
    pub orientation: Orientation,
}

impl SelectionBarConfig {
    pub fn tabs(activation: TabActivation) -> Self {
        Self {
            kind: SelectionBarKind::Tabs { activation },
            orientation: Orientation::Horizontal,
        }
    }

    pub fn segmented() -> Self {
        Self {
            kind: SelectionBarKind::Segmented,
            orientation: Orientation::Horizontal,
        }
    }

    pub fn filter_chips() -> Self {
        Self {
            kind: SelectionBarKind::FilterChips,
            orientation: Orientation::Horizontal,
        }
    }

    pub fn vertical(mut self) -> Self {
        self.orientation = Orientation::Vertical;
        self
    }
}

/// Render a configured selection bar.
pub fn selection_bar(
    state: &SelectionState,
    items: &[SelectionItem],
    config: SelectionBarConfig,
) -> impl View<SelectionState, (), GenetCtx, Element = GenetElement> + use<> {
    let items = items.to_vec();
    let enabled = enabled_positions(&items);
    let active = nearest_enabled(state.active, &enabled);
    let children: Vec<_> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let is_active = Some(index) == active;
            let selected = state.is_selected(index);
            let reason = item.disabled_reason.as_ref().map(|reason| {
                el::<_, SelectionState, ()>("span", reason.clone())
                    .attr("class", "selection-disabled-reason")
                    .attr("aria-hidden", "true")
            });
            let mut item_view = el::<_, SelectionState, ()>(
                "div",
                (
                    el::<_, SelectionState, ()>("span", item.label.clone())
                        .attr("class", "selection-label"),
                    reason,
                ),
            )
            .attr("id", item_dom_id(state, item))
            .attr(
                "class",
                if selected {
                    "selection-item selected"
                } else {
                    "selection-item"
                },
            )
            .attr("role", config.item_role())
            .attr("tabindex", if is_active { "0" } else { "-1" })
            .attr(
                "aria-disabled",
                if item.disabled { "true" } else { "false" },
            );
            match config.kind {
                SelectionBarKind::Tabs { .. } => {
                    item_view =
                        item_view.attr("aria-selected", if selected { "true" } else { "false" });
                    if let Some(panel_id) = &item.panel_id {
                        item_view = item_view.attr("aria-controls", panel_id.clone());
                    }
                }
                SelectionBarKind::Segmented => {
                    item_view =
                        item_view.attr("aria-checked", if selected { "true" } else { "false" });
                }
                SelectionBarKind::FilterChips => {
                    item_view =
                        item_view.attr("aria-pressed", if selected { "true" } else { "false" });
                }
            }
            if let Some(reason) = &item.disabled_reason {
                item_view = item_view.attr("aria-description", reason.clone());
            }

            let disabled = item.disabled;
            let kind = config.kind;
            request_focus(
                focusable_if(
                    on_click(item_view, move |state: &mut SelectionState, _| {
                        if !disabled {
                            state.active = index;
                            state.focus_active = false;
                            activate(state, index, kind);
                        }
                    }),
                    is_active && !disabled,
                ),
                state.focus_active && is_active && !disabled,
            )
        })
        .collect();

    let root = el::<_, SelectionState, ()>("div", children)
        .attr("id", state.id.clone())
        .attr("class", config.root_class())
        .attr("role", config.root_role())
        .attr("aria-label", state.label.clone())
        .attr("aria-orientation", config.orientation.as_str());
    let items_for_key = items.clone();
    on_key(root, move |state: &mut SelectionState, event| {
        let enabled = enabled_positions(&items_for_key);
        let current = nearest_enabled(state.active, &enabled);
        let next = match &event.key {
            Key::Named(NamedKey::Home) => enabled.first().copied(),
            Key::Named(NamedKey::End) => enabled.last().copied(),
            key if moves_previous(key, config) => Some(next_enabled(current, &enabled, false)),
            key if moves_next(key, config) => Some(next_enabled(current, &enabled, true)),
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                if let Some(current) = current {
                    activate(state, current, config.kind);
                    state.focus_active = true;
                }
                event.prevent_default();
                return;
            }
            _ => return,
        };
        if let Some(next) = next {
            state.active = next;
            state.focus_active = true;
            if automatically_selects(config.kind) {
                state.select_one(next);
            }
        }
        event.prevent_default();
    })
    .focusable(false)
}

pub fn tab_bar(
    state: &SelectionState,
    items: &[SelectionItem],
    activation: TabActivation,
) -> impl View<SelectionState, (), GenetCtx, Element = GenetElement> + use<> {
    selection_bar(state, items, SelectionBarConfig::tabs(activation))
}

pub fn segmented_control(
    state: &SelectionState,
    items: &[SelectionItem],
) -> impl View<SelectionState, (), GenetCtx, Element = GenetElement> + use<> {
    selection_bar(state, items, SelectionBarConfig::segmented())
}

pub fn filter_chips(
    state: &SelectionState,
    items: &[SelectionItem],
) -> impl View<SelectionState, (), GenetCtx, Element = GenetElement> + use<> {
    selection_bar(state, items, SelectionBarConfig::filter_chips())
}

impl SelectionBarConfig {
    fn root_role(self) -> &'static str {
        match self.kind {
            SelectionBarKind::Tabs { .. } => "tablist",
            SelectionBarKind::Segmented => "radiogroup",
            SelectionBarKind::FilterChips => "toolbar",
        }
    }

    fn item_role(self) -> &'static str {
        match self.kind {
            SelectionBarKind::Tabs { .. } => "tab",
            SelectionBarKind::Segmented => "radio",
            SelectionBarKind::FilterChips => "button",
        }
    }

    fn root_class(self) -> &'static str {
        match self.kind {
            SelectionBarKind::Tabs { .. } => "selection-bar tab-bar",
            SelectionBarKind::Segmented => "selection-bar segmented-control",
            SelectionBarKind::FilterChips => "selection-bar filter-chips",
        }
    }
}

fn activate(state: &mut SelectionState, index: usize, kind: SelectionBarKind) {
    match kind {
        SelectionBarKind::Tabs { .. } | SelectionBarKind::Segmented => state.select_one(index),
        SelectionBarKind::FilterChips => state.toggle(index),
    }
}

fn automatically_selects(kind: SelectionBarKind) -> bool {
    matches!(
        kind,
        SelectionBarKind::Tabs {
            activation: TabActivation::Automatic
        } | SelectionBarKind::Segmented
    )
}

fn moves_previous(key: &Key, config: SelectionBarConfig) -> bool {
    match config.kind {
        SelectionBarKind::Segmented => {
            matches!(key, Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp))
        }
        SelectionBarKind::Tabs { .. } | SelectionBarKind::FilterChips => match config.orientation {
            Orientation::Horizontal => matches!(key, Key::Named(NamedKey::ArrowLeft)),
            Orientation::Vertical => matches!(key, Key::Named(NamedKey::ArrowUp)),
        },
    }
}

fn moves_next(key: &Key, config: SelectionBarConfig) -> bool {
    match config.kind {
        SelectionBarKind::Segmented => {
            matches!(key, Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown))
        }
        SelectionBarKind::Tabs { .. } | SelectionBarKind::FilterChips => match config.orientation {
            Orientation::Horizontal => matches!(key, Key::Named(NamedKey::ArrowRight)),
            Orientation::Vertical => matches!(key, Key::Named(NamedKey::ArrowDown)),
        },
    }
}

fn enabled_positions(items: &[SelectionItem]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| (!item.disabled).then_some(index))
        .collect()
}

fn nearest_enabled(active: usize, enabled: &[usize]) -> Option<usize> {
    enabled
        .iter()
        .copied()
        .find(|index| *index >= active)
        .or_else(|| enabled.first().copied())
}

fn next_enabled(current: Option<usize>, enabled: &[usize], forward: bool) -> usize {
    let Some(current) = current else {
        return enabled.first().copied().unwrap_or(0);
    };
    let position = enabled
        .iter()
        .position(|index| *index == current)
        .unwrap_or(0);
    if forward {
        enabled
            .get(position + 1)
            .or_else(|| enabled.first())
            .copied()
            .unwrap_or(0)
    } else {
        position
            .checked_sub(1)
            .and_then(|previous| enabled.get(previous))
            .or_else(|| enabled.last())
            .copied()
            .unwrap_or(0)
    }
}

fn item_dom_id(state: &SelectionState, item: &SelectionItem) -> String {
    let fragment: String = item
        .id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("{}-item-{fragment}", state.id)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{DomHandle, GenetAppRunner, KeyEvent};

    fn items() -> Vec<SelectionItem> {
        vec![
            SelectionItem::new("Overview").controls("panel-overview"),
            SelectionItem::new("History").disabled_because("History is still loading"),
            SelectionItem::new("Links").controls("panel-links"),
        ]
    }

    fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
        dom.attribute(node, &Namespace::from(""), &LocalName::from(name))
    }

    fn find_attr(dom: &ScriptedDom, root: NodeId, name: &str, value: &str) -> Option<NodeId> {
        if attr(dom, root, name) == Some(value) {
            return Some(root);
        }
        dom.dom_children(root)
            .find_map(|child| find_attr(dom, child, name, value))
    }

    #[test]
    fn manual_tabs_move_focus_before_selection() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &SelectionState| tab_bar(state, &items(), TabActivation::Manual),
            SelectionState::single(0),
        );
        let first = find_attr(
            &dom.borrow(),
            runner.root(),
            "aria-controls",
            "panel-overview",
        )
        .expect("first tab");
        runner.set_focus(Some(first));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(runner.state().active, 2, "disabled tab is skipped");
        assert_eq!(runner.state().selected, [0]);
        let links = find_attr(&dom.borrow(), runner.root(), "aria-controls", "panel-links")
            .expect("links tab");
        assert_eq!(runner.focus(), Some(links));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(runner.state().selected, [2]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
        assert_eq!(runner.state().active, 0);
        assert_eq!(
            runner.state().selected,
            [2],
            "manual tabs do not select on movement"
        );
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(runner.state().selected, [0]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        assert_eq!(runner.state().active, 2);
    }

    #[test]
    fn segmented_arrows_move_and_select() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &SelectionState| segmented_control(state, &items()),
            SelectionState::single(0),
        );
        let first = find_attr(&dom.borrow(), runner.root(), "aria-checked", "true")
            .expect("checked segment");
        runner.set_focus(Some(first));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        assert_eq!(runner.state().active, 2);
        assert_eq!(runner.state().selected, [2]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
        assert_eq!(runner.state().selected, [0]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        assert_eq!(runner.state().selected, [2]);
    }

    #[test]
    fn filter_chip_navigation_does_not_toggle_until_activation() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &SelectionState| filter_chips(state, &items()),
            SelectionState::multiple([0, 2]),
        );
        assert_eq!(attr(&dom.borrow(), runner.root(), "role"), Some("toolbar"));
        let first =
            find_attr(&dom.borrow(), runner.root(), "aria-pressed", "true").expect("first chip");
        runner.set_focus(Some(first));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(runner.state().active, 2);
        assert_eq!(runner.state().selected, [0, 2]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        assert_eq!(runner.state().selected, [0]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
        assert_eq!(runner.state().active, 0);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        assert_eq!(runner.state().active, 2);
        let disabled = find_attr(
            &dom.borrow(),
            runner.root(),
            "aria-description",
            "History is still loading",
        )
        .expect("disabled reason");
        assert_eq!(attr(&dom.borrow(), disabled, "aria-disabled"), Some("true"));
    }
}
