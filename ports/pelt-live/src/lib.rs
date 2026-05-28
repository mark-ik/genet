/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `pelt-live`: a headless Xilem-on-serval host probe.
//!
//! Stage 1b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`. It pairs
//! [`xilem_serval::ServalAppRunner`] (state → view tree → DOM diff) with a
//! headless render driver ([`render::scene_from_scripted_dom`]: `ScriptedDom` →
//! `netrender::Scene`) and proves the whole spine end to end, offline:
//!
//! ```text
//! app state --(ServalAppRunner)--> ScriptedDom diff --(serval-layout)--> layout
//!                                                    --(paint emit)----> PaintList
//!                                                    --(paint)---------> netrender::Scene
//! ```
//!
//! No window and no input yet (input is Stage 2). The render side is GPU-free —
//! a `pelt` host binary would feed the produced `Scene` to netrender/wgpu, but
//! the probe asserts on the `Scene`/layout directly. This is `xilem-serval`'s
//! consumer; `xilem-serval` itself stays thin (no serval-layout/netrender dep),
//! and this crate carries the engine stack.

pub mod render;

pub use render::{fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};
    use xilem_serval::{El, ServalAppRunner, el};

    use crate::render::{fragments_from_scripted_dom, hit_test_node, scene_from_scripted_dom};

    /// The app state under test.
    struct Counter {
        count: u32,
    }

    /// App logic: `<div id="counter">{count}</div>`. The `id` lets author CSS
    /// target the element; the text child carries the count.
    fn counter_view(s: &Counter) -> El<String, Counter, ()> {
        el::<_, Counter, ()>("div", s.count.to_string()).attr("id", "counter")
    }

    /// The author stylesheet: make the `<div>` a block box so layout reaches it.
    const SHEET: &[&str] = &["div { display: block; }"];

    /// The text data of the (single) text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// End-to-end: tick a counter 0 → 1 → 2 → 3 through the runner, and after
    /// each tick assert (a) the DOM text under the div equals the count and
    /// (b) the serval render path (cascade → layout → paint) reflects the
    /// state — the div is laid out, and the paint list translates to a Scene.
    #[test]
    fn counter_renders_through_serval_end_to_end() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });

        // Tick 0 (initial build) through 3.
        for expected in 0..=3u32 {
            if expected > 0 {
                runner.update(|s| s.count += 1);
            }
            assert_eq!(runner.state().count, expected);

            let dom = runner.dom();
            let dom_ref = dom.borrow();
            let root = runner.root();

            // (a) State change reached the DOM: the text under the div is the
            // current count.
            assert_eq!(
                text_child(&dom_ref, root).as_deref(),
                Some(expected.to_string().as_str()),
                "tick {expected}: DOM text under div must equal the count"
            );

            // (b1) Layout reflects the DOM: the <div> was reached by layout and
            // got a fragment. This is the plan's fallback assertion level and
            // holds regardless of glyph emission.
            let fragments = fragments_from_scripted_dom(&dom_ref, SHEET, 800, 600);
            assert!(
                fragments.rect_of(root).is_some(),
                "tick {expected}: <div> must be laid out (got a fragment)"
            );

            // (b2) Paint emission + Scene translation builds without panic over
            // the live ScriptedDom (paint-over-ScriptedDom works — no fallback
            // needed). The Scene carries the requested viewport.
            let scene = scene_from_scripted_dom(&dom_ref, SHEET, 800, 600);
            assert_eq!(scene.viewport_width, 800);
            assert_eq!(scene.viewport_height, 600);
            // The counter's text paints: emission produces at least one draw op
            // (a glyph run for the digit). Non-empty ops confirm the paint half
            // ran over the live ScriptedDom, not just layout.
            assert!(
                !scene.ops.is_empty(),
                "tick {expected}: paint emission should produce draw ops"
            );
        }
    }

    /// Stage 2a: serval's existing hit-test query, wired over the live
    /// `ScriptedDom`. A point inside the laid-out `<div>` recovers a live node in
    /// its subtree; a point far outside recovers nothing. This is the
    /// `point → NodeId` half of input dispatch (the dispatch walk + handlers are
    /// Stage 2b).
    #[test]
    fn hit_test_recovers_live_node_in_subtree() {
        let dom: Rc<RefCell<ScriptedDom>> = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::new(dom.clone(), counter_view, Counter { count: 0 });
        let root = runner.root();

        let dom_ref = dom.borrow();
        // (5, 5) is inside the top-left block <div> (full-width, one text line
        // tall), so the hit-test lands on the div or its text child.
        let hit = hit_test_node(&dom_ref, SHEET, 800, 600, 5.0, 5.0)
            .expect("a point inside the div should hit something");
        assert!(
            hit == root || dom_ref.parent(hit) == Some(root),
            "hit {hit:?} should be the div or a child of it (root {root:?})"
        );

        // A point well outside every fragment recovers nothing.
        assert!(
            hit_test_node(&dom_ref, SHEET, 800, 600, 10_000.0, 10_000.0).is_none(),
            "a point outside all fragments should miss"
        );
    }
}
