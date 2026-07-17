/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Focus transitions routed through Cambium's retained message cycle.
//!
//! [`on_focus`] records its view path against the child element. The runner
//! emits [`FocusPhase::Gained`] and [`FocusPhase::Lost`] whenever pointer,
//! keyboard, or programmatic focus changes.

use core::marker::PhantomData;

use genet_scripted_dom::NodeId;
use meristem::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};

use crate::pod::GenetElement;
use crate::{ElementView, GenetCtx, OptionalAction};

const ON_FOCUS_ID: ViewId = ViewId::new(0x464F_4355);

/// Which side of a focus transition an event represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusPhase {
    /// The element became the runner's focused node.
    Gained,
    /// The element stopped being the runner's focused node.
    Lost,
}

/// A platform-neutral focus transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FocusEvent {
    pub phase: FocusPhase,
}

impl FocusEvent {
    pub const fn new(phase: FocusPhase) -> Self {
        Self { phase }
    }
}

/// A view wrapper that registers one focus handler on its child element.
pub struct OnFocus<V, State, Action, F> {
    child: V,
    handler: F,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Attach a focus-transition handler to `child`.
pub fn on_focus<V, State, Action, OA, F>(child: V, handler: F) -> OnFocus<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, FocusEvent) -> OA + 'static,
{
    OnFocus {
        child,
        handler,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnFocus`].
pub struct OnFocusState<S> {
    child_state: S,
    node: NodeId,
    path: Vec<ViewId>,
}

impl<V, State, Action, F> ViewMarker for OnFocus<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, GenetCtx> for OnFocus<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, FocusEvent) -> OA + 'static,
{
    type ViewState = OnFocusState<V::ViewState>;
    type Element = GenetElement;

    fn build(&self, ctx: &mut GenetCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(ON_FOCUS_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            let path = ctx.view_path().to_vec();
            ctx.register_focus(node, path.clone());
            (
                element,
                OnFocusState {
                    child_state,
                    node,
                    path,
                },
            )
        })
    }

    fn rebuild(
        &self,
        prev: &Self,
        view_state: &mut Self::ViewState,
        ctx: &mut GenetCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(ON_FOCUS_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            let node = *element.node;
            if node != prev_node || ctx.view_path() != view_state.path.as_slice() {
                ctx.unregister_focus(prev_node);
                let path = ctx.view_path().to_vec();
                ctx.register_focus(node, path.clone());
                view_state.node = node;
                view_state.path = path;
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut GenetCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(ON_FOCUS_ID, |ctx| {
            ctx.unregister_focus(view_state.node);
            self.child
                .teardown(&mut view_state.child_state, ctx, element);
        });
    }

    fn message(
        &self,
        view_state: &mut Self::ViewState,
        message: &mut MessageCtx,
        element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let Some(first) = message.take_first() else {
            return MessageResult::Stale;
        };
        if first != ON_FOCUS_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<FocusEvent>() {
                Some(event) => match (self.handler)(app_state, *event).action() {
                    Some(action) => MessageResult::Action(action),
                    None => MessageResult::Nop,
                },
                None => MessageResult::Stale,
            }
        } else {
            self.child
                .message(&mut view_state.child_state, message, element, app_state)
        }
    }
}

impl<V, State, Action, OA, F> ElementView<State, Action> for OnFocus<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, FocusEvent) -> OA + 'static,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyView, DomHandle, GenetAppRunner, GenetElement, el, focusable};
    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::LayoutDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct State {
        transitions: Vec<(u8, FocusPhase)>,
    }

    type FocusView = Box<dyn AnyView<State, (), GenetCtx, GenetElement>>;

    fn view(_state: &State) -> FocusView {
        Box::new(el(
            "div",
            (
                focusable(on_focus(
                    el::<_, State, ()>("button", "first"),
                    |state: &mut State, event: FocusEvent| {
                        state.transitions.push((1, event.phase));
                    },
                )),
                focusable(on_focus(
                    el::<_, State, ()>("button", "second"),
                    |state: &mut State, event: FocusEvent| {
                        state.transitions.push((2, event.phase));
                    },
                )),
            ),
        ))
    }

    #[test]
    fn programmatic_focus_routes_lost_then_gained() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), view, State::default());
        let nodes: Vec<_> = dom.borrow().dom_children(runner.root()).collect();

        runner.set_focus(Some(nodes[0]));
        runner.set_focus(Some(nodes[1]));
        runner.set_focus(None);

        assert_eq!(
            runner.state().transitions,
            [
                (1, FocusPhase::Gained),
                (1, FocusPhase::Lost),
                (2, FocusPhase::Gained),
                (2, FocusPhase::Lost),
            ]
        );
    }
}
