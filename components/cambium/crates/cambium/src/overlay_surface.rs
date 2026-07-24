/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A positioned, dismissible surface over Cambium's overlay geometry.
//!
//! This is the shared behavior layer beneath popovers, dialogs, sheets, and
//! transient cards. The caller owns open state and content; this component owns
//! edge-aware placement, outside-click interception, Escape dismissal, and the
//! semantic container. Its ancestor key listener is passive, so it can observe
//! Escape from focused panel content without adding a Tab stop.

use meristem::ViewSequence;

use crate::{
    GenetCtx, GenetElement, Key, NamedKey, Placement, PointerClick, View, anchor_point_clamped, el,
    on_click, on_key, overlay_rect,
};

/// Why an [`overlay_surface`] asked its owner to close.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayDismiss {
    OutsideClick,
    Escape,
}

/// The accessible role of an overlay panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayRole {
    Tooltip,
    Dialog,
    #[default]
    Region,
}

impl OverlayRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tooltip => "tooltip",
            Self::Dialog => "dialog",
            Self::Region => "region",
        }
    }
}

/// Geometry, semantics, and dismissal policy for [`overlay_surface`].
#[derive(Clone, Debug, PartialEq)]
pub struct OverlaySurface {
    /// Trigger box `(x, y, width, height)` in the positioned root's space.
    pub trigger: (f32, f32, f32, f32),
    /// Measured panel size `(width, height)`.
    pub panel_size: (f32, f32),
    /// Available `(x0, y0, x1, y1)` bounds.
    pub bounds: (f32, f32, f32, f32),
    pub placement: Placement,
    pub role: OverlayRole,
    pub label: String,
    pub modal: bool,
    pub dismiss_on_outside_click: bool,
    pub dismiss_on_escape: bool,
}

impl OverlaySurface {
    pub fn new(
        trigger: (f32, f32, f32, f32),
        panel_size: (f32, f32),
        bounds: (f32, f32, f32, f32),
    ) -> Self {
        Self {
            trigger,
            panel_size,
            bounds,
            placement: Placement::Below,
            role: OverlayRole::Region,
            label: "Overlay".into(),
            modal: false,
            dismiss_on_outside_click: true,
            dismiss_on_escape: true,
        }
    }

    #[must_use]
    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    #[must_use]
    pub fn with_role(mut self, role: OverlayRole) -> Self {
        self.role = role;
        self
    }

    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    #[must_use]
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    #[must_use]
    pub fn dismiss_on_outside_click(mut self, dismiss: bool) -> Self {
        self.dismiss_on_outside_click = dismiss;
        self
    }

    #[must_use]
    pub fn dismiss_on_escape(mut self, dismiss: bool) -> Self {
        self.dismiss_on_escape = dismiss;
        self
    }

    /// Edge-aware top-left panel point.
    pub fn panel_point(&self) -> (f32, f32) {
        anchor_point_clamped(self.trigger, self.panel_size, self.placement, self.bounds)
    }
}

/// Render one controlled overlay surface.
///
/// The caller keeps open state and removes this view after `on_dismiss`. A
/// transparent dismissal layer is emitted only when outside-click dismissal is
/// enabled. The panel is its later sibling, so clicks within panel content do
/// not hit that layer. Escape bubbles from focused descendants to a passive key
/// listener on the composite root.
pub fn overlay_surface<State, Action, Seq, Dismiss>(
    surface: &OverlaySurface,
    content: Seq,
    on_dismiss: Dismiss,
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action, Seq, Dismiss>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, GenetCtx, GenetElement>,
    Dismiss: Fn(&mut State, OverlayDismiss) + Clone + 'static,
{
    let (x, y) = surface.panel_point();
    let (width, height) = surface.panel_size;
    let (x0, y0, x1, y1) = surface.bounds;

    let outside = surface.dismiss_on_outside_click.then(|| {
        let dismiss = on_dismiss.clone();
        on_click(
            el::<_, State, Action>("div", ())
                .attr("class", "overlay-surface-dismiss-layer")
                .attr("aria-hidden", "true")
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:{x0}px;top:{y0}px;width:{}px;height:{}px;",
                        (x1 - x0).max(0.0),
                        (y1 - y0).max(0.0),
                    ),
                ),
            move |state: &mut State, _: PointerClick| {
                dismiss(state, OverlayDismiss::OutsideClick);
            },
        )
    });

    let mut panel = overlay_rect::<_, State, Action>(x, y, width, height, content)
        .attr("class", "overlay-surface-panel")
        .attr("role", surface.role.as_str())
        .attr("aria-label", surface.label.clone());
    if surface.modal && surface.role == OverlayRole::Dialog {
        panel = panel.attr("aria-modal", "true");
    }

    let dismiss_on_escape = surface.dismiss_on_escape;
    let dismiss = on_dismiss.clone();
    on_key(
        el::<_, State, Action>("div", (outside, panel))
            .attr(
                "class",
                if surface.modal {
                    "overlay-surface-root modal"
                } else {
                    "overlay-surface-root"
                },
            )
            .attr("style", "position:absolute;left:0;top:0;width:0;height:0;"),
        move |state: &mut State, event| {
            if dismiss_on_escape && matches!(event.key, Key::Named(NamedKey::Escape)) {
                event.prevent_default();
                dismiss(state, OverlayDismiss::Escape);
            }
        },
    )
    .focusable(false)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{DomHandle, GenetAppRunner, KeyEvent, button};

    #[derive(Default)]
    struct State {
        dismissed: Vec<OverlayDismiss>,
        inside_clicks: usize,
    }

    fn model() -> OverlaySurface {
        OverlaySurface::new(
            (90.0, 90.0, 20.0, 10.0),
            (60.0, 40.0),
            (0.0, 0.0, 160.0, 120.0),
        )
        .with_placement(Placement::Below)
        .with_role(OverlayRole::Dialog)
        .with_label("Marker detail")
    }

    fn view(_state: &State) -> impl View<State, (), GenetCtx, Element = GenetElement> + use<> {
        overlay_surface(
            &model(),
            button("Inside", |state: &mut State, _| state.inside_clicks += 1)
                .attr("id", "inside-overlay"),
            |state: &mut State, reason| state.dismissed.push(reason),
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

    #[test]
    fn placement_flips_inside_bounds() {
        assert_eq!(model().panel_point(), (90.0, 50.0));
    }

    #[test]
    fn outside_click_and_escape_dismiss_without_an_extra_tab_stop() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), view, State::default());
        let root = runner.root();
        let outside = find_attr(
            &dom.borrow(),
            root,
            "class",
            "overlay-surface-dismiss-layer",
        )
        .expect("dismiss layer");
        let panel = find_attr(&dom.borrow(), root, "role", "dialog").expect("dialog panel");
        assert_eq!(
            attr(&dom.borrow(), panel, "aria-label"),
            Some("Marker detail")
        );

        let inside = find_attr(&dom.borrow(), root, "id", "inside-overlay").expect("inside");
        runner.dispatch_click(inside, PointerClick::at((1.0, 1.0)));
        assert_eq!(runner.state().inside_clicks, 1);
        assert!(
            runner.state().dismissed.is_empty(),
            "panel click is not outside"
        );

        runner.dispatch_click(outside, PointerClick::at((1.0, 1.0)));
        assert_eq!(runner.state().dismissed, [OverlayDismiss::OutsideClick]);

        runner.focus_traverse(true);
        let focus = runner
            .focus()
            .expect("inside button receives the first Tab stop");
        assert_ne!(focus, root, "passive overlay root is not a Tab stop");
        runner.dispatch_key(KeyEvent::new(Key::Named(NamedKey::Escape)));
        assert_eq!(
            runner.state().dismissed,
            [OverlayDismiss::OutsideClick, OverlayDismiss::Escape]
        );
    }
}
