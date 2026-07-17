/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared disclosure behavior, plus accordion and recursive tree compositions.

use std::hash::Hash;

use meristem::{AnyView, ViewSequence};

use crate::{
    El, GenetCtx, GenetElement, Key, Keyed, NamedKey, View, el, focusable_if, on_click, on_key,
    request_focus,
};

/// Controlled state for a single disclosure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureState {
    pub id: String,
    pub label: String,
    pub expanded: bool,
    pub disabled: bool,
}

impl DisclosureState {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            expanded: false,
            disabled: false,
        }
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn toggle(&mut self) {
        if !self.disabled {
            self.expanded = !self.expanded;
        }
    }
}

impl Default for DisclosureState {
    fn default() -> Self {
        Self::new("cambium-disclosure", "Details")
    }
}

/// Render a labelled disclosure button and its controlled content.
pub fn disclosure<Content>(
    state: &DisclosureState,
    content: Content,
) -> impl View<DisclosureState, (), GenetCtx, Element = GenetElement> + use<Content>
where
    Content: ViewSequence<DisclosureState, (), GenetCtx, GenetElement>,
{
    disclosure_with(state, state.label.clone(), content)
}

/// Render a disclosure with application-defined trigger content.
pub fn disclosure_with<Trigger, Content>(
    state: &DisclosureState,
    trigger: Trigger,
    content: Content,
) -> impl View<DisclosureState, (), GenetCtx, Element = GenetElement> + use<Trigger, Content>
where
    Trigger: ViewSequence<DisclosureState, (), GenetCtx, GenetElement>,
    Content: ViewSequence<DisclosureState, (), GenetCtx, GenetElement>,
{
    let trigger_id = format!("{}-trigger", state.id);
    let panel_id = format!("{}-panel", state.id);
    let control = el::<_, DisclosureState, ()>("button", trigger)
        .attr("id", trigger_id.clone())
        .attr("class", "disclosure-trigger")
        .attr("type", "button");
    let control = disclosure_control(
        control,
        Some(panel_id.clone()),
        state.expanded,
        state.disabled,
        true,
        false,
        |state: &mut DisclosureState| state.toggle(),
    );
    let mut panel = el::<_, DisclosureState, ()>("div", content)
        .attr("id", panel_id)
        .attr("class", "disclosure-panel")
        .attr("aria-labelledby", trigger_id);
    if !state.expanded {
        panel = panel.attr("hidden", "true");
    }
    el::<_, DisclosureState, ()>("div", (control, panel))
        .attr("id", state.id.clone())
        .attr("class", "disclosure")
}

/// Whether an accordion permits one or many expanded panels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccordionMode {
    Single,
    #[default]
    Multiple,
}

/// Semantic and behavior settings for an [`accordion`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccordionConfig {
    pub heading_level: u8,
    pub panel_regions: bool,
}

impl Default for AccordionConfig {
    fn default() -> Self {
        Self {
            heading_level: 3,
            panel_regions: true,
        }
    }
}

impl AccordionConfig {
    pub fn with_heading_level(mut self, level: u8) -> Self {
        self.heading_level = level.clamp(1, 6);
        self
    }

    pub fn with_panel_regions(mut self, panel_regions: bool) -> Self {
        self.panel_regions = panel_regions;
        self
    }
}

/// Application-owned accordion state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccordionState<Id> {
    pub id: String,
    pub label: String,
    pub expanded: Vec<Id>,
    pub mode: AccordionMode,
    pub collapsible: bool,
}

impl<Id> AccordionState<Id> {
    pub fn new() -> Self {
        Self {
            id: "cambium-accordion".into(),
            label: "Sections".into(),
            expanded: Vec::new(),
            mode: AccordionMode::Multiple,
            collapsible: true,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn single(mut self, collapsible: bool) -> Self {
        self.mode = AccordionMode::Single;
        self.collapsible = collapsible;
        self.expanded.truncate(1);
        self
    }

    pub fn with_expanded(mut self, expanded: impl IntoIterator<Item = Id>) -> Self
    where
        Id: Eq,
    {
        self.expanded.clear();
        for id in expanded {
            if !self.expanded.contains(&id) {
                self.expanded.push(id);
            }
            if self.mode == AccordionMode::Single {
                break;
            }
        }
        self
    }
}

impl<Id> Default for AccordionState<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id> AccordionState<Id>
where
    Id: Clone + Eq,
{
    pub fn is_expanded(&self, id: &Id) -> bool {
        self.expanded.contains(id)
    }

    pub fn toggle(&mut self, id: Id) {
        if let Some(position) = self.expanded.iter().position(|open| open == &id) {
            if self.collapsible {
                self.expanded.remove(position);
            }
            return;
        }
        if self.mode == AccordionMode::Single {
            self.expanded.clear();
        }
        self.expanded.push(id);
    }
}

/// One accordion header and its default text body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccordionItem<Id> {
    pub id: Id,
    pub dom_id: String,
    pub label: String,
    pub body: String,
}

impl<Id> AccordionItem<Id> {
    pub fn new(id: Id, label: impl Into<String>, body: impl Into<String>) -> Self
    where
        Id: ToString,
    {
        let dom_id = id.to_string();
        Self {
            id,
            dom_id,
            label: label.into(),
            body: body.into(),
        }
    }

    pub fn with_dom_id(mut self, dom_id: impl Into<String>) -> Self {
        self.dom_id = dom_id.into();
        self
    }
}

/// Render an accordion with text panel bodies.
pub fn accordion<Id>(
    state: &AccordionState<Id>,
    items: &[AccordionItem<Id>],
    config: AccordionConfig,
) -> impl View<AccordionState<Id>, (), GenetCtx, Element = GenetElement> + use<Id>
where
    Id: Clone + Eq + Hash + 'static,
{
    accordion_with(state, items, config, |item| item.body.clone())
}

/// Render an accordion with application-defined panel content.
pub fn accordion_with<Id, Content, Render>(
    state: &AccordionState<Id>,
    items: &[AccordionItem<Id>],
    config: AccordionConfig,
    render: Render,
) -> impl View<AccordionState<Id>, (), GenetCtx, Element = GenetElement> + use<Id, Content, Render>
where
    Id: Clone + Eq + Hash + 'static,
    Content: ViewSequence<AccordionState<Id>, (), GenetCtx, GenetElement>,
    Render: Fn(&AccordionItem<Id>) -> Content,
{
    let heading_level = config.heading_level.clamp(1, 6).to_string();
    let children: Vec<_> = items
        .iter()
        .cloned()
        .map(|item| {
            let expanded = state.is_expanded(&item.id);
            let trigger_id = format!("{}-item-{}-trigger", state.id, sanitize(&item.dom_id));
            let panel_id = format!("{}-item-{}-panel", state.id, sanitize(&item.dom_id));
            let disabled = expanded && state.mode == AccordionMode::Single && !state.collapsible;
            let control = el::<_, AccordionState<Id>, ()>("button", item.label.clone())
                .attr("id", trigger_id.clone())
                .attr("class", "accordion-trigger")
                .attr("type", "button");
            let toggle_id = item.id.clone();
            let control = disclosure_control(
                control,
                Some(panel_id.clone()),
                expanded,
                disabled,
                true,
                false,
                move |state: &mut AccordionState<Id>| state.toggle(toggle_id.clone()),
            );
            let heading = el::<_, AccordionState<Id>, ()>("div", control)
                .attr("class", "accordion-heading")
                .attr("role", "heading")
                .attr("aria-level", heading_level.clone());
            let mut panel = el::<_, AccordionState<Id>, ()>("div", render(&item))
                .attr("id", panel_id)
                .attr("class", "accordion-panel")
                .attr("aria-labelledby", trigger_id);
            if config.panel_regions {
                panel = panel.attr("role", "region");
            }
            if !expanded {
                panel = panel.attr("hidden", "true");
            }
            let section = el::<_, AccordionState<Id>, ()>("section", (heading, panel))
                .attr("class", "accordion-item");
            (item.id, section)
        })
        .collect();

    el::<_, AccordionState<Id>, ()>("div", Keyed::new(children))
        .attr("id", state.id.clone())
        .attr("class", "accordion")
        .attr("role", "group")
        .attr("aria-label", state.label.clone())
}

/// One node in a [`tree_view`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeItem<Id> {
    pub id: Id,
    pub dom_id: String,
    pub label: String,
    pub children: Vec<TreeItem<Id>>,
}

impl<Id> TreeItem<Id> {
    pub fn new(id: Id, label: impl Into<String>) -> Self
    where
        Id: ToString,
    {
        let dom_id = id.to_string();
        Self {
            id,
            dom_id,
            label: label.into(),
            children: Vec::new(),
        }
    }

    pub fn with_dom_id(mut self, dom_id: impl Into<String>) -> Self {
        self.dom_id = dom_id.into();
        self
    }

    pub fn with_children(mut self, children: impl IntoIterator<Item = TreeItem<Id>>) -> Self {
        self.children = children.into_iter().collect();
        self
    }
}

/// Whether moving tree focus also changes selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TreeSelectionMode {
    Explicit,
    #[default]
    FollowsFocus,
}

/// Roving focus, expansion, and selection state for a recursive tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeState<Id> {
    pub id: String,
    pub label: String,
    pub expanded: Vec<Id>,
    pub selection_mode: TreeSelectionMode,
    active: Option<Id>,
    selected: Option<Id>,
    focus_active: bool,
}

impl<Id> TreeState<Id> {
    pub fn new() -> Self {
        Self {
            id: "cambium-tree".into(),
            label: "Tree".into(),
            expanded: Vec::new(),
            selection_mode: TreeSelectionMode::FollowsFocus,
            active: None,
            selected: None,
            focus_active: false,
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_expanded(mut self, expanded: impl IntoIterator<Item = Id>) -> Self
    where
        Id: Eq,
    {
        self.expanded.clear();
        for id in expanded {
            if !self.expanded.contains(&id) {
                self.expanded.push(id);
            }
        }
        self
    }

    pub fn with_selection_mode(mut self, selection_mode: TreeSelectionMode) -> Self {
        self.selection_mode = selection_mode;
        self
    }

    pub fn with_selected(mut self, selected: Id) -> Self
    where
        Id: Clone,
    {
        self.active = Some(selected.clone());
        self.selected = Some(selected);
        self
    }

    pub fn active(&self) -> Option<&Id> {
        self.active.as_ref()
    }

    pub fn selected(&self) -> Option<&Id> {
        self.selected.as_ref()
    }
}

impl<Id> Default for TreeState<Id> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Id> TreeState<Id>
where
    Id: Clone + Eq,
{
    pub fn is_expanded(&self, id: &Id) -> bool {
        self.expanded.contains(id)
    }

    pub fn toggle(&mut self, id: Id) {
        if let Some(position) = self.expanded.iter().position(|open| open == &id) {
            self.expanded.remove(position);
        } else {
            self.expanded.push(id);
        }
    }

    fn move_active(&mut self, id: Id) {
        self.active = Some(id.clone());
        if self.selection_mode == TreeSelectionMode::FollowsFocus {
            self.selected = Some(id);
        }
        self.focus_active = true;
    }

    fn activate(&mut self, id: Id) {
        self.active = Some(id.clone());
        self.selected = Some(id);
        self.focus_active = true;
    }

    fn reconcile(&mut self, items: &[TreeItem<Id>]) {
        let mut all = Vec::new();
        collect_all(items, &mut all);
        for (index, id) in all.iter().enumerate() {
            assert!(
                !all[..index].contains(id),
                "duplicate identity in TreeItem hierarchy"
            );
        }
        self.expanded
            .retain(|expanded| all.iter().any(|id| id == expanded));
        let visible = visible_nodes(items, &self.expanded);
        if self
            .active
            .as_ref()
            .is_none_or(|active| !visible.iter().any(|node| &node.id == active))
        {
            self.active = self
                .selected
                .as_ref()
                .filter(|selected| visible.iter().any(|node| &node.id == *selected))
                .cloned()
                .or_else(|| visible.first().map(|node| node.id.clone()));
            self.focus_active = false;
        }
        if self
            .selected
            .as_ref()
            .is_none_or(|selected| !all.iter().any(|id| id == selected))
        {
            self.selected = self.active.clone();
        }
    }
}

#[derive(Clone)]
struct VisibleNode<Id> {
    id: Id,
    label: String,
    parent: Option<Id>,
    has_children: bool,
    expanded: bool,
}

type TreeNodeView<Id> = Box<dyn AnyView<TreeState<Id>, (), GenetCtx, GenetElement>>;

/// Render a recursive, single-select tree with selection following focus.
pub fn tree_view<Id>(
    state: &mut TreeState<Id>,
    items: &[TreeItem<Id>],
) -> impl View<TreeState<Id>, (), GenetCtx, Element = GenetElement> + use<Id>
where
    Id: Clone + Eq + Hash + 'static,
{
    state.reconcile(items);
    let visible = visible_nodes(items, &state.expanded);
    let nodes = tree_nodes(items, state, 1);
    let root = el::<_, TreeState<Id>, ()>("ul", nodes)
        .attr("id", state.id.clone())
        .attr("class", "tree-view")
        .attr("role", "tree")
        .attr("aria-label", state.label.clone());

    on_key(root, move |state: &mut TreeState<Id>, event| {
        let Some(current) = state
            .active
            .as_ref()
            .and_then(|active| visible.iter().position(|node| &node.id == active))
        else {
            return;
        };
        let node = &visible[current];
        let destination = match &event.key {
            Key::Named(NamedKey::ArrowDown) => Some((current + 1).min(visible.len() - 1)),
            Key::Named(NamedKey::ArrowUp) => Some(current.saturating_sub(1)),
            Key::Named(NamedKey::Home) => Some(0),
            Key::Named(NamedKey::End) => Some(visible.len() - 1),
            Key::Named(NamedKey::ArrowRight) if node.has_children && !node.expanded => {
                state.toggle(node.id.clone());
                None
            }
            Key::Named(NamedKey::ArrowRight) if node.has_children => visible
                .get(current + 1)
                .filter(|child| child.parent.as_ref() == Some(&node.id))
                .map(|_| current + 1),
            Key::Named(NamedKey::ArrowLeft) if node.has_children && node.expanded => {
                state.toggle(node.id.clone());
                None
            }
            Key::Named(NamedKey::ArrowLeft) => node
                .parent
                .as_ref()
                .and_then(|parent| visible.iter().position(|candidate| &candidate.id == parent)),
            Key::Named(NamedKey::Enter) if node.has_children => {
                state.activate(node.id.clone());
                state.toggle(node.id.clone());
                None
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => {
                state.activate(node.id.clone());
                None
            }
            Key::Character(text) if !text.is_empty() => {
                let query = text.to_lowercase();
                (1..=visible.len())
                    .map(|offset| (current + offset) % visible.len())
                    .find(|index| visible[*index].label.to_lowercase().starts_with(&query))
            }
            _ => return,
        };
        if let Some(destination) = destination {
            state.move_active(visible[destination].id.clone());
        }
        event.prevent_default();
    })
    .focusable(false)
}

fn tree_nodes<Id>(
    items: &[TreeItem<Id>],
    state: &TreeState<Id>,
    level: usize,
) -> Vec<TreeNodeView<Id>>
where
    Id: Clone + Eq + Hash + 'static,
{
    let set_size = items.len();
    items
        .iter()
        .cloned()
        .enumerate()
        .map(|(position, item)| {
            let has_children = !item.children.is_empty();
            let expanded = has_children && state.is_expanded(&item.id);
            let active = state.active.as_ref() == Some(&item.id);
            let selected = state.selected.as_ref() == Some(&item.id);
            let item_id = format!("{}-item-{}", state.id, sanitize(&item.dom_id));
            let group_id = format!("{item_id}-group");
            let children = tree_nodes(&item.children, state, level + 1);
            let group = has_children.then(|| {
                let mut group = el::<_, TreeState<Id>, ()>("ul", children)
                    .attr("id", group_id.clone())
                    .attr("class", "tree-group")
                    .attr("role", "group");
                if !expanded {
                    group = group.attr("hidden", "true");
                }
                group
            });
            let control = el::<_, TreeState<Id>, ()>(
                "li",
                (
                    el::<_, TreeState<Id>, ()>("span", item.label.clone())
                        .attr("class", "tree-label"),
                    group,
                ),
            )
            .attr("id", item_id)
            .attr(
                "class",
                if active {
                    "tree-item active"
                } else {
                    "tree-item"
                },
            )
            .attr("role", "treeitem")
            .attr("tabindex", if active { "0" } else { "-1" })
            .attr("aria-level", level.to_string())
            .attr("aria-posinset", (position + 1).to_string())
            .attr("aria-setsize", set_size.to_string())
            .attr("aria-selected", if selected { "true" } else { "false" });
            let toggle_id = item.id.clone();
            let control = disclosure_control(
                control,
                has_children.then_some(group_id),
                expanded,
                false,
                active,
                state.focus_active && active,
                move |state: &mut TreeState<Id>| {
                    state.activate(toggle_id.clone());
                    if has_children {
                        state.toggle(toggle_id.clone());
                    }
                },
            );
            Box::new(control) as TreeNodeView<Id>
        })
        .collect()
}

fn disclosure_control<State, Action, Seq, Toggle>(
    mut control: El<Seq, State, Action>,
    panel_id: Option<String>,
    expanded: bool,
    disabled: bool,
    is_focusable: bool,
    focus_requested: bool,
    on_toggle: Toggle,
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action, Seq, Toggle>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, GenetCtx, GenetElement>,
    Toggle: Fn(&mut State) + 'static,
{
    control = control.attr("data-disclosure-control", "true");
    if let Some(panel_id) = panel_id {
        control = control
            .attr("aria-controls", panel_id)
            .attr("aria-expanded", if expanded { "true" } else { "false" });
    }
    if disabled {
        control = control.attr("aria-disabled", "true");
    }
    request_focus(
        focusable_if(
            on_click(control, move |state, event| {
                event.stop_propagation();
                if !disabled {
                    on_toggle(state);
                }
            }),
            is_focusable,
        ),
        focus_requested,
    )
}

fn collect_all<Id: Clone>(items: &[TreeItem<Id>], out: &mut Vec<Id>) {
    for item in items {
        out.push(item.id.clone());
        collect_all(&item.children, out);
    }
}

fn visible_nodes<Id>(items: &[TreeItem<Id>], expanded: &[Id]) -> Vec<VisibleNode<Id>>
where
    Id: Clone + Eq,
{
    fn collect<Id>(
        items: &[TreeItem<Id>],
        expanded: &[Id],
        parent: Option<&Id>,
        out: &mut Vec<VisibleNode<Id>>,
    ) where
        Id: Clone + Eq,
    {
        for item in items {
            let is_expanded = expanded.contains(&item.id);
            out.push(VisibleNode {
                id: item.id.clone(),
                label: item.label.clone(),
                parent: parent.cloned(),
                has_children: !item.children.is_empty(),
                expanded: is_expanded,
            });
            if is_expanded {
                collect(&item.children, expanded, Some(&item.id), out);
            }
        }
    }

    let mut out = Vec::new();
    collect(items, expanded, None, &mut out);
    out
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{AnyView, DomHandle, GenetAppRunner, KeyEvent, PointerClick, lens};

    fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
        dom.attribute(node, &Namespace::from(""), &LocalName::from(name))
    }

    fn find_attr(dom: &ScriptedDom, node: NodeId, name: &str, value: &str) -> Option<NodeId> {
        if attr(dom, node, name) == Some(value) {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|child| find_attr(dom, child, name, value))
    }

    #[test]
    fn disclosure_links_and_toggles_its_panel_by_pointer_and_keyboard() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &DisclosureState| disclosure(state, "Panel body"),
            DisclosureState::new("details", "Details"),
        );
        let trigger = find_attr(&dom.borrow(), runner.root(), "id", "details-trigger")
            .expect("disclosure trigger");
        assert_eq!(attr(&dom.borrow(), trigger, "aria-expanded"), Some("false"));
        let panel = find_attr(&dom.borrow(), runner.root(), "id", "details-panel")
            .expect("disclosure panel");
        assert_eq!(attr(&dom.borrow(), panel, "hidden"), Some("true"));

        runner.dispatch_click(trigger, PointerClick::at((4.0, 4.0)));
        assert!(runner.state().expanded);
        assert_eq!(attr(&dom.borrow(), trigger, "aria-expanded"), Some("true"));
        assert_eq!(attr(&dom.borrow(), panel, "hidden"), None);

        runner.set_focus(Some(trigger));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        assert!(!runner.state().expanded);
    }

    fn accordion_items() -> Vec<AccordionItem<&'static str>> {
        vec![
            AccordionItem::new("one", "One", "First panel"),
            AccordionItem::new("two", "Two", "Second panel"),
        ]
    }

    #[test]
    fn accordion_composes_heading_buttons_over_the_disclosure_control() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(
            dom.clone(),
            |state: &AccordionState<&'static str>| {
                accordion(
                    state,
                    &accordion_items(),
                    AccordionConfig::default().with_heading_level(2),
                )
            },
            AccordionState::new().single(false).with_expanded(["one"]),
        );
        let first = find_attr(
            &dom.borrow(),
            runner.root(),
            "id",
            "cambium-accordion-item-one-trigger",
        )
        .expect("first accordion trigger");
        assert_eq!(attr(&dom.borrow(), first, "aria-expanded"), Some("true"));
        assert_eq!(attr(&dom.borrow(), first, "aria-disabled"), Some("true"));
        assert_eq!(
            attr(&dom.borrow(), first, "data-disclosure-control"),
            Some("true")
        );
        let heading = dom.borrow().parent(first).expect("heading parent");
        assert_eq!(attr(&dom.borrow(), heading, "role"), Some("heading"));
        assert_eq!(attr(&dom.borrow(), heading, "aria-level"), Some("2"));

        runner.dispatch_click(first, PointerClick::at((4.0, 4.0)));
        assert!(runner.state().is_expanded(&"one"));
        let second = find_attr(
            &dom.borrow(),
            runner.root(),
            "id",
            "cambium-accordion-item-two-trigger",
        )
        .expect("second accordion trigger");
        runner.dispatch_click(second, PointerClick::at((4.0, 4.0)));
        assert_eq!(runner.state().expanded, ["two"]);
    }

    fn tree_items() -> Vec<TreeItem<&'static str>> {
        vec![
            TreeItem::new("root", "Root").with_children([
                TreeItem::new("alpha", "Alpha"),
                TreeItem::new("beta", "Beta"),
            ]),
            TreeItem::new("other", "Other"),
        ]
    }

    struct TreeApp {
        tree: TreeState<&'static str>,
    }

    type TreeAppView = Box<dyn AnyView<TreeApp, (), GenetCtx, GenetElement>>;

    fn tree_app_view(_: &TreeApp) -> TreeAppView {
        Box::new(lens(
            |tree: &mut TreeState<&'static str>| tree_view(tree, &tree_items()),
            |app: &mut TreeApp| &mut app.tree,
        ))
    }

    #[test]
    fn recursive_tree_uses_disclosure_and_standard_hierarchical_navigation() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            tree_app_view,
            TreeApp {
                tree: TreeState::new()
                    .with_id("test-tree")
                    .with_expanded(["root"])
                    .with_selected("root"),
            },
        );
        let root_item = find_attr(&dom.borrow(), runner.root(), "id", "test-tree-item-root")
            .expect("root tree item");
        assert_eq!(attr(&dom.borrow(), runner.root(), "role"), Some("tree"));
        assert_eq!(attr(&dom.borrow(), root_item, "role"), Some("treeitem"));
        assert_eq!(
            attr(&dom.borrow(), root_item, "data-disclosure-control"),
            Some("true")
        );
        assert_eq!(
            attr(&dom.borrow(), root_item, "aria-expanded"),
            Some("true")
        );
        let alpha = find_attr(&dom.borrow(), runner.root(), "id", "test-tree-item-alpha")
            .expect("alpha tree item");
        runner.dispatch_click(alpha, PointerClick::at((4.0, 4.0)));
        assert!(
            runner.state().tree.is_expanded(&"root"),
            "a child click does not bubble into the parent disclosure"
        );
        assert_eq!(runner.state().tree.selected(), Some(&"alpha"));
        runner.update(|app| {
            app.tree.active = Some("root");
            app.tree.selected = Some("root");
            app.tree.focus_active = true;
        });
        runner.set_focus(Some(root_item));

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(runner.state().tree.active(), Some(&"alpha"));
        assert_eq!(runner.focus(), Some(alpha));

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowDown)));
        assert_eq!(runner.state().tree.active(), Some(&"beta"));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowLeft)));
        assert_eq!(runner.state().tree.active(), Some(&"root"));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowLeft)));
        assert!(!runner.state().tree.is_expanded(&"root"));
        assert_eq!(
            attr(&dom.borrow(), root_item, "aria-expanded"),
            Some("false")
        );
        let group = find_attr(
            &dom.borrow(),
            runner.root(),
            "id",
            "test-tree-item-root-group",
        )
        .expect("root child group");
        assert_eq!(attr(&dom.borrow(), group, "hidden"), Some("true"));
    }

    #[test]
    fn explicit_tree_selection_stays_distinct_from_roving_focus() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            tree_app_view,
            TreeApp {
                tree: TreeState::new()
                    .with_id("test-tree")
                    .with_expanded(["root"])
                    .with_selected("root")
                    .with_selection_mode(TreeSelectionMode::Explicit),
            },
        );
        let root_item = find_attr(&dom.borrow(), runner.root(), "id", "test-tree-item-root")
            .expect("root tree item");
        runner.set_focus(Some(root_item));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::ArrowRight)));
        assert_eq!(runner.state().tree.active(), Some(&"alpha"));
        assert_eq!(runner.state().tree.selected(), Some(&"root"));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        assert_eq!(runner.state().tree.selected(), Some(&"alpha"));
    }
}
