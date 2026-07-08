/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Arrangement views (chisel catalog tier 3): a container that owns its
//! children's x/y/z while the children stay real serval nodes.
//!
//! [`arrangement`] is a `position: relative` container claiming an explicit
//! content extent; [`placed`] wraps one child absolutely at a
//! [`Placement`] (position + `z-index` — serval's `paint_stacking` orders the
//! stack natively). Because placements are plain attribute mutations, a
//! re-placement (drag, raise, virtualization window shift) is an attribute
//! diff on a retained element, not a rebuild: hit-test, focus, and a11y state
//! ride through. Fixed-height virtualization pairs this with
//! [`chisel::VirtualWindow`]: the container takes `total_height()` so the
//! scroll range stays honest while only `range()` rows exist as DOM.
//! Design: serval `docs/2026-07-08_chisel_widget_catalog.md`.

use chisel::Placement;
use xilem_core::ViewSequence;

use crate::pod::ServalElement;
use crate::{El, ServalCtx, el};

/// Wrap `child` absolutely at `placement` (the arranged-child primitive).
/// The child sizes itself; the wrapper owns position + stacking only.
pub fn placed<State, Action, Seq>(placement: Placement, child: Seq) -> El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    el("div", child)
        .attr("class", "arranged")
        .attr("style", placement.style())
}

/// The arrangement container: `position: relative` (the children's containing
/// block and stacking context) with an explicit `width`×`height` content
/// extent. Nest it inside an `overflow: scroll` box for a virtualized list —
/// the extent keeps the scrollbar honest while only the materialized rows
/// exist.
pub fn arrangement<State, Action, Seq>(
    width: f32,
    height: f32,
    children: Seq,
) -> El<Seq, State, Action>
where
    State: 'static,
    Action: 'static,
    Seq: ViewSequence<State, Action, ServalCtx, ServalElement>,
{
    el("div", children).attr("class", "arrangement").attr(
        "style",
        format!("position: relative; width: {width}px; height: {height}px;"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServalAppRunner;
    use chisel::VirtualWindow;
    use layout_dom_api::{LayoutDom, LocalName, Namespace};
    use serval_scripted_dom::ScriptedDom;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn style_of(dom: &ScriptedDom, node: serval_scripted_dom::NodeId) -> String {
        dom.attribute(node, &Namespace::from(""), &LocalName::from("style"))
            .unwrap_or_default()
            .to_string()
    }

    struct ListState {
        scroll: f32,
    }

    fn list_view(state: &ListState) -> El<Vec<El<String, ListState, ()>>, ListState, ()> {
        let vw = VirtualWindow {
            total_rows: 10_000,
            row_height: 24.0,
            viewport_height: 300.0,
            scroll: state.scroll,
            overscan: 3,
        };
        let rows: Vec<El<String, ListState, ()>> = vw
            .range()
            .map(|i| placed(vw.row_placement(i), format!("row {i}")))
            .collect();
        arrangement(300.0, vw.total_height(), rows)
    }

    /// The virtualization done condition: a 10k-row list materializes only the
    /// visible sliver as DOM, the container claims the full extent, and
    /// scrolling shifts the window without growing the DOM.
    #[test]
    fn ten_thousand_rows_materialize_only_the_window() {
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), list_view, ListState { scroll: 0.0 });
        let root = runner.root();
        {
            let d = dom.borrow();
            let count = d.dom_children(root).count();
            assert!(
                count < 30,
                "10k rows materialize as a sliver, got {count} children"
            );
            assert!(
                style_of(&d, root).contains("height: 240000px"),
                "the container claims the full extent",
            );
        }

        runner.update(|s| s.scroll = 4800.0);
        let d = dom.borrow();
        let children: Vec<_> = d.dom_children(root).collect();
        assert!(children.len() < 30, "the window stays a sliver after scroll");
        let first_style = style_of(&d, children[0]);
        assert!(
            first_style.contains("top: 4728px"),
            "the first materialized row sits at the scrolled window (197 * 24), got {first_style}",
        );
    }

    struct Cards {
        a: Placement,
        b: Placement,
    }

    fn cards_view(state: &Cards) -> El<(El<&'static str, Cards, ()>, El<&'static str, Cards, ()>), Cards, ()> {
        arrangement(
            400.0,
            300.0,
            (placed(state.a, "card a"), placed(state.b, "card b")),
        )
    }

    /// The card done condition (DOM half): dragging and raising a card is an
    /// attribute diff on a *retained* element — same node identity, new
    /// position and z — so focus/hit-test/a11y state would ride through.
    #[test]
    fn dragging_a_card_moves_and_raises_the_same_retained_node() {
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            cards_view,
            Cards {
                a: Placement::new(10.0, 10.0).with_z(1),
                b: Placement::new(60.0, 40.0).with_z(2),
            },
        );
        let root = runner.root();
        let before: Vec<_> = dom.borrow().dom_children(root).collect();
        assert_eq!(before.len(), 2);

        // Drag card a to (200, 120) and raise it above b.
        runner.update(|s| s.a = Placement::new(200.0, 120.0).with_z(3));

        let d = dom.borrow();
        let after: Vec<_> = d.dom_children(root).collect();
        assert_eq!(
            before, after,
            "re-placement retains the same child nodes (attribute diff, not rebuild)",
        );
        let a_style = style_of(&d, after[0]);
        assert!(
            a_style.contains("left: 200px")
                && a_style.contains("top: 120px")
                && a_style.contains("z-index: 3"),
            "card a moved and raised: {a_style}",
        );
        assert!(
            style_of(&d, after[1]).contains("z-index: 2"),
            "card b keeps its stacking",
        );
    }
}
