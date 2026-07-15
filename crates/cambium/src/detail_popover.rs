/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Hover-peek, click-pin detail popovers.

use meristem::{AnyView, ViewSequence};

use crate::{
    GenetCtx, GenetElement, HoverPhase, OverlayDismiss, OverlayRole, OverlaySurface, clickable, el,
    on_hover, overlay_surface, request_focus,
};

/// Visibility and interaction mode of a [`detail_popover`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailPopoverMode {
    #[default]
    Hidden,
    Peek,
    Pinned,
}

/// Controlled state for a [`detail_popover`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DetailPopoverState {
    pub mode: DetailPopoverMode,
    /// One-shot edge consumed by [`request_focus`] after a dismissal.
    pub return_focus: bool,
}

/// Interaction reported by a [`detail_popover`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailPopoverEvent {
    Hover(bool),
    TogglePinned,
    Dismiss(OverlayDismiss),
}

impl DetailPopoverState {
    pub fn apply(&mut self, event: DetailPopoverEvent) {
        match event {
            DetailPopoverEvent::Hover(true) => {
                self.return_focus = false;
                if self.mode == DetailPopoverMode::Hidden {
                    self.mode = DetailPopoverMode::Peek;
                }
            }
            DetailPopoverEvent::Hover(false) => {
                if self.mode == DetailPopoverMode::Peek {
                    self.mode = DetailPopoverMode::Hidden;
                }
            }
            DetailPopoverEvent::TogglePinned => {
                if self.mode == DetailPopoverMode::Pinned {
                    self.mode = DetailPopoverMode::Hidden;
                    self.return_focus = true;
                } else {
                    self.mode = DetailPopoverMode::Pinned;
                    self.return_focus = false;
                }
            }
            DetailPopoverEvent::Dismiss(_) => {
                self.mode = DetailPopoverMode::Hidden;
                self.return_focus = true;
            }
        }
    }
}

/// Render a controlled detail popover with distinct preview and pinned content.
///
/// Hovering the trigger shows the informational `preview`. Activating its
/// button pins the interactive `content`. Escape and an outside click dismiss a
/// pinned panel and return focus to the trigger. The owner stores
/// [`DetailPopoverState`] and normally handles events with
/// [`DetailPopoverState::apply`].
pub fn detail_popover<State, Action, Trigger, Preview, Content, Change>(
    state: DetailPopoverState,
    surface: &OverlaySurface,
    trigger: Trigger,
    preview: Preview,
    content: Content,
    on_change: Change,
) -> Box<dyn AnyView<State, Action, GenetCtx, GenetElement>>
where
    State: 'static,
    Action: 'static,
    Trigger: ViewSequence<State, Action, GenetCtx, GenetElement>,
    Preview: ViewSequence<State, Action, GenetCtx, GenetElement>,
    Content: ViewSequence<State, Action, GenetCtx, GenetElement>,
    Change: Fn(&mut State, DetailPopoverEvent) + Clone + 'static,
{
    let change = on_change.clone();
    let trigger = clickable(
        el::<_, State, Action>("div", trigger)
            .attr("class", "detail-popover-trigger")
            .attr("role", "button")
            .attr(
                "aria-expanded",
                if state.mode == DetailPopoverMode::Pinned {
                    "true"
                } else {
                    "false"
                },
            )
            .attr("aria-haspopup", "dialog"),
        move |app_state: &mut State, _| change(app_state, DetailPopoverEvent::TogglePinned),
    );
    let change = on_change.clone();
    let trigger = on_hover(trigger, move |app_state: &mut State, event| {
        match event.phase {
            HoverPhase::Enter => change(app_state, DetailPopoverEvent::Hover(true)),
            HoverPhase::Leave => change(app_state, DetailPopoverEvent::Hover(false)),
            HoverPhase::Move => {}
        }
    });
    let trigger = request_focus(trigger, state.return_focus);

    let panel: Option<Box<dyn AnyView<State, Action, GenetCtx, GenetElement>>> = match state.mode {
        DetailPopoverMode::Hidden => None,
        DetailPopoverMode::Peek => {
            let preview_surface = surface
                .clone()
                .with_role(OverlayRole::Tooltip)
                .dismiss_on_outside_click(false)
                .dismiss_on_escape(false);
            Some(Box::new(overlay_surface(
                &preview_surface,
                el::<_, State, Action>("div", preview).attr("class", "detail-popover-preview"),
                |_state: &mut State, _reason| {},
            )))
        }
        DetailPopoverMode::Pinned => {
            let pinned_surface = surface
                .clone()
                .with_role(OverlayRole::Dialog)
                .dismiss_on_outside_click(true)
                .dismiss_on_escape(true);
            let change = on_change.clone();
            Some(Box::new(overlay_surface(
                &pinned_surface,
                el::<_, State, Action>("div", content).attr("class", "detail-popover-content"),
                move |app_state: &mut State, reason| {
                    change(app_state, DetailPopoverEvent::Dismiss(reason));
                },
            )))
        }
    };

    Box::new(el::<_, State, Action>("div", (trigger, panel)).attr("class", "detail-popover-root"))
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{
        DomHandle, GenetAppRunner, HoverEvent, Key, KeyEvent, NamedKey, Placement, PointerClick,
        button,
    };

    #[derive(Default)]
    struct State {
        popover: DetailPopoverState,
        used: usize,
    }

    fn surface() -> OverlaySurface {
        OverlaySurface::new(
            (90.0, 90.0, 20.0, 10.0),
            (60.0, 40.0),
            (0.0, 0.0, 160.0, 120.0),
        )
        .with_placement(Placement::Below)
        .with_label("Marker detail")
    }

    fn view(state: &State) -> Box<dyn AnyView<State, (), GenetCtx, GenetElement>> {
        detail_popover(
            state.popover,
            &surface(),
            "Marker",
            "Preview",
            button("Use detail", |state: &mut State, _| state.used += 1).attr("id", "detail-use"),
            |state: &mut State, event| state.popover.apply(event),
        )
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

    fn hover(phase: HoverPhase) -> HoverEvent {
        HoverEvent::new(phase, (1.0, 1.0), (20.0, 20.0))
    }

    #[test]
    fn peek_pin_dismiss_and_restore_focus() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), view, State::default());
        let root = runner.root();
        let trigger =
            find_attr(&dom.borrow(), root, "class", "detail-popover-trigger").expect("trigger");

        runner.dispatch_hover(trigger, hover(HoverPhase::Enter));
        assert_eq!(runner.state().popover.mode, DetailPopoverMode::Peek);
        assert!(find_attr(&dom.borrow(), root, "role", "tooltip").is_some());
        assert!(
            find_attr(
                &dom.borrow(),
                root,
                "class",
                "overlay-surface-dismiss-layer"
            )
            .is_none()
        );

        runner.dispatch_hover(trigger, hover(HoverPhase::Leave));
        assert_eq!(runner.state().popover.mode, DetailPopoverMode::Hidden);

        runner.dispatch_click(trigger, PointerClick::at((1.0, 1.0)));
        assert_eq!(runner.state().popover.mode, DetailPopoverMode::Pinned);
        let inside = find_attr(&dom.borrow(), root, "id", "detail-use").expect("inside button");
        runner.set_focus(Some(inside));
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
        assert_eq!(runner.state().popover.mode, DetailPopoverMode::Hidden);
        assert_eq!(runner.focus(), Some(trigger));

        runner.dispatch_click(trigger, PointerClick::at((1.0, 1.0)));
        let outside = find_attr(
            &dom.borrow(),
            root,
            "class",
            "overlay-surface-dismiss-layer",
        )
        .expect("outside layer");
        runner.dispatch_click(outside, PointerClick::at((1.0, 1.0)));
        assert_eq!(runner.state().popover.mode, DetailPopoverMode::Hidden);
        assert_eq!(runner.focus(), Some(trigger));
    }
}
