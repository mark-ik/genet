/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! One command model rendered as a palette, picker, or context menu.

use meristem::AnyView;

use crate::{
    Action, GenetCtx, GenetElement, Key, KeyEvent, NamedKey, PointerClick, View, el, on_click,
    on_key,
};

/// One entry in a command surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandItem {
    pub id: String,
    pub label: String,
    pub shortcut: Option<String>,
    pub disabled: bool,
    pub disabled_reason: Option<String>,
    pub children: Vec<CommandItem>,
}

impl CommandItem {
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        Self {
            id: label.clone(),
            label,
            shortcut: None,
            disabled: false,
            disabled_reason: None,
            children: Vec::new(),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
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

    pub fn with_children(mut self, children: impl IntoIterator<Item = CommandItem>) -> Self {
        self.children = children.into_iter().collect();
        self
    }
}

/// Shared interaction state for every command-surface configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandState {
    pub query: String,
    /// Position in the currently visible root list.
    pub selected: usize,
    /// Original root-item index whose submenu is open.
    pub submenu: Option<usize>,
    pub submenu_selected: usize,
    pub label: String,
    pub id: String,
}

impl Default for CommandState {
    fn default() -> Self {
        Self {
            query: String::new(),
            selected: 0,
            submenu: None,
            submenu_selected: 0,
            label: "Actions".into(),
            id: "cambium-action-list".into(),
        }
    }
}

impl CommandState {
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

/// Named semantic configuration for the shared command engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommandSurfaceKind {
    Palette,
    Picker,
    ContextMenu { x: f32, y: f32 },
}

/// Action emitted by a command surface. A one-element path addresses a root
/// item; two elements address a child in a depth-one submenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandEvent {
    Activate(Vec<usize>),
    Dismiss,
}

impl Action for CommandEvent {}

type CommandView = Box<dyn AnyView<CommandState, CommandEvent, GenetCtx, GenetElement>>;

/// Render `items` through the shared command interaction engine.
pub fn command_surface(
    state: &CommandState,
    items: &[CommandItem],
    kind: CommandSurfaceKind,
) -> impl View<CommandState, CommandEvent, GenetCtx, Element = GenetElement> + use<> {
    let query = state.query.to_lowercase();
    let root: Vec<(usize, CommandItem)> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            kind != CommandSurfaceKind::Palette || item.label.to_lowercase().contains(&query)
        })
        .map(|(index, item)| (index, item.clone()))
        .collect();
    let root_positions = navigable_positions(&root, kind);
    let selected = nearest_enabled(state.selected, &root_positions);
    let open_position = state
        .submenu
        .and_then(|parent| root.iter().position(|(index, _)| *index == parent));
    let open_submenu = state.submenu.zip(open_position);

    let mut rows: Vec<CommandView> = Vec::new();
    for (position, (original_index, item)) in root.iter().enumerate() {
        let parent = *original_index;
        let is_selected = open_submenu.is_none() && Some(position) == selected;
        let is_open = open_submenu.is_some_and(|(index, _)| index == parent);
        rows.push(command_row(
            state,
            item,
            vec![parent],
            position,
            is_selected,
            is_open,
            kind,
        ));

        if is_open {
            let child_items: Vec<(usize, CommandItem)> =
                item.children.iter().cloned().enumerate().collect();
            let child_positions = navigable_positions(&child_items, kind);
            let child_selected = nearest_enabled(state.submenu_selected, &child_positions);
            let child_rows: Vec<CommandView> = child_items
                .iter()
                .enumerate()
                .map(|(child_position, (child_index, child))| {
                    command_row(
                        state,
                        child,
                        vec![parent, *child_index],
                        child_position,
                        Some(child_position) == child_selected,
                        false,
                        kind,
                    )
                })
                .collect();
            rows.push(Box::new(
                el::<_, CommandState, CommandEvent>("div", child_rows)
                    .attr("class", "command-submenu")
                    .attr("role", "menu")
                    .attr("aria-label", item.label.clone())
                    .attr(
                        "style",
                        format!(
                            "position: absolute; left: 100%; top: {}px;",
                            position as f32 * 32.0
                        ),
                    ),
            ));
        }
    }

    let active_id = active_id(state, &root, selected, open_submenu, kind);
    let list_id = if kind == CommandSurfaceKind::Palette {
        format!("{}-options", state.id)
    } else {
        format!("{}-items", state.id)
    };
    let query_class = if kind == CommandSurfaceKind::Palette {
        "command-query action-list-query"
    } else {
        "command-query"
    };
    let query_text = (!state.query.is_empty()).then(|| {
        el::<_, CommandState, CommandEvent>("div", format!("{}: {}", state.label, state.query))
            .attr("class", query_class)
            .attr("aria-hidden", "true")
    });
    let list_role = if kind == CommandSurfaceKind::Palette {
        "listbox"
    } else {
        "presentation"
    };
    let list_class = if kind == CommandSurfaceKind::Palette {
        "command-items action-list-options"
    } else {
        "command-items"
    };
    let list = el::<_, CommandState, CommandEvent>("div", rows)
        .attr("id", list_id.clone())
        .attr("class", list_class)
        .attr("role", list_role);
    let mut root_view = el::<_, CommandState, CommandEvent>("div", (query_text, list))
        .attr("id", state.id.clone())
        .attr("class", kind.root_class())
        .attr("role", kind.root_role())
        .attr("aria-label", state.label.clone())
        .attr("aria-activedescendant", active_id)
        .attr("tabindex", "0");
    if kind == CommandSurfaceKind::Palette {
        root_view = root_view
            .attr("aria-autocomplete", "list")
            .attr("aria-expanded", "true")
            .attr("aria-controls", list_id);
    }
    if let CommandSurfaceKind::ContextMenu { x, y } = kind {
        root_view = root_view.attr(
            "style",
            format!("position: absolute; left: {x}px; top: {y}px;"),
        );
    }

    let root_for_key = root.clone();
    on_key(root_view, move |state: &mut CommandState, event| {
        let output = handle_key(state, &root_for_key, kind, &event);
        if output.prevent_default {
            event.prevent_default();
        }
        output.event
    })
}

pub fn command_palette(
    state: &CommandState,
    items: &[CommandItem],
) -> impl View<CommandState, CommandEvent, GenetCtx, Element = GenetElement> + use<> {
    command_surface(state, items, CommandSurfaceKind::Palette)
}

pub fn command_picker(
    state: &CommandState,
    items: &[CommandItem],
) -> impl View<CommandState, CommandEvent, GenetCtx, Element = GenetElement> + use<> {
    command_surface(state, items, CommandSurfaceKind::Picker)
}

pub fn command_menu(
    state: &CommandState,
    items: &[CommandItem],
    x: f32,
    y: f32,
) -> impl View<CommandState, CommandEvent, GenetCtx, Element = GenetElement> + use<> {
    command_surface(state, items, CommandSurfaceKind::ContextMenu { x, y })
}

impl CommandSurfaceKind {
    fn root_role(self) -> &'static str {
        match self {
            Self::Palette => "combobox",
            Self::Picker => "listbox",
            Self::ContextMenu { .. } => "menu",
        }
    }

    fn root_class(self) -> &'static str {
        match self {
            Self::Palette => "command-surface command-palette action-list",
            Self::Picker => "command-surface command-picker",
            Self::ContextMenu { .. } => "command-surface command-menu",
        }
    }

    fn item_role(self) -> &'static str {
        match self {
            Self::Palette | Self::Picker => "option",
            Self::ContextMenu { .. } => "menuitem",
        }
    }
}

fn command_row(
    state: &CommandState,
    item: &CommandItem,
    path: Vec<usize>,
    position: usize,
    selected: bool,
    submenu_open: bool,
    kind: CommandSurfaceKind,
) -> CommandView {
    let row_id = item_dom_id(state, &path, kind);
    let shortcut = item.shortcut.as_ref().map(|shortcut| {
        el::<_, CommandState, CommandEvent>("span", shortcut.clone())
            .attr("class", "command-shortcut")
            .attr("aria-hidden", "true")
    });
    let reason = item.disabled_reason.as_ref().map(|reason| {
        el::<_, CommandState, CommandEvent>("span", reason.clone())
            .attr("class", "command-disabled-reason")
            .attr("aria-hidden", "true")
    });
    let submenu_mark = (!item.children.is_empty()).then(|| {
        el::<_, CommandState, CommandEvent>("span", "›")
            .attr("class", "command-submenu-mark")
            .attr("aria-hidden", "true")
    });
    let mut row = el::<_, CommandState, CommandEvent>(
        "div",
        (
            el::<_, CommandState, CommandEvent>("span", item.label.clone())
                .attr("class", "command-label"),
            reason,
            shortcut,
            submenu_mark,
        ),
    )
    .attr("id", row_id)
    .attr(
        "class",
        if selected && kind == CommandSurfaceKind::Palette {
            "command-item action-item selected"
        } else if kind == CommandSurfaceKind::Palette {
            "command-item action-item"
        } else if selected {
            "command-item selected"
        } else {
            "command-item"
        },
    )
    .attr("role", kind.item_role())
    .attr(
        "aria-disabled",
        if item.disabled { "true" } else { "false" },
    );
    if matches!(
        kind,
        CommandSurfaceKind::Palette | CommandSurfaceKind::Picker
    ) {
        row = row.attr("aria-selected", if selected { "true" } else { "false" });
    }
    if let Some(reason) = &item.disabled_reason {
        row = row.attr("aria-description", reason.clone());
    }
    if !item.children.is_empty() && matches!(kind, CommandSurfaceKind::ContextMenu { .. }) {
        row = row
            .attr("aria-haspopup", "menu")
            .attr("aria-expanded", if submenu_open { "true" } else { "false" });
    }

    let disabled = item.disabled;
    let has_children = !item.children.is_empty();
    Box::new(on_click(
        row,
        move |state: &mut CommandState, _: PointerClick| {
            if disabled {
                None
            } else if has_children && matches!(kind, CommandSurfaceKind::ContextMenu { .. }) {
                state.selected = position;
                state.submenu = path.first().copied();
                state.submenu_selected = 0;
                None
            } else {
                Some(CommandEvent::Activate(path.clone()))
            }
        },
    ))
}

#[derive(Default)]
struct KeyOutput {
    prevent_default: bool,
    event: Option<CommandEvent>,
}

fn handle_key(
    state: &mut CommandState,
    root: &[(usize, CommandItem)],
    kind: CommandSurfaceKind,
    key_event: &KeyEvent,
) -> KeyOutput {
    let root_positions = navigable_positions(root, kind);
    let selected = nearest_enabled(state.selected, &root_positions);
    let active_parent = state.submenu.and_then(|parent| {
        root.iter()
            .find(|(index, _)| *index == parent)
            .map(|(_, item)| item)
    });
    let child_items: Vec<(usize, CommandItem)> = active_parent
        .map(|item| item.children.iter().cloned().enumerate().collect())
        .unwrap_or_default();
    let child_positions = navigable_positions(&child_items, kind);
    let child_selected = nearest_enabled(state.submenu_selected, &child_positions);
    let in_submenu = active_parent.is_some();

    let mut output = KeyOutput {
        prevent_default: true,
        event: None,
    };
    match &key_event.key {
        Key::Named(NamedKey::ArrowDown) => {
            if in_submenu {
                state.submenu_selected = next_enabled(child_selected, &child_positions, true);
            } else {
                state.selected = next_enabled(selected, &root_positions, true);
            }
        }
        Key::Named(NamedKey::ArrowUp) => {
            if in_submenu {
                state.submenu_selected = next_enabled(child_selected, &child_positions, false);
            } else {
                state.selected = next_enabled(selected, &root_positions, false);
            }
        }
        Key::Named(NamedKey::Home) => {
            if in_submenu {
                state.submenu_selected = child_positions.first().copied().unwrap_or(0);
            } else {
                state.selected = root_positions.first().copied().unwrap_or(0);
            }
        }
        Key::Named(NamedKey::End) => {
            if in_submenu {
                state.submenu_selected = child_positions.last().copied().unwrap_or(0);
            } else {
                state.selected = root_positions.last().copied().unwrap_or(0);
            }
        }
        Key::Named(NamedKey::ArrowRight)
            if matches!(kind, CommandSurfaceKind::ContextMenu { .. }) && !in_submenu =>
        {
            if let Some(position) = selected
                && let Some((index, item)) = root.get(position)
                && !item.disabled
                && !item.children.is_empty()
            {
                state.submenu = Some(*index);
                state.submenu_selected = navigable_positions(
                    &item
                        .children
                        .iter()
                        .cloned()
                        .enumerate()
                        .collect::<Vec<_>>(),
                    kind,
                )
                .first()
                .copied()
                .unwrap_or(0);
            }
        }
        Key::Named(NamedKey::ArrowLeft)
            if matches!(kind, CommandSurfaceKind::ContextMenu { .. }) && in_submenu =>
        {
            state.submenu = None;
        }
        Key::Named(NamedKey::Enter) => {
            if in_submenu {
                if let (Some(parent), Some(position)) = (state.submenu, child_selected)
                    && let Some((child, item)) = child_items.get(position)
                    && !item.disabled
                {
                    output.event = Some(CommandEvent::Activate(vec![parent, *child]));
                }
            } else if let Some(position) = selected
                && let Some((index, item)) = root.get(position)
                && !item.disabled
            {
                if matches!(kind, CommandSurfaceKind::ContextMenu { .. })
                    && !item.children.is_empty()
                {
                    state.submenu = Some(*index);
                    state.submenu_selected = 0;
                } else {
                    output.event = Some(CommandEvent::Activate(vec![*index]));
                }
            }
        }
        Key::Named(NamedKey::Space) if kind == CommandSurfaceKind::Palette => {
            state.query.push(' ');
            state.selected = 0;
            state.submenu = None;
        }
        Key::Named(NamedKey::Space) => {
            if in_submenu {
                if let (Some(parent), Some(position)) = (state.submenu, child_selected)
                    && let Some((child, item)) = child_items.get(position)
                    && !item.disabled
                {
                    output.event = Some(CommandEvent::Activate(vec![parent, *child]));
                }
            } else if let Some(position) = selected
                && let Some((index, item)) = root.get(position)
                && !item.disabled
            {
                if matches!(kind, CommandSurfaceKind::ContextMenu { .. })
                    && !item.children.is_empty()
                {
                    state.submenu = Some(*index);
                    state.submenu_selected = 0;
                } else {
                    output.event = Some(CommandEvent::Activate(vec![*index]));
                }
            }
        }
        Key::Named(NamedKey::Escape) => {
            if in_submenu {
                state.submenu = None;
            } else {
                output.event = Some(CommandEvent::Dismiss);
            }
        }
        Key::Named(NamedKey::Backspace) if kind == CommandSurfaceKind::Palette => {
            state.query.pop();
            state.selected = 0;
            state.submenu = None;
        }
        Key::Character(text) if kind == CommandSurfaceKind::Palette => {
            if !key_event.mods.ctrl && !key_event.mods.alt && !key_event.mods.meta {
                state.query.push_str(text);
                state.selected = 0;
                state.submenu = None;
            } else {
                output.prevent_default = false;
            }
        }
        Key::Named(NamedKey::Tab) if matches!(kind, CommandSurfaceKind::ContextMenu { .. }) => {
            output.event = Some(CommandEvent::Dismiss);
            output.prevent_default = false;
        }
        _ => {
            output.prevent_default = false;
        }
    }
    output
}

fn navigable_positions(items: &[(usize, CommandItem)], kind: CommandSurfaceKind) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter_map(|(position, (_, item))| {
            (matches!(kind, CommandSurfaceKind::ContextMenu { .. }) || !item.disabled)
                .then_some(position)
        })
        .collect()
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

fn active_id(
    state: &CommandState,
    root: &[(usize, CommandItem)],
    selected: Option<usize>,
    open_submenu: Option<(usize, usize)>,
    kind: CommandSurfaceKind,
) -> String {
    if let Some((parent, position)) = open_submenu {
        let item = &root[position].1;
        let children: Vec<_> = item.children.iter().cloned().enumerate().collect();
        let positions = navigable_positions(&children, kind);
        return nearest_enabled(state.submenu_selected, &positions)
            .and_then(|child_position| children.get(child_position))
            .map(|(child, _)| item_dom_id(state, &[parent, *child], kind))
            .unwrap_or_default();
    }
    selected
        .and_then(|position| root.get(position))
        .map(|(index, _)| item_dom_id(state, &[*index], kind))
        .unwrap_or_default()
}

fn item_dom_id(state: &CommandState, path: &[usize], kind: CommandSurfaceKind) -> String {
    let path = path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("-");
    if kind == CommandSurfaceKind::Palette {
        format!("{}-option-{path}", state.id)
    } else {
        format!("{}-item-{path}", state.id)
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{DomHandle, GenetAppRunner, KeyEvent};

    fn items() -> Vec<CommandItem> {
        vec![
            CommandItem::new("Open").with_id("open"),
            CommandItem::new("Export").with_id("export").with_children([
                CommandItem::new("Plain text"),
                CommandItem::new("PDF").disabled_because("PDF support is unavailable"),
                CommandItem::new("JSON"),
            ]),
            CommandItem::new("Close").disabled_because("Keep one document open"),
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
    fn palette_filters_and_skips_disabled_items() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &CommandState| command_palette(state, &items()),
            CommandState::default(),
        );
        runner.set_focus(Some(runner.root()));
        runner.dispatch_key(KeyEvent::new(Key::Character("cl".into())));
        assert_eq!(runner.state().query, "cl");
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert!(actions.is_empty(), "the only match is disabled");
        let disabled = find_attr(&dom.borrow(), runner.root(), "aria-disabled", "true")
            .expect("disabled option");
        assert_eq!(
            attr(&dom.borrow(), disabled, "aria-description"),
            Some("Keep one document open")
        );
    }

    #[test]
    fn menu_opens_and_closes_a_submenu_with_standard_arrows() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &CommandState| command_menu(state, &items(), 12.0, 16.0),
            CommandState::default(),
        );
        runner.set_focus(Some(runner.root()));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(runner.state().submenu, Some(1));
        let parent = find_attr(&dom.borrow(), runner.root(), "aria-expanded", "true")
            .expect("expanded parent");
        assert_eq!(attr(&dom.borrow(), parent, "role"), Some("menuitem"));

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert!(
            actions.is_empty(),
            "a disabled menu item is focusable but inert"
        );
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(actions, [CommandEvent::Activate(vec![1, 2])]);

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(runner.state().submenu, None);
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
        assert_eq!(actions, [CommandEvent::Dismiss]);
    }

    #[test]
    fn picker_uses_listbox_semantics_and_home_end() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &CommandState| command_picker(state, &items()),
            CommandState::default(),
        );
        assert_eq!(attr(&dom.borrow(), runner.root(), "role"), Some("listbox"));
        runner.set_focus(Some(runner.root()));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(actions, [CommandEvent::Activate(vec![1])]);
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Home)));
        let actions = runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Enter)));
        assert_eq!(actions, [CommandEvent::Activate(vec![0])]);
    }
}
