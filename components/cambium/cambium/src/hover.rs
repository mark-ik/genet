/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pointer-hover events routed through Cambium's retained message cycle.
//!
//! [`on_hover`] records its view path against the child element. A host calls
//! [`GenetAppRunner::dispatch_hover`](crate::GenetAppRunner::dispatch_hover)
//! when its hit target enters, leaves, or moves within that element. The host
//! owns transition detection; Cambium owns ancestor resolution, message
//! routing, action bubbling, cancellation, and rebuilding.

use core::marker::PhantomData;

use genet_scripted_dom::NodeId;
use meristem::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};

use crate::pod::GenetElement;
use crate::{ElementView, GenetCtx, OptionalAction, Propagation};

// Distinctive marker id ("HOVE") so a message on the wrong path is stale.
const ON_HOVER_ID: ViewId = ViewId::new(0x484F_5645);

/// Which transition a [`HoverEvent`] represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoverPhase {
    /// The pointer entered the resolved hover target.
    Enter,
    /// The pointer left the resolved hover target.
    Leave,
    /// The pointer moved while remaining inside the resolved hover target.
    Move,
}

/// A platform-neutral hover event.
///
/// `local` and `size` use the handling element's coordinate space. They let a
/// component react to position without depending on a windowing crate. Hosts
/// that only need transition state may pass zeroes.
#[derive(Clone, Debug)]
pub struct HoverEvent {
    pub phase: HoverPhase,
    pub local: (f32, f32),
    pub size: (f32, f32),
    /// Shared cancellation state for host defaults and propagation policy.
    pub prop: Propagation,
}

impl HoverEvent {
    /// Build a hover event with fresh propagation state.
    pub fn new(phase: HoverPhase, local: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            phase,
            local,
            size,
            prop: Propagation::new(),
        }
    }

    /// Cancel the host's default for this hover pass.
    pub fn prevent_default(&self) {
        self.prop.prevent_default();
    }
}

/// A view wrapper that registers one hover handler on its child element.
pub struct OnHover<V, State, Action, F> {
    child: V,
    handler: F,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Attach a hover handler to `child`.
///
/// The handler receives Enter, Leave, and Move events routed by
/// [`GenetAppRunner::dispatch_hover`](crate::GenetAppRunner::dispatch_hover).
/// It may mutate state and return an [`OptionalAction`].
pub fn on_hover<V, State, Action, OA, F>(child: V, handler: F) -> OnHover<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, HoverEvent) -> OA + 'static,
{
    OnHover {
        child,
        handler,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnHover`].
pub struct OnHoverState<S> {
    child_state: S,
    node: NodeId,
    path: Vec<ViewId>,
}

impl<V, State, Action, F> ViewMarker for OnHover<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, GenetCtx> for OnHover<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, HoverEvent) -> OA + 'static,
{
    type ViewState = OnHoverState<V::ViewState>;
    type Element = GenetElement;

    fn build(&self, ctx: &mut GenetCtx, app_state: &mut State) -> (Self::Element, Self::ViewState) {
        ctx.with_id(ON_HOVER_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            let path = ctx.view_path().to_vec();
            ctx.register_hover(node, path.clone());
            (
                element,
                OnHoverState {
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
        ctx.with_id(ON_HOVER_ID, |ctx| {
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
                ctx.unregister_hover(prev_node);
                let path = ctx.view_path().to_vec();
                ctx.register_hover(node, path.clone());
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
        ctx.with_id(ON_HOVER_ID, |ctx| {
            ctx.unregister_hover(view_state.node);
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
        if first != ON_HOVER_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<HoverEvent>() {
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

impl<V, State, Action, OA, F> ElementView<State, Action> for OnHover<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, HoverEvent) -> OA + 'static,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, el};
    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::LayoutDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct State {
        phases: Vec<HoverPhase>,
    }

    type HoverView = Box<dyn AnyView<State, (), GenetCtx, GenetElement>>;

    fn view(_state: &State) -> HoverView {
        Box::new(on_hover(
            el::<_, State, ()>("div", el::<_, State, ()>("span", "child")),
            |state: &mut State, event: HoverEvent| {
                if event.phase == HoverPhase::Move {
                    event.prevent_default();
                }
                state.phases.push(event.phase);
            },
        ))
    }

    fn event(phase: HoverPhase) -> HoverEvent {
        HoverEvent::new(phase, (8.0, 6.0), (40.0, 20.0))
    }

    #[test]
    fn descendant_hits_route_enter_move_leave_to_the_hover_view() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), view, State::default());
        let root = runner.root();
        let child = {
            let dom = dom.borrow();
            dom.dom_children(root)
                .find(|&node| {
                    dom.element_name(node)
                        .is_some_and(|name| name.local.as_ref() == "span")
                })
                .expect("hover child")
        };

        assert_eq!(runner.hover_target(child), Some(root));
        runner.dispatch_hover(child, event(HoverPhase::Enter));
        runner.dispatch_hover(child, event(HoverPhase::Move));
        assert!(runner.default_prevented());
        runner.dispatch_hover(child, event(HoverPhase::Leave));
        assert!(!runner.default_prevented());
        assert_eq!(
            runner.state().phases,
            [HoverPhase::Enter, HoverPhase::Move, HoverPhase::Leave]
        );
    }
}
