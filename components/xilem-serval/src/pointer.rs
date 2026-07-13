/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The native pointer-drag event view: [`on_pointer`]`(child, handler)`.
//!
//! The drag foundation under sliders (and, later, scrollbar-thumb dragging,
//! resize handles, drag-tab-out). It is the [`on_click`](crate::on_click)
//! pattern for a *press → move → release* cycle: the view records its routing
//! path in [`ServalCtx`]'s pointer registry keyed by its DOM node, and the
//! runner's `dispatch_pointer_*` routes a [`PointerEvent`] down that path.
//!
//! Unlike click, a drag has **capture**: the element that received the
//! `Down` keeps receiving `Move`/`Up` until release, even if the cursor leaves
//! it. The runner owns that capture state (see
//! [`ServalAppRunner`](crate::ServalAppRunner)); a single registered handler per
//! node receives all three phases (no capture/bubble split — drag routes
//! straight to the captured target).

use core::marker::PhantomData;

use serval_scripted_dom::NodeId;
use xilem_core::{MessageCtx, MessageResult, Mut, View, ViewId, ViewMarker, ViewPathTracker};

use crate::pod::ServalElement;
use crate::{ElementView, OptionalAction, Propagation, ServalCtx};

// Distinctive marker id (randomly generated) so a stray message on a wrong path
// is caught rather than silently matching. 0x504F_494E == "POIN".
const ON_POINTER_ID: ViewId = ViewId::new(0x504F_494E);

/// Which phase of a pointer drag a [`PointerEvent`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerPhase {
    /// Button pressed on the element; begins capture.
    Down,
    /// Pointer moved while captured (the button is held).
    Move,
    /// Button released; ends capture.
    Up,
}

/// A native pointer-drag event payload.
///
/// `local` is the pointer position in the handling element's local coordinate
/// space (its top-left is `(0, 0)`); `size` is that element's box size. Together
/// they let a handler normalize without knowing layout — e.g. a slider value is
/// `(local.0 / size.0).clamp(0.0, 1.0)`. The host computes both from the
/// captured element's laid-out rect (the headless view layer has no layout).
#[derive(Clone, Debug)]
pub struct PointerEvent {
    pub phase: PointerPhase,
    pub local: (f32, f32),
    pub size: (f32, f32),
    /// Clone-through cancellation state — the native twin of a JS event's
    /// `preventDefault` / `stopPropagation` (every clone shares one cell, so a
    /// handler's call is seen by the runner). A drag handler calls
    /// `e.prop.prevent_default()` to suppress the host's default for this pointer
    /// pass; the runner records it into [`default_prevented`] after routing, per
    /// event — never the stale click/key value. See [`Propagation`].
    ///
    /// [`default_prevented`]: crate::ServalAppRunner::default_prevented
    pub prop: Propagation,
}

impl PointerEvent {
    /// A pointer event with a fresh [`Propagation`] cell. The host builds one per
    /// winit pointer event from the captured element's laid-out rect.
    pub fn new(phase: PointerPhase, local: (f32, f32), size: (f32, f32)) -> Self {
        Self {
            phase,
            local,
            size,
            prop: Propagation::new(),
        }
    }
}

/// Wraps a [`View`] and registers a native pointer-drag handler on its element.
/// Construct with [`on_pointer`].
pub struct OnPointer<V, State, Action, F> {
    child: V,
    handler: F,
    phantom: PhantomData<fn() -> (State, Action)>,
}

/// Attach a pointer-drag handler to `child`. `handler` runs for each
/// [`PointerEvent`] (Down/Move/Up) the runner routes to this element during a
/// drag it captured. It mutates app state and may return an
/// [`OptionalAction`]; the runner rebuilds afterward.
pub fn on_pointer<V, State, Action, OA, F>(child: V, handler: F) -> OnPointer<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerEvent) -> OA + 'static,
{
    OnPointer {
        child,
        handler,
        phantom: PhantomData,
    }
}

/// Retained state for an [`OnPointer`].
pub struct OnPointerState<S> {
    child_state: S,
    node: NodeId,
    /// The routing path this handler registered under — rebuild reconciles the
    /// `(node, path)` pair (an adoption changes the path without recreating the
    /// element; moveBefore plan S5).
    path: Vec<ViewId>,
}

impl<V, State, Action, F> ViewMarker for OnPointer<V, State, Action, F> {}

impl<V, State, Action, OA, F> View<State, Action, ServalCtx> for OnPointer<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerEvent) -> OA + 'static,
{
    type ViewState = OnPointerState<V::ViewState>;
    type Element = ServalElement;

    fn build(
        &self,
        ctx: &mut ServalCtx,
        app_state: &mut State,
    ) -> (Self::Element, Self::ViewState) {
        ctx.with_id(ON_POINTER_ID, |ctx| {
            let (element, child_state) = self.child.build(ctx, app_state);
            let node = element.node;
            let path = ctx.view_path().to_vec();
            ctx.register_pointer(node, path.clone());
            (
                element,
                OnPointerState {
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
        ctx: &mut ServalCtx,
        mut element: Mut<'_, Self::Element>,
        app_state: &mut State,
    ) {
        ctx.with_id(ON_POINTER_ID, |ctx| {
            let prev_node = view_state.node;
            self.child.rebuild(
                &prev.child,
                &mut view_state.child_state,
                ctx,
                element.reborrow_mut(),
                app_state,
            );
            // Reconcile the `(node, path)` pair — the path changes when this
            // subtree was adopted into a different position. (moveBefore S5.)
            let node = *element.node;
            if node != prev_node || ctx.view_path() != view_state.path.as_slice() {
                ctx.unregister_pointer(prev_node);
                let path = ctx.view_path().to_vec();
                ctx.register_pointer(node, path.clone());
                view_state.node = node;
                view_state.path = path;
            }
        });
    }

    fn teardown(
        &self,
        view_state: &mut Self::ViewState,
        ctx: &mut ServalCtx,
        element: Mut<'_, Self::Element>,
    ) {
        ctx.with_id(ON_POINTER_ID, |ctx| {
            ctx.unregister_pointer(view_state.node);
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
        if first != ON_POINTER_ID {
            return MessageResult::Stale;
        }
        if message.remaining_path().is_empty() {
            match message.take_message::<PointerEvent>() {
                Some(event) => match (self.handler)(app_state, *event).action() {
                    Some(a) => MessageResult::Action(a),
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

// Passes the child's element through, so a pointer-wrapped element is itself an
// `ElementView` and composes with on_click / on_key.
impl<V, State, Action, OA, F> ElementView<State, Action> for OnPointer<V, State, Action, F>
where
    State: 'static,
    Action: 'static,
    V: ElementView<State, Action>,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerEvent) -> OA + 'static,
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags::custom_leaf;
    use crate::{AnyView, DomHandle, ServalAppRunner, ServalCtx, ServalElement};
    use chisel::{Knob, Size};
    use layout_dom_api::LayoutDom;
    use serval_scripted_dom::ScriptedDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    const KNOB_KEY: u64 = 3;
    const BOX: (f32, f32) = (48.0, 48.0);

    struct KnobState {
        knob: Knob,
    }

    type KnobView = Box<dyn AnyView<KnobState, (), ServalCtx, ServalElement>>;

    /// The catalog's composition rule 2 made concrete: `on_pointer` wraps the
    /// leaf, the drag delta maps to a value **here in the view layer**, and the
    /// leaf only paints (`Knob` implements no `Leaf::event`).
    fn knob_view(_s: &KnobState) -> KnobView {
        Box::new(on_pointer(
            custom_leaf::<KnobState, ()>(KNOB_KEY, 48, 48),
            |s: &mut KnobState, e: PointerEvent| {
                // Normalize without knowing layout: `local` is element-local and
                // `size` is the element's box, both supplied by the runner.
                if !matches!(e.phase, PointerPhase::Up) {
                    s.knob.set_value((e.local.0 / e.size.0).clamp(0.0, 1.0));
                }
            },
        ))
    }

    fn drag(phase: PointerPhase, x: f32) -> PointerEvent {
        PointerEvent::new(phase, (x, 24.0), BOX)
    }

    #[test]
    fn dragging_a_pointer_wrapped_leaf_drives_the_knob_from_the_view_layer() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            knob_view,
            KnobState {
                knob: Knob::new(Size { width: 48.0, height: 48.0 }),
            },
        );
        let root = runner.root();

        assert_eq!(
            dom.borrow().element_name(root).map(|q| q.local.to_string()).as_deref(),
            Some("custom-leaf"),
            "on_pointer wraps the leaf element itself; nothing wraps it in a div",
        );
        assert_eq!(runner.state().knob.value(), 0.0, "knob starts at zero");

        // Down at the horizontal midpoint of a 48px box -> 0.5.
        runner.dispatch_pointer_down(root, drag(PointerPhase::Down, 24.0));
        assert_eq!(runner.state().knob.value(), 0.5, "press sets the value");
        assert_eq!(
            runner.pointer_capture(),
            Some(root),
            "the leaf captures the drag, so moves route to it even outside its box",
        );

        // Move to three-quarters across -> 0.75.
        runner.dispatch_pointer_move(drag(PointerPhase::Move, 36.0));
        assert_eq!(runner.state().knob.value(), 0.75, "the drag tracks the pointer");

        // A move past the edge clamps rather than overshooting.
        runner.dispatch_pointer_move(drag(PointerPhase::Move, 96.0));
        assert_eq!(runner.state().knob.value(), 1.0, "past the edge clamps to full");

        // Up releases the capture and this handler ignores it, so the value holds.
        runner.dispatch_pointer_up(drag(PointerPhase::Up, 0.0));
        assert_eq!(runner.state().knob.value(), 1.0, "release does not reset the value");
        assert_eq!(runner.pointer_capture(), None, "up releases the capture");
    }
}
