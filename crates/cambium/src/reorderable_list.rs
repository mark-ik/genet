/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Keyed pointer and keyboard reordering with application-owned persistence.

use std::hash::Hash;

use meristem::ViewSequence;

use crate::{
    Action, GenetCtx, GenetElement, Key, Keyed, NamedKey, PointerEvent, PointerPhase, View, el,
    on_key, on_pointer, request_focus,
};

/// One application-owned item presented by [`reorderable_list`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderItem<Id> {
    pub id: Id,
    pub label: String,
    /// Stable DOM fragment. It defaults to the identity's display form.
    pub dom_id: String,
}

impl<Id> ReorderItem<Id> {
    pub fn new(id: Id, label: impl Into<String>) -> Self
    where
        Id: ToString,
    {
        let dom_id = id.to_string();
        Self {
            id,
            label: label.into(),
            dom_id,
        }
    }

    pub fn with_dom_id(mut self, dom_id: impl Into<String>) -> Self {
        self.dom_id = dom_id.into();
        self
    }
}

/// The sole durable output of a reorder interaction.
///
/// `to` is the item's final zero-based index after the application removes it
/// from `from`. Cambium does not mutate or persist the application collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReorderMove<Id> {
    pub id: Id,
    pub from: usize,
    pub to: usize,
}

impl<Id> Action for ReorderMove<Id> {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragInput {
    Pointer,
    Keyboard,
}

#[derive(Clone, Debug, PartialEq)]
struct DragState<Id> {
    id: Id,
    label: String,
    origin: usize,
    destination: usize,
    grab_fraction: f32,
    input: DragInput,
    original_order: Vec<Id>,
}

/// Transient interaction state for a [`reorderable_list`].
#[derive(Clone, Debug, PartialEq)]
pub struct ReorderState<Id> {
    pub id: String,
    pub label: String,
    active: Option<Id>,
    drag: Option<DragState<Id>>,
    focus_active: bool,
    announcement: String,
}

impl<Id> Default for ReorderState<Id> {
    fn default() -> Self {
        Self {
            id: "cambium-reorder".into(),
            label: "Reorderable list".into(),
            active: None,
            drag: None,
            focus_active: false,
            announcement: String::new(),
        }
    }
}

impl<Id> ReorderState<Id> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn with_active(mut self, active: Id) -> Self {
        self.active = Some(active);
        self
    }

    pub fn active(&self) -> Option<&Id> {
        self.active.as_ref()
    }

    pub fn dragged(&self) -> Option<&Id> {
        self.drag.as_ref().map(|drag| &drag.id)
    }

    pub fn destination(&self) -> Option<usize> {
        self.drag.as_ref().map(|drag| drag.destination)
    }

    pub fn announcement(&self) -> &str {
        &self.announcement
    }

    pub fn cancel(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.active = Some(drag.id);
            self.focus_active = true;
            self.announcement = format!("Cancelled move for {}", drag.label);
        }
    }
}

impl<Id> ReorderState<Id>
where
    Id: Clone + Eq,
{
    fn reconcile(&mut self, items: &[ReorderItem<Id>]) {
        let order: Vec<_> = items.iter().map(|item| item.id.clone()).collect();
        if self
            .active
            .as_ref()
            .is_none_or(|active| !order.contains(active))
        {
            self.active = order.first().cloned();
            self.focus_active = false;
        }
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.original_order != order)
        {
            self.drag = None;
            self.announcement = "Move cancelled because the list changed".into();
        }
    }

    fn begin(
        &mut self,
        item: &ReorderItem<Id>,
        origin: usize,
        grab_fraction: f32,
        input: DragInput,
        order: &[Id],
    ) {
        self.active = Some(item.id.clone());
        self.focus_active = true;
        self.announcement = format!("Picked up {}, position {}", item.label, origin + 1);
        self.drag = Some(DragState {
            id: item.id.clone(),
            label: item.label.clone(),
            origin,
            destination: origin,
            grab_fraction,
            input,
            original_order: order.to_vec(),
        });
    }

    fn set_destination(&mut self, destination: usize, len: usize) {
        if let Some(drag) = self.drag.as_mut() {
            drag.destination = destination.min(len.saturating_sub(1));
            self.announcement = format!(
                "Moving {}, position {} of {}",
                drag.label,
                drag.destination + 1,
                len
            );
        }
    }

    fn finish(&mut self) -> Option<ReorderMove<Id>> {
        let drag = self.drag.take()?;
        self.active = Some(drag.id.clone());
        self.focus_active = true;
        if drag.origin == drag.destination {
            self.announcement = format!("{} stayed in position {}", drag.label, drag.origin + 1);
            return None;
        }
        self.announcement = format!(
            "Moved {} from position {} to {}",
            drag.label,
            drag.origin + 1,
            drag.destination + 1
        );
        Some(ReorderMove {
            id: drag.id,
            from: drag.origin,
            to: drag.destination,
        })
    }
}

/// Render a keyed reorder interaction.
///
/// Space or Enter picks up the active item; arrows, Home, and End move the
/// indicator; Space or Enter drops; Escape cancels. `Alt+Arrow` performs the
/// WAI-ARIA rearrangeable-list shortcut directly. Pointer drags use the same
/// final [`ReorderMove`].
pub fn reorderable_list<Id>(
    state: &mut ReorderState<Id>,
    items: &[ReorderItem<Id>],
) -> impl View<ReorderState<Id>, ReorderMove<Id>, GenetCtx, Element = GenetElement> + use<Id>
where
    Id: Clone + Eq + Hash + 'static,
{
    reorderable_list_with(state, items, |item| {
        el::<_, ReorderState<Id>, ReorderMove<Id>>("span", item.label.clone())
            .attr("class", "reorder-label")
    })
}

/// Render a keyed reorder interaction with application-defined row content.
///
/// The renderer owns the row body while Cambium retains the surrounding list
/// item, identity, focus, pointer capture, indicator, and movement contract.
pub fn reorderable_list_with<Id, Content, Render>(
    state: &mut ReorderState<Id>,
    items: &[ReorderItem<Id>],
    render: Render,
) -> impl View<ReorderState<Id>, ReorderMove<Id>, GenetCtx, Element = GenetElement>
+ use<Id, Content, Render>
where
    Id: Clone + Eq + Hash + 'static,
    Content: ViewSequence<ReorderState<Id>, ReorderMove<Id>, GenetCtx, GenetElement>,
    Render: Fn(&ReorderItem<Id>) -> Content,
{
    state.reconcile(items);
    let items = items.to_vec();
    let order: Vec<_> = items.iter().map(|item| item.id.clone()).collect();
    let len = items.len();
    let active = state.active.clone();
    let drag = state.drag.clone();
    let root_id = state.id.clone();
    let instructions_id = format!("{root_id}-instructions");

    let children: Vec<_> = items
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, item)| {
            let content = render(&item);
            let is_active = active.as_ref() == Some(&item.id);
            let item_drag = drag.as_ref().filter(|drag| drag.id == item.id);
            let drop_before = drag.as_ref().is_some_and(|drag| {
                drag.destination != drag.origin
                    && drag.destination == index
                    && drag.destination < drag.origin
            });
            let drop_after = drag.as_ref().is_some_and(|drag| {
                drag.destination != drag.origin
                    && drag.destination == index
                    && drag.destination > drag.origin
            });
            let before = drop_before.then(|| drop_indicator::<Id>("before"));
            let after = drop_after.then(|| drop_indicator::<Id>("after"));
            let class = if item_drag.is_some() {
                "reorder-item dragging"
            } else if drop_before {
                "reorder-item drop-before"
            } else if drop_after {
                "reorder-item drop-after"
            } else {
                "reorder-item"
            };
            let row = el::<_, ReorderState<Id>, ReorderMove<Id>>(
                "li",
                (
                    before,
                    content,
                    el::<_, ReorderState<Id>, ReorderMove<Id>>(
                        "span",
                        format!("{} of {len}", index + 1),
                    )
                    .attr("class", "reorder-position")
                    .attr("aria-hidden", "true"),
                    after,
                ),
            )
            .attr("id", format!("{root_id}-item-{}", sanitize(&item.dom_id)))
            .attr("class", class)
            .attr("role", "listitem")
            .attr("tabindex", if is_active { "0" } else { "-1" })
            .attr("aria-describedby", instructions_id.clone())
            .attr(
                "aria-label",
                format!("{}, position {} of {len}", item.label, index + 1),
            );

            let key_item = item.clone();
            let key_order = order.clone();
            let keyboard = on_key(row, move |state: &mut ReorderState<Id>, event| {
                let direct_destination = if event.mods.alt {
                    match event.key {
                        Key::Named(NamedKey::ArrowUp) => Some(index.saturating_sub(1)),
                        Key::Named(NamedKey::ArrowDown) => {
                            Some((index + 1).min(len.saturating_sub(1)))
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                if let Some(to) = direct_destination {
                    event.prevent_default();
                    state.active = Some(key_item.id.clone());
                    state.focus_active = true;
                    if to == index {
                        return None;
                    }
                    state.announcement = format!(
                        "Moved {} from position {} to {}",
                        key_item.label,
                        index + 1,
                        to + 1
                    );
                    return Some(ReorderMove {
                        id: key_item.id.clone(),
                        from: index,
                        to,
                    });
                }

                let owns_drag = state.drag.as_ref().is_some_and(|drag| {
                    drag.id == key_item.id && drag.input == DragInput::Keyboard
                });
                if owns_drag {
                    match event.key {
                        Key::Named(NamedKey::ArrowUp) => {
                            let to = state.destination().unwrap_or(index).saturating_sub(1);
                            state.set_destination(to, len);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            let to = (state.destination().unwrap_or(index) + 1)
                                .min(len.saturating_sub(1));
                            state.set_destination(to, len);
                        }
                        Key::Named(NamedKey::Home) => state.set_destination(0, len),
                        Key::Named(NamedKey::End) => {
                            state.set_destination(len.saturating_sub(1), len)
                        }
                        Key::Named(NamedKey::Escape) => state.cancel(),
                        Key::Named(NamedKey::Enter | NamedKey::Space) => {
                            event.prevent_default();
                            return state.finish();
                        }
                        _ => return None,
                    }
                    event.prevent_default();
                    return None;
                }

                let destination = match event.key {
                    Key::Named(NamedKey::ArrowUp) => Some(index.saturating_sub(1)),
                    Key::Named(NamedKey::ArrowDown) => Some((index + 1).min(len.saturating_sub(1))),
                    Key::Named(NamedKey::Home) => Some(0),
                    Key::Named(NamedKey::End) => Some(len.saturating_sub(1)),
                    Key::Named(NamedKey::Enter | NamedKey::Space) => {
                        state.begin(&key_item, index, 0.5, DragInput::Keyboard, &key_order);
                        event.prevent_default();
                        return None;
                    }
                    _ => return None,
                };
                if let Some(destination) = destination {
                    state.active = key_order.get(destination).cloned();
                    state.focus_active = true;
                }
                event.prevent_default();
                None
            })
            .focusable(is_active);

            let pointer_item = item.clone();
            let pointer_order = order.clone();
            let pointer = on_pointer(
                request_focus(
                    keyboard,
                    is_active && state.focus_active && active.as_ref() == Some(&item.id),
                ),
                move |state: &mut ReorderState<Id>, event: PointerEvent| {
                    let height = event.size.1.max(1.0);
                    match event.phase {
                        PointerPhase::Down => {
                            state.begin(
                                &pointer_item,
                                index,
                                event.local.1 / height,
                                DragInput::Pointer,
                                &pointer_order,
                            );
                            event.prop.prevent_default();
                            None
                        }
                        PointerPhase::Move => {
                            if let Some(drag) = state.drag.as_ref().filter(|drag| {
                                drag.id == pointer_item.id && drag.input == DragInput::Pointer
                            }) {
                                let destination = pointer_destination(
                                    drag.origin,
                                    drag.grab_fraction,
                                    event.local.1,
                                    height,
                                    len,
                                );
                                state.set_destination(destination, len);
                            }
                            event.prop.prevent_default();
                            None
                        }
                        PointerPhase::Up => {
                            if let Some(drag) = state.drag.as_ref().filter(|drag| {
                                drag.id == pointer_item.id && drag.input == DragInput::Pointer
                            }) {
                                let destination = pointer_destination(
                                    drag.origin,
                                    drag.grab_fraction,
                                    event.local.1,
                                    height,
                                    len,
                                );
                                state.set_destination(destination, len);
                                event.prop.prevent_default();
                                state.finish()
                            } else {
                                None
                            }
                        }
                    }
                },
            );
            (item.id, pointer)
        })
        .collect();

    let instructions = el::<_, ReorderState<Id>, ReorderMove<Id>>(
        "p",
        "Press Space or Enter to pick up an item. Use Arrow keys, Home, or End to move it. Press Space or Enter to drop, or Escape to cancel. Alt plus Arrow moves immediately.",
    )
    .attr("id", instructions_id)
    .attr("class", "reorder-instructions");
    let status = el::<_, ReorderState<Id>, ReorderMove<Id>>("p", state.announcement.clone())
        .attr("class", "reorder-status")
        .attr("role", "status")
        .attr("aria-live", "polite");
    el(
        "div",
        (
            instructions,
            status,
            el("ol", Keyed::new(children)).attr("class", "reorder-items"),
        ),
    )
    .attr("id", state.id.clone())
    .attr("class", "reorderable-list")
    .attr("role", "group")
    .attr("aria-label", state.label.clone())
}

fn drop_indicator<Id>(
    position: &'static str,
) -> impl View<ReorderState<Id>, ReorderMove<Id>, GenetCtx, Element = GenetElement> + use<Id>
where
    Id: 'static,
{
    el::<_, ReorderState<Id>, ReorderMove<Id>>("span", "")
        .attr("class", "reorder-drop-indicator")
        .attr("data-position", position)
        .attr("aria-hidden", "true")
}

fn pointer_destination(
    origin: usize,
    grab_fraction: f32,
    local_y: f32,
    row_height: f32,
    len: usize,
) -> usize {
    let delta = (local_y / row_height - grab_fraction).round() as isize;
    (origin as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize
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
    use crate::{AnyView, DomHandle, GenetAppRunner, KeyEvent, Modifiers, lens, map_action};

    #[derive(Default)]
    struct App {
        order: Vec<&'static str>,
        reorder: ReorderState<&'static str>,
        moves: Vec<ReorderMove<&'static str>>,
    }

    type AppView = Box<dyn AnyView<App, (), GenetCtx, GenetElement>>;

    fn items(order: &[&'static str]) -> Vec<ReorderItem<&'static str>> {
        order
            .iter()
            .map(|id| {
                let label = match *id {
                    "a" => "Alpha",
                    "b" => "Bravo",
                    _ => "Charlie",
                };
                ReorderItem::new(*id, label)
            })
            .collect()
    }

    fn app_view(state: &App) -> AppView {
        let items = items(&state.order);
        Box::new(map_action(
            lens(
                move |reorder: &mut ReorderState<&'static str>| reorderable_list(reorder, &items),
                |app: &mut App| &mut app.reorder,
            ),
            |app: &mut App, movement: ReorderMove<&'static str>| {
                let from = app
                    .order
                    .iter()
                    .position(|id| *id == movement.id)
                    .expect("move identity remains in application order");
                let item = app.order.remove(from);
                let to = movement.to.min(app.order.len());
                app.order.insert(to, item);
                app.moves.push(movement);
            },
        ))
    }

    fn runner() -> GenetAppRunner<App, fn(&App) -> AppView, AppView> {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        GenetAppRunner::new(
            dom,
            app_view,
            App {
                order: vec!["a", "b", "c"],
                reorder: ReorderState::new()
                    .with_id("test-reorder")
                    .with_label("Test order"),
                moves: Vec::new(),
            },
        )
    }

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

    fn pointer(phase: PointerPhase, y: f32) -> PointerEvent {
        PointerEvent::new(phase, (8.0, y), (180.0, 20.0))
    }

    #[test]
    fn keyboard_cancel_and_drop_preserve_focus_and_emit_one_move() {
        let mut runner = runner();
        let first = find_attr(
            &runner.dom().borrow(),
            runner.root(),
            "id",
            "test-reorder-item-a",
        )
        .expect("first row");
        runner.set_focus(Some(first));

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        assert_eq!(runner.state().reorder.destination(), Some(2));
        assert!(
            find_attr(
                &runner.dom().borrow(),
                runner.root(),
                "data-position",
                "after"
            )
            .is_some()
        );
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
        assert_eq!(runner.state().order, ["a", "b", "c"]);
        assert_eq!(runner.state().reorder.destination(), None);

        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        assert_eq!(runner.state().order, ["b", "c", "a"]);
        assert_eq!(
            runner.state().moves,
            [ReorderMove {
                id: "a",
                from: 0,
                to: 2,
            }]
        );
        assert_eq!(runner.focus(), Some(first), "the keyed row keeps focus");
    }

    #[test]
    fn pointer_and_keyboard_emit_the_same_move() {
        let mut pointer_runner = runner();
        let first = find_attr(
            &pointer_runner.dom().borrow(),
            pointer_runner.root(),
            "id",
            "test-reorder-item-a",
        )
        .expect("first row");
        pointer_runner.dispatch_pointer_down(first, pointer(PointerPhase::Down, 10.0));
        assert_eq!(pointer_runner.pointer_capture(), Some(first));
        pointer_runner.dispatch_pointer_move(pointer(PointerPhase::Move, 50.0));
        pointer_runner.dispatch_pointer_up(pointer(PointerPhase::Up, 50.0));
        assert_eq!(pointer_runner.pointer_capture(), None);

        let mut keyboard_runner = runner();
        let first = find_attr(
            &keyboard_runner.dom().borrow(),
            keyboard_runner.root(),
            "id",
            "test-reorder-item-a",
        )
        .expect("first row");
        keyboard_runner.set_focus(Some(first));
        keyboard_runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));
        keyboard_runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::End)));
        keyboard_runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Space)));

        assert_eq!(pointer_runner.state().moves, keyboard_runner.state().moves);
        assert_eq!(pointer_runner.state().order, keyboard_runner.state().order);
    }

    #[test]
    fn alt_arrow_moves_directly_without_entering_drag_mode() {
        let mut runner = runner();
        let first = find_attr(
            &runner.dom().borrow(),
            runner.root(),
            "id",
            "test-reorder-item-a",
        )
        .expect("first row");
        runner.set_focus(Some(first));
        runner.dispatch_key(KeyEvent::with_mods(
            Key::Named(NamedKey::ArrowDown),
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        ));

        assert_eq!(runner.state().order, ["b", "a", "c"]);
        assert_eq!(runner.state().reorder.dragged(), None);
        assert_eq!(
            runner.state().moves,
            [ReorderMove {
                id: "a",
                from: 0,
                to: 1,
            }]
        );
        assert_eq!(runner.focus(), Some(first));
    }

    #[test]
    fn removing_the_captured_item_clears_drag_capture_and_indicator() {
        let mut runner = runner();
        let first = find_attr(
            &runner.dom().borrow(),
            runner.root(),
            "id",
            "test-reorder-item-a",
        )
        .expect("first row");
        runner.dispatch_pointer_down(first, pointer(PointerPhase::Down, 10.0));
        runner.dispatch_pointer_move(pointer(PointerPhase::Move, 30.0));
        assert!(runner.state().reorder.dragged().is_some());
        assert!(runner.pointer_capture().is_some());

        runner.update(|app| {
            app.order.remove(0);
        });

        assert_eq!(runner.pointer_capture(), None);
        assert_eq!(runner.state().reorder.dragged(), None);
        assert!(
            find_attr(
                &runner.dom().borrow(),
                runner.root(),
                "class",
                "reorder-drop-indicator"
            )
            .is_none()
        );
    }
}
