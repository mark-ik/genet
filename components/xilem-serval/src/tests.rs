/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! End-to-end backend probe (Stage 1a).
//!
//! Drives the full `View`/`ViewSequence` path: builds a view tree into a
//! `ScriptedDom`, rebuilds it with structural and attribute changes, and
//! asserts both the resulting tree shape and the drained `DomMutation`s for:
//!   1. an initial tree,
//!   2. a middle insert,
//!   3. a middle delete,
//!   4. an attribute change and removal.

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut, Namespace, NodeKind};
use serval_scripted_dom::{NodeId, ScriptedDom};
use xilem_core::{MessageCtx, MessageResult, View};

use crate::{DomHandle, El, ServalCtx, ServalElement, el};

// --- MARK: read helpers -------------------------------------------------------

/// Element-child local names of `node`, in DOM order (skips text nodes).
fn element_child_names(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
    dom.dom_children(node)
        .filter(|&c| dom.kind(c) == NodeKind::Element)
        .map(|c| dom.element_name(c).unwrap().local.to_string())
        .collect()
}

/// All child node kinds + payloads (element local name or text data), in order.
fn child_summary(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
    dom.dom_children(node)
        .map(|c| match dom.kind(c) {
            NodeKind::Element => format!("<{}>", dom.element_name(c).unwrap().local),
            NodeKind::Text => format!("#text:{}", dom.text(c).unwrap_or("")),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Read an attribute in the null namespace.
fn attr<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
    dom.attribute(node, &Namespace::from(""), &name.into())
}

fn count_inserted(muts: &[DomMutation<NodeId>]) -> usize {
    muts.iter()
        .filter(|m| matches!(m, DomMutation::Inserted { .. }))
        .count()
}

fn count_removed(muts: &[DomMutation<NodeId>]) -> usize {
    muts.iter()
        .filter(|m| matches!(m, DomMutation::Removed { .. }))
        .count()
}

fn drain(dom: &DomHandle) -> Vec<DomMutation<NodeId>> {
    let mut out = Vec::new();
    dom.borrow_mut().drain_mutations(&mut out);
    out
}

// --- MARK: the view under test ------------------------------------------------

/// `(<a>, optional <b>, <c>)` children under a root `<div>`. The middle `<b>`
/// is present only when `with_middle` is true — toggling it exercises a true
/// middle insert/delete (reference node = `<c>`), not a tail append.
type TestView = El<
    (
        El<&'static str, (), ()>,
        Option<El<&'static str, (), ()>>,
        El<&'static str, (), ()>,
    ),
    (),
    (),
>;

fn app_logic(with_middle: bool, attr_on: bool) -> TestView {
    let middle = with_middle.then(|| el::<_, (), ()>("b", "B"));
    let view = el::<_, (), ()>(
        "div",
        (el::<_, (), ()>("a", "A"), middle, el::<_, (), ()>("c", "C")),
    );
    if attr_on {
        view.attr("id", "root").attr("class", "panel")
    } else {
        view
    }
}

/// Build the view, attach its root element under the document root, and return
/// the live state for subsequent rebuilds.
struct Harness {
    dom: DomHandle,
    ctx: ServalCtx,
    root_el: ServalElement,
    view: TestView,
    view_state: <TestView as View<(), (), ServalCtx>>::ViewState,
}

impl Harness {
    fn build(with_middle: bool, attr_on: bool) -> Self {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut ctx = ServalCtx::new(dom.clone());
        let view = app_logic(with_middle, attr_on);
        let (root_el, view_state) = view.build(&mut ctx, &mut ());
        // Attach the produced root under the document root.
        let doc_root = dom.borrow().document();
        dom.borrow_mut().insert_before(doc_root, root_el.node, None);
        Self {
            dom,
            ctx,
            root_el,
            view,
            view_state,
        }
    }

    fn root(&self) -> NodeId {
        self.root_el.node
    }

    fn rebuild(&mut self, with_middle: bool, attr_on: bool) {
        let next = app_logic(with_middle, attr_on);
        let mut node = self.root_el.node;
        let mut_ref = crate::ServalElementMut {
            node: &mut node,
            dom: self.dom.clone(),
            parent: None,
        };
        next.rebuild(
            &self.view,
            &mut self.view_state,
            &mut self.ctx,
            mut_ref,
            &mut (),
        );
        self.root_el.node = node;
        self.view = next;
    }
}

// --- MARK: 1. initial tree ----------------------------------------------------

#[test]
fn initial_tree_lands_in_dom() {
    let h = Harness::build(false, false);
    let dom = h.dom.borrow();
    let root = h.root();

    assert_eq!(dom.kind(root), NodeKind::Element);
    assert_eq!(dom.element_name(root).unwrap().local.to_string(), "div");
    // <a>, <c> with the middle absent.
    assert_eq!(element_child_names(&dom, root), vec!["a", "c"]);

    // Each leaf holds its text node.
    let a = dom.dom_children(root).next().unwrap();
    assert_eq!(child_summary(&dom, a), vec!["#text:A".to_string()]);
}

#[test]
fn initial_tree_records_inserts() {
    let h = Harness::build(false, false);
    let muts = drain(&h.dom);
    // Detached creations record nothing; each *attach* records one Inserted:
    //   #text:A -> <a>, #text:C -> <c>, <a> -> <div>, <c> -> <div>,
    //   <div> -> document = 5 inserts. (The root <div> create is detached.)
    assert_eq!(count_inserted(&muts), 5, "muts: {muts:?}");
    assert_eq!(count_removed(&muts), 0);
}

// --- MARK: 2. middle insert ---------------------------------------------------

#[test]
fn middle_insert_orders_and_records() {
    let mut h = Harness::build(false, false);
    let _ = drain(&h.dom); // clear the build mutations

    h.rebuild(true, false);

    let dom = h.dom.borrow();
    let root = h.root();
    assert_eq!(
        element_child_names(&dom, root),
        vec!["a", "b", "c"],
        "middle <b> must land between <a> and <c>"
    );
    drop(dom);

    let muts = drain(&h.dom);
    // The inserted <b> plus its text child = 2 inserts, no removals.
    assert_eq!(count_inserted(&muts), 2, "muts: {muts:?}");
    assert_eq!(count_removed(&muts), 0, "muts: {muts:?}");
    // The <b> insert is recorded under the root <div>.
    let root = h.root();
    assert!(
        muts.iter().any(|m| matches!(
            m,
            DomMutation::Inserted { parent, .. } if *parent == root
        )),
        "expected an Inserted under root: {muts:?}"
    );
}

// --- MARK: 3. middle delete ---------------------------------------------------

#[test]
fn middle_delete_orders_and_records() {
    let mut h = Harness::build(true, false); // start with <b> present
    assert_eq!(
        element_child_names(&h.dom.borrow(), h.root()),
        vec!["a", "b", "c"]
    );
    let _ = drain(&h.dom);

    h.rebuild(false, false); // drop the middle

    let dom = h.dom.borrow();
    let root = h.root();
    assert_eq!(element_child_names(&dom, root), vec!["a", "c"]);
    drop(dom);

    let muts = drain(&h.dom);
    // Removing <b> records exactly one Removed (the subtree drops with it).
    assert_eq!(count_removed(&muts), 1, "muts: {muts:?}");
    assert_eq!(count_inserted(&muts), 0, "muts: {muts:?}");
    let root = h.root();
    assert!(
        muts.iter().any(|m| matches!(
            m,
            DomMutation::Removed { former_parent, .. } if *former_parent == root
        )),
        "expected a Removed from root: {muts:?}"
    );
}

// --- MARK: 4. attribute change + removal --------------------------------------

#[test]
fn attribute_set_then_removed() {
    // Build with attributes off, then turn them on (set), then off again (remove).
    let mut h = Harness::build(false, false);
    let _ = drain(&h.dom);

    // Set: id="root", class="panel".
    h.rebuild(false, true);
    {
        let dom = h.dom.borrow();
        let root = h.root();
        assert_eq!(attr(&dom, root, "id"), Some("root"));
        assert_eq!(attr(&dom, root, "class"), Some("panel"));
    }
    let set_muts = drain(&h.dom);
    let set_attr_changes: Vec<_> = set_muts
        .iter()
        .filter(|m| matches!(m, DomMutation::AttributeChanged { .. }))
        .collect();
    assert_eq!(
        set_attr_changes.len(),
        2,
        "two attributes newly added: {set_muts:?}"
    );
    // Newly added -> old_value None.
    assert!(
        set_attr_changes.iter().all(|m| matches!(
            m,
            DomMutation::AttributeChanged {
                old_value: None,
                ..
            }
        )),
        "set: {set_attr_changes:?}"
    );

    // Remove: both attributes gone.
    h.rebuild(false, false);
    {
        let dom = h.dom.borrow();
        let root = h.root();
        assert_eq!(attr(&dom, root, "id"), None);
        assert_eq!(attr(&dom, root, "class"), None);
    }
    let rm_muts = drain(&h.dom);
    let rm_attr_changes: Vec<_> = rm_muts
        .iter()
        .filter(|m| matches!(m, DomMutation::AttributeChanged { .. }))
        .collect();
    assert_eq!(
        rm_attr_changes.len(),
        2,
        "two attributes removed: {rm_muts:?}"
    );
    // Removal records the prior value as old_value.
    assert!(
        rm_attr_changes.iter().all(|m| matches!(
            m,
            DomMutation::AttributeChanged {
                old_value: Some(_),
                ..
            }
        )),
        "remove: {rm_attr_changes:?}"
    );
}

// --- MARK: message routing smoke ----------------------------------------------

#[test]
fn message_to_unknown_path_is_handled() {
    // No event wiring in Stage 1a, but the message plumbing must compile and
    // route. A bare text view returns Stale for any message.
    let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
    let mut ctx = ServalCtx::new(dom.clone());
    let view = "hello";
    let (mut element, mut state) = View::<(), (), ServalCtx>::build(&view, &mut ctx, &mut ());
    let mut node = element.node;
    let env = xilem_core::Environment::new();
    let mut msg = MessageCtx::new(env, Vec::new(), xilem_core::DynMessage::new(()));
    let mut_ref = crate::ServalElementMut {
        node: &mut node,
        dom: dom.clone(),
        parent: None,
    };
    let result: MessageResult<()> =
        View::<(), (), ServalCtx>::message(&view, &mut state, &mut msg, mut_ref, &mut ());
    assert!(matches!(result, MessageResult::Stale));
    element.node = node;
}

// --- MARK: keyed sequences ----------------------------------------------------

#[cfg(test)]
mod keyed {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::LayoutDom;
    use serval_scripted_dom::{NodeId, ScriptedDom};
    use xilem_core::{MessageCtx, MessageResult, Mut, View, ViewMarker};

    use crate::{Keyed, ServalAppRunner, ServalCtx, ServalElement, el};

    #[derive(Default)]
    struct KeyedStats {
        builds: Vec<&'static str>,
        rebuilds: Vec<&'static str>,
        teardowns: Vec<&'static str>,
    }

    impl KeyedStats {
        fn clear(&mut self) {
            self.builds.clear();
            self.rebuilds.clear();
            self.teardowns.clear();
        }
    }

    struct KeyedDemo {
        ids: Vec<&'static str>,
        stats: Rc<RefCell<KeyedStats>>,
    }

    #[derive(Clone)]
    struct TaggedText {
        id: &'static str,
        stats: Rc<RefCell<KeyedStats>>,
    }

    impl ViewMarker for TaggedText {}

    impl View<KeyedDemo, (), ServalCtx> for TaggedText {
        type Element = ServalElement;
        type ViewState = <&'static str as View<KeyedDemo, (), ServalCtx>>::ViewState;

        fn build(
            &self,
            ctx: &mut ServalCtx,
            app_state: &mut KeyedDemo,
        ) -> (Self::Element, Self::ViewState) {
            let _ = app_state;
            self.stats.borrow_mut().builds.push(self.id);
            View::<KeyedDemo, (), ServalCtx>::build(&self.id, ctx, app_state)
        }

        fn rebuild(
            &self,
            prev: &Self,
            view_state: &mut Self::ViewState,
            ctx: &mut ServalCtx,
            element: Mut<'_, Self::Element>,
            app_state: &mut KeyedDemo,
        ) {
            assert_eq!(
                self.id, prev.id,
                "keyed rebuild paired different logical children"
            );
            self.stats.borrow_mut().rebuilds.push(self.id);
            View::<KeyedDemo, (), ServalCtx>::rebuild(
                &self.id, &prev.id, view_state, ctx, element, app_state,
            );
        }

        fn teardown(
            &self,
            view_state: &mut Self::ViewState,
            ctx: &mut ServalCtx,
            element: Mut<'_, Self::Element>,
        ) {
            self.stats.borrow_mut().teardowns.push(self.id);
            View::<KeyedDemo, (), ServalCtx>::teardown(&self.id, view_state, ctx, element);
        }

        fn message(
            &self,
            view_state: &mut Self::ViewState,
            message: &mut MessageCtx,
            element: Mut<'_, Self::Element>,
            app_state: &mut KeyedDemo,
        ) -> MessageResult<()> {
            View::<KeyedDemo, (), ServalCtx>::message(
                &self.id, view_state, message, element, app_state,
            )
        }
    }

    fn keyed_logic(
        demo: &KeyedDemo,
    ) -> impl View<KeyedDemo, (), ServalCtx, Element = ServalElement> + use<> {
        let children: Keyed<&'static str, TaggedText> = demo
            .ids
            .iter()
            .copied()
            .map(|id| {
                (
                    id,
                    TaggedText {
                        id,
                        stats: demo.stats.clone(),
                    },
                )
            })
            .collect();
        el::<_, KeyedDemo, ()>("div", children)
    }

    fn child_texts(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
        dom.dom_children(node)
            .filter_map(|child| dom.text(child).map(str::to_string))
            .collect()
    }

    #[test]
    fn keyed_middle_insert_retains_later_child_state() {
        let stats = Rc::new(RefCell::new(KeyedStats::default()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            Rc::new(RefCell::new(ScriptedDom::new())),
            keyed_logic,
            KeyedDemo {
                ids: vec!["a", "c"],
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear();

        runner.update(|demo| demo.ids = vec!["a", "b", "c"]);

        let dom = runner.dom();
        let dom = dom.borrow();
        assert_eq!(child_texts(&dom, runner.root()), vec!["a", "b", "c"]);
        drop(dom);

        let stats = stats.borrow();
        assert_eq!(stats.builds, vec!["b"]);
        assert_eq!(stats.rebuilds, vec!["a", "c"]);
        assert!(stats.teardowns.is_empty());
    }

    #[test]
    fn keyed_middle_delete_retains_later_child_state() {
        let stats = Rc::new(RefCell::new(KeyedStats::default()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            Rc::new(RefCell::new(ScriptedDom::new())),
            keyed_logic,
            KeyedDemo {
                ids: vec!["a", "b", "c"],
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear();

        runner.update(|demo| demo.ids = vec!["a", "c"]);

        let dom = runner.dom();
        let dom = dom.borrow();
        assert_eq!(child_texts(&dom, runner.root()), vec!["a", "c"]);
        drop(dom);

        let stats = stats.borrow();
        assert!(stats.builds.is_empty());
        assert_eq!(stats.rebuilds, vec!["a", "c"]);
        assert_eq!(stats.teardowns, vec!["b"]);
    }

    #[test]
    fn keyed_reorder_moves_the_element_without_teardown() {
        use layout_dom_api::{DomMutation, LayoutDomMut};

        let stats = Rc::new(RefCell::new(KeyedStats::default()));
        let dom_handle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom_handle.clone(),
            keyed_logic,
            KeyedDemo {
                ids: vec!["a", "b"],
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear();
        let before: Vec<NodeId> = {
            let dom = dom_handle.borrow();
            dom.dom_children(runner.root()).collect()
        };
        {
            // Clear the build-phase mutation noise so the reorder's records
            // stand alone.
            let mut drained = Vec::new();
            dom_handle.borrow_mut().drain_mutations(&mut drained);
        }

        runner.update(|demo| demo.ids = vec!["b", "a"]);

        // Both children keep their DOM nodes (same NodeIds, swapped order):
        // a keyed reorder over single-element children is a move, never a
        // teardown + rebuild. (moveBefore plan S5.)
        let dom = runner.dom();
        let dom = dom.borrow();
        assert_eq!(child_texts(&dom, runner.root()), vec!["b", "a"]);
        let after: Vec<NodeId> = dom.dom_children(runner.root()).collect();
        assert_eq!(after, vec![before[1], before[0]], "same nodes, swapped");
        drop(dom);

        // The DOM observed exactly one atomic Moved — no Removed, no Inserted.
        let mut muts = Vec::new();
        dom_handle.borrow_mut().drain_mutations(&mut muts);
        let root = runner.root();
        assert_eq!(
            muts,
            vec![DomMutation::Moved {
                node: before[1],
                from_parent: root,
                to_parent: root,
            }]
        );

        let stats = stats.borrow();
        assert!(stats.builds.is_empty(), "no child rebuilt from scratch");
        assert_eq!(stats.rebuilds, vec!["b", "a"]);
        assert!(stats.teardowns.is_empty(), "no child torn down");
    }
}

// --- MARK: portable keyed (cross-parent moves, moveBefore S5) ------------------

#[cfg(test)]
mod portable {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{DomMutation, LayoutDom, LayoutDomMut};
    use serval_scripted_dom::{NodeId, ScriptedDom};
    use xilem_core::{MessageCtx, MessageResult, Mut, View, ViewMarker};

    use crate::{
        El, OnClick, PointerClick, PortableKeyed, ServalAppRunner, ServalCtx, ServalElement, el,
        on_click,
    };

    #[derive(Default)]
    struct Stats {
        builds: Vec<&'static str>,
        teardowns: Vec<&'static str>,
    }

    struct MoveDemo {
        left: Vec<&'static str>,
        right: Vec<&'static str>,
        clicks: usize,
        stats: Rc<RefCell<Stats>>,
    }

    /// A count-the-click handler as a plain fn so the inner view type stays
    /// nameable (a capturing closure would make `Tile::ViewState` unnameable).
    fn note_click(demo: &mut MoveDemo, _click: PointerClick) {
        demo.clicks += 1;
    }

    type Inner = OnClick<El<String, MoveDemo, ()>, MoveDemo, (), fn(&mut MoveDemo, PointerClick)>;

    /// A portable tile: a clickable `<p>` labeled with its id, delegating to a
    /// reconstructed inner view (the `TaggedText` pattern) so it can be `Clone`.
    #[derive(Clone)]
    struct Tile {
        id: &'static str,
        stats: Rc<RefCell<Stats>>,
    }

    impl Tile {
        fn inner(&self) -> Inner {
            on_click(
                el::<_, MoveDemo, ()>("p", self.id.to_string()),
                note_click as fn(&mut MoveDemo, PointerClick),
            )
        }
    }

    impl ViewMarker for Tile {}

    impl View<MoveDemo, (), ServalCtx> for Tile {
        type Element = ServalElement;
        type ViewState = <Inner as View<MoveDemo, (), ServalCtx>>::ViewState;

        fn build(
            &self,
            ctx: &mut ServalCtx,
            app_state: &mut MoveDemo,
        ) -> (Self::Element, Self::ViewState) {
            self.stats.borrow_mut().builds.push(self.id);
            self.inner().build(ctx, app_state)
        }

        fn rebuild(
            &self,
            prev: &Self,
            view_state: &mut Self::ViewState,
            ctx: &mut ServalCtx,
            element: Mut<'_, Self::Element>,
            app_state: &mut MoveDemo,
        ) {
            self.inner()
                .rebuild(&prev.inner(), view_state, ctx, element, app_state);
        }

        fn teardown(
            &self,
            view_state: &mut Self::ViewState,
            ctx: &mut ServalCtx,
            element: Mut<'_, Self::Element>,
        ) {
            self.stats.borrow_mut().teardowns.push(self.id);
            self.inner().teardown(view_state, ctx, element);
        }

        fn message(
            &self,
            view_state: &mut Self::ViewState,
            message: &mut MessageCtx,
            element: Mut<'_, Self::Element>,
            app_state: &mut MoveDemo,
        ) -> MessageResult<()> {
            self.inner()
                .message(view_state, message, element, app_state)
        }
    }

    fn pane(ids: &[&'static str], stats: &Rc<RefCell<Stats>>) -> PortableKeyed<&'static str, Tile> {
        ids.iter()
            .copied()
            .map(|id| {
                (
                    id,
                    Tile {
                        id,
                        stats: stats.clone(),
                    },
                )
            })
            .collect()
    }

    fn move_logic(
        demo: &MoveDemo,
    ) -> impl View<MoveDemo, (), ServalCtx, Element = ServalElement> + use<> {
        el::<_, MoveDemo, ()>(
            "div",
            (
                el::<_, MoveDemo, ()>("section", pane(&demo.left, &demo.stats)),
                el::<_, MoveDemo, ()>("section", pane(&demo.right, &demo.stats)),
            ),
        )
    }

    /// The two `<section>` panes under the runner root.
    fn sections(dom: &ScriptedDom, root: NodeId) -> (NodeId, NodeId) {
        let mut kids = dom.dom_children(root);
        let left = kids.next().expect("left section");
        let right = kids.next().expect("right section");
        (left, right)
    }

    fn child_texts(dom: &ScriptedDom, node: NodeId) -> Vec<String> {
        dom.dom_children(node)
            .filter_map(|p| {
                dom.dom_children(p)
                    .find_map(|t| dom.text(t).map(str::to_string))
            })
            .collect()
    }

    fn drain(dom: &Rc<RefCell<ScriptedDom>>) -> Vec<DomMutation<NodeId>> {
        let mut out = Vec::new();
        dom.borrow_mut().drain_mutations(&mut out);
        out
    }

    /// Source-before-target: the tile's element, DOM node, view state, and
    /// event handler all survive a cross-parent move, and the DOM observes
    /// exactly one atomic `Moved` — the tear-out contract. (moveBefore S5.)
    #[test]
    fn cross_parent_move_preserves_element_state_and_handlers() {
        let stats = Rc::new(RefCell::new(Stats::default()));
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move_logic,
            MoveDemo {
                left: vec!["x"],
                right: vec!["y"],
                clicks: 0,
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear_all();
        let (left, right) = sections(&dom.borrow(), runner.root());
        let x_node = dom.borrow().dom_children(left).next().expect("x tile");
        let _ = drain(&dom);

        // Move x from the left pane to the front of the right pane. The left
        // pane (source) rebuilds first in tree order, so it parks x and the
        // right pane adopts it.
        runner.update(|demo| {
            demo.left = vec![];
            demo.right = vec!["x", "y"];
        });

        {
            let dom = dom.borrow();
            assert_eq!(child_texts(&dom, left), Vec::<String>::new());
            assert_eq!(child_texts(&dom, right), vec!["x", "y"]);
            let moved = dom.dom_children(right).next().expect("moved tile");
            assert_eq!(moved, x_node, "the tile keeps its DOM node");
        }
        assert_eq!(
            drain(&dom),
            vec![DomMutation::Moved {
                node: x_node,
                from_parent: left,
                to_parent: right,
            }],
            "one atomic move; nothing removed, nothing inserted"
        );
        {
            let stats = stats.borrow();
            assert!(stats.builds.is_empty(), "no fresh build");
            assert!(stats.teardowns.is_empty(), "no teardown");
        }

        // The handler survived the move AND routes through the new position:
        // without (node, path) reconciliation this would be a stale path.
        runner.dispatch_click(x_node, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state().clicks, 1, "the moved tile's click routes");
    }

    /// Target-before-source: the left pane rebuilds before the right pane has
    /// parked anything, so the arrival misses the nursery and builds fresh;
    /// the departed original is parked, then drained (real teardown + node
    /// removal). Correct, no leak — just no preservation. The ordering caveat
    /// on the module docs, exercised.
    #[test]
    fn target_before_source_falls_back_to_fresh_build_and_drain() {
        let stats = Rc::new(RefCell::new(Stats::default()));
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move_logic,
            MoveDemo {
                left: vec![],
                right: vec!["y"],
                clicks: 0,
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear_all();
        let (left, right) = sections(&dom.borrow(), runner.root());
        let y_node = dom.borrow().dom_children(right).next().expect("y tile");
        let _ = drain(&dom);

        // Move y right → left: the target (left) rebuilds first.
        runner.update(|demo| {
            demo.left = vec!["y"];
            demo.right = vec![];
        });

        {
            let dom = dom.borrow();
            assert_eq!(child_texts(&dom, left), vec!["y"]);
            assert_eq!(child_texts(&dom, right), Vec::<String>::new());
            let fresh = dom.dom_children(left).next().expect("fresh tile");
            assert_ne!(fresh, y_node, "no preservation against tree order");
            assert!(!dom.is_live(y_node), "the departed original is removed");
        }
        let stats = stats.borrow();
        assert_eq!(stats.builds, vec!["y"], "one fresh build");
        assert_eq!(stats.teardowns, vec!["y"], "the parked original drained");
    }

    /// A key that leaves every list is parked, then drained at the end of the
    /// same rebuild: real teardown, node removed — parking never leaks.
    #[test]
    fn removed_key_parks_then_drains_to_real_teardown() {
        let stats = Rc::new(RefCell::new(Stats::default()));
        let dom = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move_logic,
            MoveDemo {
                left: vec!["x"],
                right: vec![],
                clicks: 0,
                stats: stats.clone(),
            },
        );
        stats.borrow_mut().clear_all();
        let (left, _right) = sections(&dom.borrow(), runner.root());
        let x_node = dom.borrow().dom_children(left).next().expect("x tile");
        let _ = drain(&dom);

        runner.update(|demo| demo.left = vec![]);

        {
            let dom = dom.borrow();
            assert_eq!(child_texts(&dom, left), Vec::<String>::new());
            assert!(!dom.is_live(x_node), "drained tile's node is removed");
        }
        let stats = stats.borrow();
        assert!(stats.builds.is_empty());
        assert_eq!(stats.teardowns, vec!["x"], "drain runs the real teardown");
    }

    impl Stats {
        fn clear_all(&mut self) {
            self.builds.clear();
            self.teardowns.clear();
        }
    }
}

// --- MARK: multi-projection runner (one state, N windows) ---------------------

#[cfg(test)]
mod multi {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{DomHandle, El, OnClick, PointerClick, ServalMultiRunner, el, on_click};

    struct Counter {
        count: u32,
    }

    fn bump(state: &mut Counter, _click: PointerClick) {
        state.count += 1;
    }

    type ClickView = OnClick<El<String, Counter, ()>, Counter, (), fn(&mut Counter, PointerClick)>;

    fn click_view(state: &Counter) -> ClickView {
        on_click(
            el::<_, Counter, ()>("div", state.count.to_string()),
            bump as fn(&mut Counter, PointerClick),
        )
    }

    fn fresh_dom() -> DomHandle {
        Rc::new(RefCell::new(ScriptedDom::new()))
    }

    fn text_of(dom: &DomHandle, root: NodeId) -> String {
        let dom = dom.borrow();
        dom.dom_children(root)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
            .unwrap_or_default()
    }

    /// The thesis receipt: one state, N windows, nothing to synchronize — a
    /// single `update` lands in every projection's dom.
    #[test]
    fn one_update_projects_into_every_window() {
        let mut runner = ServalMultiRunner::<_, _, _, ()>::new(Counter { count: 0 });
        let (dom_a, dom_b) = (fresh_dom(), fresh_dom());
        let a = runner.push_projection(dom_a.clone(), click_view);
        let b = runner.push_projection(dom_b.clone(), click_view);
        let (root_a, root_b) = (runner.root(a).unwrap(), runner.root(b).unwrap());
        assert_eq!(text_of(&dom_a, root_a), "0");
        assert_eq!(text_of(&dom_b, root_b), "0");

        runner.update(|s| s.count += 1);

        assert_eq!(text_of(&dom_a, root_a), "1");
        assert_eq!(text_of(&dom_b, root_b), "1");
    }

    /// `update_local` rebuilds only the named projection — the per-window path.
    /// The one state still changes, but the other window's dom is left until it
    /// rebuilds, so a per-window snapshot (an orrery render, pane rows) does not
    /// churn N windows every frame.
    #[test]
    fn update_local_rebuilds_only_that_projection() {
        let mut runner = ServalMultiRunner::<_, _, _, ()>::new(Counter { count: 0 });
        let (dom_a, dom_b) = (fresh_dom(), fresh_dom());
        let a = runner.push_projection(dom_a.clone(), click_view);
        let b = runner.push_projection(dom_b.clone(), click_view);
        let (root_a, root_b) = (runner.root(a).unwrap(), runner.root(b).unwrap());

        runner.update_local(a, |s| s.count += 1);

        assert_eq!(runner.state().count, 1, "the one state changed");
        assert_eq!(text_of(&dom_a, root_a), "1", "the named window rebuilt");
        assert_eq!(text_of(&dom_b, root_b), "0", "the other window was not rebuilt");
    }

    /// A dispatch into window A mutates the one truth and window B reflects it
    /// in the same pass — the mirror fan-outs' replacement.
    #[test]
    fn dispatch_in_one_window_updates_the_others() {
        let mut runner = ServalMultiRunner::<_, _, _, ()>::new(Counter { count: 0 });
        let (dom_a, dom_b) = (fresh_dom(), fresh_dom());
        let a = runner.push_projection(dom_a.clone(), click_view);
        let b = runner.push_projection(dom_b.clone(), click_view);
        let (root_a, root_b) = (runner.root(a).unwrap(), runner.root(b).unwrap());

        runner.dispatch_click(a, root_a, PointerClick::at((0.0, 0.0)));

        assert_eq!(runner.state().count, 1);
        assert_eq!(text_of(&dom_a, root_a), "1");
        assert_eq!(text_of(&dom_b, root_b), "1", "the other window sees it too");
    }

    /// Focus and pointer capture are per-window interaction state, not shared.
    #[test]
    fn focus_is_per_window() {
        let mut runner = ServalMultiRunner::<_, _, _, ()>::new(Counter { count: 0 });
        let a = runner.push_projection(fresh_dom(), click_view);
        let b = runner.push_projection(fresh_dom(), click_view);
        let root_a = runner.root(a).unwrap();

        runner.set_focus(a, Some(root_a));
        assert_eq!(runner.focus(a), Some(root_a));
        assert_eq!(runner.focus(b), None, "window B's focus is its own");
    }

    /// Removing a projection tears its tree down and detaches its root; the
    /// shared state and the other windows are untouched.
    #[test]
    fn remove_projection_tears_down_its_tree_only() {
        let mut runner = ServalMultiRunner::<_, _, _, ()>::new(Counter { count: 0 });
        let (dom_a, dom_b) = (fresh_dom(), fresh_dom());
        let a = runner.push_projection(dom_a.clone(), click_view);
        let b = runner.push_projection(dom_b.clone(), click_view);
        let root_a = runner.root(a).unwrap();
        let root_b = runner.root(b).unwrap();

        runner.remove_projection(a);

        assert_eq!(runner.projection_count(), 1);
        {
            let dom = dom_a.borrow();
            assert!(
                dom.dom_children(dom.document()).next().is_none(),
                "window A's document is empty after teardown"
            );
        }
        runner.update(|s| s.count += 5);
        assert_eq!(text_of(&dom_b, root_b), "5", "window B lives on");
        // A stale handle resolves to nothing rather than panicking.
        assert!(runner.root(a).is_none());
        assert_eq!(
            runner.dispatch_click(a, root_a, PointerClick::at((0.0, 0.0))),
            vec![]
        );
    }
}

// --- MARK: Stage 3a — component composition (backend-only) --------------------
//
// These prove the `xilem_core` composition vocabulary works over `ServalCtx`
// using only this crate + the `ScriptedDom` — no serval-layout/netrender. The
// `pelt-live` suite asserts the same with full render-path coverage; these are
// the boundary-level twin, so a `lens`/`map_action`/`OptionalAction` regression
// is caught even with the engine stack absent.

#[cfg(test)]
mod composition {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, PointerClick, ServalAppRunner, ServalCtx, ServalElement, View, el, lens,
        map_action, on_click,
    };

    /// The text of the single text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// Every element in `node`'s subtree (pre-order) named `name`.
    fn elements_named(dom: &ScriptedDom, node: NodeId, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        fn walk(dom: &ScriptedDom, node: NodeId, name: &str, out: &mut Vec<NodeId>) {
            if dom.kind(node) == NodeKind::Element
                && dom
                    .element_name(node)
                    .is_some_and(|q| q.local.as_ref() == name)
            {
                out.push(node);
            }
            for c in dom.dom_children(node) {
                walk(dom, c, name, out);
            }
        }
        walk(dom, node, name, &mut out);
        out
    }

    // A reusable counter component over a bare `u32` sub-state; the canonical
    // `lens` component shape. `+ use<>` keeps the opaque type from capturing the
    // input lifetime (else it can't be a single `V` for `FnMut(&_) -> V`).
    fn counter_button(
        count: &mut u32,
    ) -> impl View<u32, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, u32, ()>("button", count.to_string()),
            |c: &mut u32, _ev| *c += 1,
        )
    }

    struct App {
        left: u32,
        right: u32,
    }

    fn app_view(_s: &App) -> impl View<App, (), ServalCtx, Element = ServalElement> + use<> {
        el::<_, App, ()>(
            "div",
            (
                lens(counter_button, |s: &mut App| &mut s.left),
                lens(counter_button, |s: &mut App| &mut s.right),
            ),
        )
    }

    /// `lens` composes two independently-stateful counters: clicking the left
    /// button moves only `App::left`, and the rebuild updates only its text.
    #[test]
    fn lens_isolates_substate() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), app_view, App { left: 0, right: 0 });
        let root = runner.root();

        let (left, right) = {
            let d = dom.borrow();
            let bs = elements_named(&d, root, "button");
            assert_eq!(bs.len(), 2, "two counter buttons");
            (bs[0], bs[1])
        };

        runner.dispatch_click(left, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            (runner.state().left, runner.state().right),
            (1, 0),
            "only the lensed left sub-state changes"
        );
        {
            let d = dom.borrow();
            assert_eq!(text_child(&d, left).as_deref(), Some("1"));
            assert_eq!(text_child(&d, right).as_deref(), Some("0"));
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    struct Bump;
    impl crate::Action for Bump {}

    fn bump_button() -> impl View<(), Bump, ServalCtx, Element = ServalElement> + use<> {
        on_click(el::<_, (), Bump>("button", "+"), |_s: &mut (), _ev| Bump)
    }

    struct Parent {
        count: u32,
        unit: (),
    }

    fn parent_view(
        _s: &Parent,
    ) -> impl View<Parent, (), ServalCtx, Element = ServalElement> + use<> {
        let child = lens(|_u: &mut ()| bump_button(), |p: &mut Parent| &mut p.unit);
        el::<_, Parent, ()>(
            "div",
            map_action(child, |p: &mut Parent, _a: Bump| {
                p.count += 1;
            }),
        )
    }

    /// `OptionalAction` + `map_action`: the child returns a `Bump` action, which
    /// `map_action` turns into the parent effect `count += 1`.
    #[test]
    fn map_action_applies_parent_effect() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            parent_view,
            Parent { count: 0, unit: () },
        );
        let root = runner.root();
        let button = {
            let d = dom.borrow();
            elements_named(&d, root, "button")[0]
        };

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            runner.state().count,
            1,
            "child Bump action maps to the parent increment"
        );
    }
}

// --- MARK: Stage 3b — keyboard + focus (backend-only) -------------------------
//
// The headless twin of `pelt-live`'s Stage 3b suite: focus routing, the
// no-focus no-op, click-to-focus, and key bubbling — proven over the
// `ScriptedDom` with no serval-layout/netrender, so a key-registry/focus
// regression is caught even with the engine stack absent.

#[cfg(test)]
mod keyboard {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, El, Key, KeyEvent, Modifiers, NamedKey, OnClick, OnKey, PointerClick,
        ServalAppRunner, ServalCtx, ServalElement, View, el, focusable, on_click, on_key,
    };

    /// The app state: a text buffer a key handler edits.
    struct Editor {
        text: String,
    }

    /// An activation counter — the state a plain button's click handler bumps.
    struct Clicks {
        count: u32,
    }

    /// Append the typed character / apply the editing key to `text`. A free
    /// function (not a closure) so the view type stays a nameable `fn` pointer.
    fn edit(s: &mut Editor, ev: KeyEvent) {
        match ev.key {
            Key::Character(c) => s.text.push_str(&c),
            Key::Named(NamedKey::Backspace) => {
                s.text.pop();
            },
            Key::Named(_) => {},
        }
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
    fn find_element_by_name(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom
                .element_name(node)
                .is_some_and(|q| q.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|c| find_element_by_name(dom, c, name))
    }

    fn ch(s: &str) -> KeyEvent {
        KeyEvent::new(Key::Character(s.to_string()))
    }

    fn named(k: NamedKey) -> KeyEvent {
        KeyEvent::new(Key::Named(k))
    }

    /// A `Tab` key event, optionally with `Shift` (for reverse traversal).
    fn tab(shift: bool) -> KeyEvent {
        KeyEvent::with_mods(
            Key::Named(NamedKey::Tab),
            Modifiers {
                shift,
                ..Default::default()
            },
        )
    }

    /// Tab / Shift+Tab move focus across the focusable set in document order,
    /// wrapping — the runner-level traversal default (a document engine has no
    /// built-in tab order). Two focusable elements (each carries an `on_key`).
    #[test]
    fn tab_traverses_focusables() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let noop: fn(&mut (), KeyEvent) = |_, _| {};
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |_: &()| {
                el::<_, (), ()>(
                    "div",
                    (
                        on_key(el::<_, (), ()>("a", ()), noop),
                        on_key(el::<_, (), ()>("b", ()), noop),
                    ),
                )
            },
            (),
        );
        let (a, b) = {
            let d = dom.borrow();
            let root = runner.root();
            (
                find_element_by_name(&d, root, "a").expect("<a>"),
                find_element_by_name(&d, root, "b").expect("<b>"),
            )
        };

        // Nothing focused → Tab focuses the first; Tab advances; Tab wraps.
        assert_eq!(runner.focus(), None);
        runner.dispatch_key(tab(false));
        assert_eq!(
            runner.focus(),
            Some(a),
            "Tab from nothing focuses the first"
        );
        runner.dispatch_key(tab(false));
        assert_eq!(runner.focus(), Some(b), "Tab advances");
        runner.dispatch_key(tab(false));
        assert_eq!(runner.focus(), Some(a), "Tab wraps to the first");

        // Shift+Tab goes backward (wrapping to the last).
        runner.dispatch_key(tab(true));
        assert_eq!(runner.focus(), Some(b), "Shift+Tab goes backward (wraps)");
    }

    // --- focus routing --------------------------------------------------------

    /// `<div><input on_key=edit/><span/></div>`: a focusable `<input>` (carries a
    /// key handler) beside a non-focusable `<span>`. Concrete `fn`-pointer
    /// handler so the type is nameable.
    type FocusView = El<
        (
            OnKey<El<&'static str, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>,
            El<&'static str, Editor, ()>,
        ),
        Editor,
        (),
    >;

    fn focus_view(_s: &Editor) -> FocusView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        el::<_, Editor, ()>(
            "div",
            (
                on_key(el::<_, Editor, ()>("input", ""), handler),
                el::<_, Editor, ()>("span", "x"),
            ),
        )
    }

    /// Focus the `<input>`, type "h" then "i" (state == "hi"), then Backspace
    /// (state == "h"). The key handler routes to the focused element.
    #[test]
    fn typed_keys_reach_focused_element() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            focus_view,
            Editor {
                text: String::new(),
            },
        );
        let root = runner.root();

        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("an <input> must exist")
        };

        runner.set_focus(Some(input));
        assert_eq!(runner.focus(), Some(input));

        runner.dispatch_key(ch("h"));
        runner.dispatch_key(ch("i"));
        assert_eq!(
            runner.state().text,
            "hi",
            "typed chars append to the buffer"
        );

        runner.dispatch_key(named(NamedKey::Backspace));
        assert_eq!(runner.state().text, "h", "Backspace deletes the last char");
    }

    /// With no focus (never focused), `dispatch_key` is a no-op: it returns empty
    /// and leaves state unchanged. Also covered: explicitly clearing focus.
    #[test]
    fn no_focus_is_a_noop() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            focus_view,
            Editor {
                text: String::new(),
            },
        );

        assert_eq!(runner.focus(), None, "nothing focused initially");
        let out = runner.dispatch_key(ch("h"));
        assert!(out.is_empty(), "no focus → empty action vec");
        assert_eq!(runner.state().text, "", "no focus → state unchanged");

        // Explicitly clearing focus is the same no-op.
        runner.set_focus(None);
        let out = runner.dispatch_key(ch("z"));
        assert!(out.is_empty());
        assert_eq!(runner.state().text, "");
    }

    // --- click sets focus -----------------------------------------------------

    /// `<div on_key=edit><label/></div>`: the focusable `<div>` carries the key
    /// handler and contains a non-focusable `<label>` child. The div also has no
    /// click handler — focus does not depend on `on_click`.
    type ClickFocusView =
        OnKey<El<El<&'static str, Editor, ()>, Editor, ()>, Editor, (), fn(&mut Editor, KeyEvent)>;

    fn click_focus_view(_s: &Editor) -> ClickFocusView {
        let handler: fn(&mut Editor, KeyEvent) = edit;
        on_key(
            el::<_, Editor, ()>("div", el::<_, Editor, ()>("label", "L")),
            handler,
        )
    }

    /// Clicking the focusable div (via its child) focuses the div; a subsequent
    /// `dispatch_key` routes there. Then clicking a non-focusable node clears
    /// focus to `None`.
    #[test]
    fn click_sets_and_clears_focus() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_focus_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root(); // the OnKey wraps the root <div>

        let label = {
            let d = dom.borrow();
            find_element_by_name(&d, div, "label").expect("a <label>")
        };

        assert_eq!(runner.focus(), None);

        // Click the label (a child of the focusable div) → focus the div, since
        // the nearest focusable ancestor of the label is the div.
        runner.dispatch_click(label, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            runner.focus(),
            Some(div),
            "clicking inside the focusable div focuses the div"
        );

        // A key now reaches the div's handler.
        runner.dispatch_key(ch("a"));
        assert_eq!(runner.state().text, "a", "key routes to the focused div");

        // Clicking a node whose ancestor chain has no key handler clears focus.
        // The document root is such a node (above the div, no handler).
        let doc = dom.borrow().document();
        runner.dispatch_click(doc, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            runner.focus(),
            None,
            "clicking outside any focusable element clears focus"
        );
    }

    // --- key bubbling ---------------------------------------------------------

    /// A key handler on a *parent* fires when the focused child has none.
    /// `<div on_key=edit><button on_click=noop/></div>`: focus the button (no key
    /// handler) and dispatch — the key bubbles to the div's handler. The button
    /// carries only a click handler, so it is *not* focusable; we `set_focus`
    /// it directly to model "a non-focusable node happened to be focused" and
    /// confirm the bubble still reaches the parent.
    type BubbleKeyView = OnKey<
        El<
            OnClick<El<&'static str, Editor, ()>, Editor, (), fn(&mut Editor, PointerClick)>,
            Editor,
            (),
        >,
        Editor,
        (),
        fn(&mut Editor, KeyEvent),
    >;

    fn bubble_key_view(_s: &Editor) -> BubbleKeyView {
        let key_handler: fn(&mut Editor, KeyEvent) = edit;
        let click_noop: fn(&mut Editor, PointerClick) = |_s, _ev| {};
        on_key(
            el::<_, Editor, ()>(
                "div",
                on_click(el::<_, Editor, ()>("button", "+"), click_noop),
            ),
            key_handler,
        )
    }

    #[test]
    fn key_bubbles_from_focused_child_to_parent_handler() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            bubble_key_view,
            Editor {
                text: String::new(),
            },
        );
        let div = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, div, "button").expect("a <button>")
        };
        assert_ne!(
            button, div,
            "button is a descendant of the handler-bearing div"
        );

        // The button has only a click handler, so it is not focusable; aim focus
        // at it directly to exercise the bubble (focus on a child with no key
        // handler → the parent's handler fires).
        runner.set_focus(Some(button));
        runner.dispatch_key(ch("z"));
        assert_eq!(
            runner.state().text,
            "z",
            "key on the focused child bubbles to the parent div's handler"
        );
    }

    // --- focusable() marker + Enter/Space activation (G2.3) --------------------

    /// `focusable(on_click(button))`: a plain button (a click handler, no key
    /// handler) made keyboard-operable by `focusable`. Without the marker it would
    /// be tab-unreachable and un-activatable.
    fn focusable_button_view(
        _s: &Clicks,
    ) -> impl View<Clicks, (), ServalCtx, Element = ServalElement> + use<> {
        focusable(on_click(
            el::<_, Clicks, ()>("button", "+"),
            |s: &mut Clicks, _ev: PointerClick| s.count += 1,
        ))
    }

    /// `focusable` puts the plain button in the Tab order, and Enter/Space activate
    /// it by synthesizing a click; a character key does not.
    #[test]
    fn enter_and_space_activate_a_focusable_button() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            focusable_button_view,
            Clicks { count: 0 },
        );
        let button = runner.root(); // `focusable` wraps the button, passing it through

        // Tab reaches it even though it carries no key handler — `focusable` put it
        // in the focusable set.
        assert_eq!(runner.focus(), None);
        runner.dispatch_key(tab(false));
        assert_eq!(
            runner.focus(),
            Some(button),
            "focusable() puts the plain button in the Tab order"
        );

        // Enter and Space each activate it (a synthesized click runs the on_click).
        runner.dispatch_key(named(NamedKey::Enter));
        assert_eq!(
            runner.state().count,
            1,
            "Enter activates the focused button"
        );
        runner.dispatch_key(named(NamedKey::Space));
        assert_eq!(
            runner.state().count,
            2,
            "Space activates the focused button"
        );

        // A character key is not an activation.
        runner.dispatch_key(ch("x"));
        assert_eq!(
            runner.state().count,
            2,
            "a character key does not activate the button"
        );
    }

    /// `clickable(child, handler)` is `focusable(on_click(..))` in one combinator: the div is
    /// Tab-reachable, Enter activates it (the synthesized click), and a pointer click does too.
    #[test]
    fn clickable_is_focusable_and_activatable() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_s: &Clicks| {
                crate::clickable(
                    el::<_, Clicks, ()>("div", "row"),
                    |s: &mut Clicks, _ev: PointerClick| s.count += 1,
                )
            },
            Clicks { count: 0 },
        );
        let row = runner.root();

        runner.dispatch_key(tab(false));
        assert_eq!(
            runner.focus(),
            Some(row),
            "clickable() puts the div in the Tab order"
        );
        runner.dispatch_key(named(NamedKey::Enter));
        assert_eq!(runner.state().count, 1, "Enter activates the clickable");
        runner.dispatch_click(row, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state().count, 2, "a pointer click activates it too");
    }

    // --- Tab is overridable per-view (G2.3) -----------------------------------

    /// Tab is delivered to the focused element's `on_key` first; a handler that
    /// inserts a tab character and calls `prevent_default` keeps focus put (the
    /// textarea case), where an ordinary focusable lets the runner traverse.
    #[test]
    fn tab_is_overridable_by_a_handler() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        // The first field captures Tab: insert '\t' and prevent the traversal.
        let trap: fn(&mut Editor, KeyEvent) = |s, ev| {
            if matches!(ev.key, Key::Named(NamedKey::Tab)) {
                s.text.push('\t');
                ev.prevent_default();
            }
        };
        let plain: fn(&mut Editor, KeyEvent) = |_, _| {};
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |_: &Editor| {
                el::<_, Editor, ()>(
                    "div",
                    (
                        on_key(el::<_, Editor, ()>("input", ""), trap),
                        on_key(el::<_, Editor, ()>("textarea", ""), plain),
                    ),
                )
            },
            Editor {
                text: String::new(),
            },
        );
        let (input, textarea) = {
            let d = dom.borrow();
            let root = runner.root();
            (
                find_element_by_name(&d, root, "input").expect("<input>"),
                find_element_by_name(&d, root, "textarea").expect("<textarea>"),
            )
        };

        // Focus the trap field, press Tab: the handler consumes it (a tab char) and
        // prevents the default, so focus stays — the override.
        runner.set_focus(Some(input));
        runner.dispatch_key(tab(false));
        assert_eq!(
            runner.focus(),
            Some(input),
            "a Tab handler that prevents default keeps focus put"
        );
        assert_eq!(
            runner.state().text,
            "\t",
            "the overriding handler inserted a tab character"
        );

        // The plain field does not prevent: Tab there still traverses (wrapping to
        // the first focusable), proving the default survives where unoverridden.
        runner.set_focus(Some(textarea));
        runner.dispatch_key(tab(false));
        assert_eq!(
            runner.focus(),
            Some(input),
            "an un-prevented Tab still traverses the focusable set"
        );
    }
}

// --- MARK: Stage 3 — form controls (text_field, backend-only) -----------------
//
// The headless twin of `pelt-live`'s Stage 3 form-control coverage: a reusable
// `text_field` whose state is its own `TextInput` (buffer + caret), edited through the focus + key
// dispatch foundation, and composed under a larger struct via `lens` — proven
// over the `ScriptedDom` with no serval-layout/netrender, so a regression in the
// field's edit handler or its `lens` composition is caught with the engine stack
// absent. (NB: per Stage 3b, space arrives as `Named(Space)`, not
// `Character(" ")`, so the sequence below exercises that path explicitly.)

#[cfg(test)]
mod controls {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        AnyView, DomHandle, Key, KeyEvent, Modifiers, NamedKey, PointerClick, PointerEvent,
        PointerPhase, RadioGroup, SelectState, ServalAppRunner, ServalCtx, ServalElement, Slider,
        TextInput, View, WheelEvent, button, checkbox, el, lens, on_pointer, on_wheel, overlay_at,
        overlay_rect, radio_group, select, slider, text_field,
    };

    /// The text data of the single text child under `node`, if any.
    fn text_child(dom: &ScriptedDom, node: NodeId) -> Option<String> {
        dom.dom_children(node)
            .find(|&c| dom.kind(c) == NodeKind::Text)
            .and_then(|c| dom.text(c).map(str::to_string))
    }

    /// All text in `node`'s subtree concatenated in document order. The field
    /// renders its content as `(before, preedit-span, after)`, so its rendered
    /// text is the concatenation, not a single child.
    fn field_text(dom: &ScriptedDom, node: NodeId) -> String {
        fn go(dom: &ScriptedDom, node: NodeId, out: &mut String) {
            if dom.kind(node) == NodeKind::Text {
                if let Some(t) = dom.text(node) {
                    out.push_str(t);
                }
            }
            for c in dom.dom_children(node) {
                go(dom, c, out);
            }
        }
        let mut out = String::new();
        go(dom, node, &mut out);
        out
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
    fn find_element_by_name(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom
                .element_name(node)
                .is_some_and(|q| q.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|c| find_element_by_name(dom, c, name))
    }

    fn ch(s: &str) -> KeyEvent {
        KeyEvent::new(Key::Character(s.to_string()))
    }

    fn named(k: NamedKey) -> KeyEvent {
        KeyEvent::new(Key::Named(k))
    }

    /// Type the canonical sequence into a focused field; the buffer progresses:
    ///   "h", "i", Space, "y", "o", Backspace
    ///     → "h", "hi", "hi ", "hi y", "hi yo", "hi y"
    /// Space goes through `Named(Space)` (the Stage 3b convention); the caret
    /// stays at the end throughout, so each char inserts there and Backspace
    /// removes the trailing "o". Callers assert the resulting buffer / DOM text.
    fn type_hi_y(runner: &mut impl FieldRunner) {
        runner.key(ch("h"));
        runner.key(ch("i"));
        runner.key(named(NamedKey::Space));
        runner.key(ch("y"));
        runner.key(ch("o"));
        runner.key(named(NamedKey::Backspace));
    }

    /// Minimal shim so `type_hi_y` drives either runner shape (the bare-`String`
    /// runner and the `lens`-composed struct runner) through one `dispatch_key`.
    trait FieldRunner {
        fn key(&mut self, ev: KeyEvent);
    }

    // --- bare String state ----------------------------------------------------

    impl<Logic, V> FieldRunner for ServalAppRunner<TextInput, Logic, V, ()>
    where
        Logic: FnMut(&TextInput) -> V,
        V: View<TextInput, (), ServalCtx, Element = ServalElement>,
    {
        fn key(&mut self, ev: KeyEvent) {
            self.dispatch_key(ev);
        }
    }

    /// A `text_field` over bare `TextInput` state: focus it, type the sequence,
    /// and assert the buffer reads `"hi y"` with the caret at the end, and the
    /// `<input>` DOM text is the clean buffer (the caret is painted, not in text).
    #[test]
    fn text_field_edits_its_own_buffer() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            // The field's state IS the whole app state here: a `TextInput`.
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let root = runner.root();

        // The field renders an <input> (the focusable element).
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("the field renders an <input>")
        };

        runner.set_focus(Some(input));
        type_hi_y(&mut runner);

        assert_eq!(runner.state().text(), "hi y", "the field edited its buffer");
        assert_eq!(
            runner.state().caret(),
            4,
            "caret sits at the end after typing"
        );
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "hi y",
            "the <input> DOM text is the clean buffer"
        );
    }

    /// Caret movement + insert/delete at the caret: type, move left, insert in the
    /// middle, then delete on both sides. Proves the field is an insertion-point
    /// editor (not append-only), tracking the caret index as it moves. The DOM
    /// text is the clean buffer (the caret is painted on screen, not in the text).
    #[test]
    fn text_field_caret_moves_and_edits() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "input").expect("an <input>")
        };
        runner.set_focus(Some(input));

        runner.key(ch("h"));
        runner.key(ch("i")); // "hi", caret 2
        runner.key(named(NamedKey::ArrowLeft)); // caret 1: "h|i"
        runner.key(ch("X")); // insert at 1: "hXi", caret 2
        assert_eq!(runner.state().text(), "hXi");
        assert_eq!(runner.state().caret(), 2, "caret after the inserted X");
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "hXi",
            "DOM text is the clean buffer; the caret is painted, not in the text"
        );

        runner.key(named(NamedKey::ArrowLeft));
        runner.key(named(NamedKey::ArrowLeft)); // caret 0: "|hXi"
        runner.key(named(NamedKey::Delete)); // delete after caret (h): "Xi", caret 0
        assert_eq!(runner.state().text(), "Xi");
        assert_eq!(runner.state().caret(), 0);

        runner.key(named(NamedKey::ArrowRight)); // caret 1
        runner.key(named(NamedKey::Backspace)); // delete before caret (X): "i", caret 0
        assert_eq!(runner.state().text(), "i");
        assert_eq!(runner.state().caret(), 0, "caret back at the start");
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "i",
            "DOM text is the clean buffer"
        );
    }

    /// `TextInput`'s edits are character-correct across a multi-byte char (`é` is
    /// two UTF-8 bytes), exercised directly on the model (no runner): a byte-index
    /// bug would split `é` and panic or corrupt the buffer.
    #[test]
    fn text_input_edits_are_char_correct() {
        let mut t = TextInput::new("aé"); // 2 chars, caret at 2
        assert_eq!(t.caret(), 2);
        t.move_left(false); // caret 1 (between 'a' and 'é')
        t.insert_str("X"); // "aXé", caret 2
        assert_eq!(t.text(), "aXé");
        assert_eq!(t.caret(), 2);
        t.backspace(); // remove 'X' before caret: "aé", caret 1
        assert_eq!(t.text(), "aé");
        t.delete(); // remove 'é' after caret: "a", caret 1
        assert_eq!(t.text(), "a");
        assert_eq!(t.caret(), 1);
        assert_eq!(t.display(), "a|");
    }

    // --- lens-composed struct field -------------------------------------------

    /// A larger app with an independently-edited text field plus a sibling field
    /// the edits must *not* touch — proving the `text_field` composes via `lens`.
    struct Form {
        name: TextInput,
        other: String,
    }

    impl<Logic, V> FieldRunner for ServalAppRunner<Form, Logic, V, ()>
    where
        Logic: FnMut(&Form) -> V,
        V: View<Form, (), ServalCtx, Element = ServalElement>,
    {
        fn key(&mut self, ev: KeyEvent) {
            self.dispatch_key(ev);
        }
    }

    /// `lens(text_field, |f| &mut f.name)` under a `<div>`: the field edits only
    /// `Form::name`, leaving `Form::other` untouched. The same key sequence as
    /// the bare-`String` test, proving the field is a drop-in `lens` component.
    #[test]
    fn text_field_composes_under_lens() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_f: &Form| {
                // `text_field` takes `&TextInput` (the field renders from a
                // shared read), so a thin `|s: &mut TextInput| text_field(s)`
                // adapter bridges it to the `Fn(&mut ChildState) -> View` shape
                // `lens` wants — the same adapter the Stage 3a `bump_button` test
                // uses.
                el::<_, Form, ()>(
                    "div",
                    lens(
                        |s: &mut TextInput| text_field(s),
                        |f: &mut Form| &mut f.name,
                    ),
                )
            },
            Form {
                name: TextInput::default(),
                other: "untouched".to_string(),
            },
        );
        let root = runner.root();

        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "input").expect("the field renders an <input>")
        };

        runner.set_focus(Some(input));
        type_hi_y(&mut runner);

        assert_eq!(
            runner.state().name.text(),
            "hi y",
            "only the lensed field changed"
        );
        assert_eq!(
            runner.state().other,
            "untouched",
            "the sibling field must be untouched"
        );
        assert_eq!(
            text_child(&dom.borrow(), input).as_deref(),
            Some("hi y"),
            "the rebuild reflected the edits into the lensed field's <input> text"
        );
    }

    /// Home / End jump the caret to the line ends (the new named keys), driven
    /// through the full key-dispatch + edit path.
    #[test]
    fn text_field_home_end_move_caret() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "input").expect("an <input>")
        };
        runner.set_focus(Some(input));

        runner.key(ch("a"));
        runner.key(ch("b"));
        runner.key(ch("c")); // "abc", caret 3
        runner.key(named(NamedKey::Home));
        assert_eq!(runner.state().caret(), 0, "Home jumps to the start");
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "abc",
            "Home moves the caret, not the buffer text"
        );

        runner.key(named(NamedKey::End));
        assert_eq!(runner.state().caret(), 3, "End jumps to the end");
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "abc",
            "End moves the caret, not the buffer text"
        );
    }

    /// Read a null-namespace attribute of `node`.
    fn attr(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<String> {
        dom.attribute(node, &Namespace::from(""), &LocalName::from(name))
            .map(str::to_string)
    }

    /// A `checkbox` over a `bool`: clicking toggles the bool and reflects it as
    /// `aria-checked` + the `checked` class (for a11y + styling).
    #[test]
    fn checkbox_toggles_and_reflects_state() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            ServalAppRunner::<_, _, _, ()>::new(dom.clone(), |c: &bool| checkbox(*c), false);
        let cb = runner.root(); // the checkbox is the whole view → its root element

        assert_eq!(runner.state(), &false);
        assert_eq!(
            attr(&dom.borrow(), cb, "aria-checked").as_deref(),
            Some("false")
        );
        assert_eq!(
            attr(&dom.borrow(), cb, "class").as_deref(),
            Some("checkbox")
        );

        // Click → checked.
        runner.dispatch_click(cb, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state(), &true);
        assert_eq!(
            attr(&dom.borrow(), cb, "aria-checked").as_deref(),
            Some("true")
        );
        assert_eq!(
            attr(&dom.borrow(), cb, "class").as_deref(),
            Some("checkbox checked")
        );

        // Click again → back to unchecked.
        runner.dispatch_click(cb, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state(), &false);
    }

    /// A `button(label, handler)`: renders a `<button>` with the label, and a
    /// click runs the handler.
    #[test]
    fn button_runs_its_handler() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let inc: fn(&mut u32, PointerClick) = |n, _| *n += 1;
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |_: &u32| button("inc", inc),
            0u32,
        );
        let btn = runner.root();

        assert_eq!(
            dom.borrow().element_name(btn).unwrap().local.as_ref(),
            "button"
        );
        assert_eq!(text_child(&dom.borrow(), btn).as_deref(), Some("inc"));

        runner.dispatch_click(btn, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state(), &1);
    }

    /// `button(label, h).attr("class", ..)`: the fluent `OnClick::attr` forwards
    /// to the wrapped `<button>`, so a class (or any attribute) can be set after
    /// wrapping. The handler still fires.
    #[test]
    fn button_attr_stamps_class_and_keeps_handler() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let inc: fn(&mut u32, PointerClick) = |n, _| *n += 1;
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |_: &u32| button("inc", inc).attr("class", "primary"),
            0u32,
        );
        let btn = runner.root();

        assert_eq!(
            dom.borrow().element_name(btn).unwrap().local.as_ref(),
            "button"
        );
        assert_eq!(
            attr(&dom.borrow(), btn, "class").as_deref(),
            Some("primary")
        );

        runner.dispatch_click(btn, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state(), &1);
    }

    /// `overlay_at(x, y, content)`: a `<div>` carrying its position in an inline
    /// `style` (`position: absolute` + the insets), wrapping the content. The
    /// inline style is what serval's cascade reads to place the overlay.
    #[test]
    fn overlay_at_carries_inline_position() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &()| overlay_at::<_, (), ()>(30.0, 15.0, "menu"),
            (),
        );
        let ov = runner.root();

        assert_eq!(dom.borrow().element_name(ov).unwrap().local.as_ref(), "div");
        assert_eq!(text_child(&dom.borrow(), ov).as_deref(), Some("menu"));
        assert_eq!(
            attr(&dom.borrow(), ov, "style").as_deref(),
            Some("position: absolute; left: 30px; top: 15px;"),
        );
    }

    /// `overlay_rect(x, y, w, h, content)`: a positioned `<div>` that also carries
    /// `width`/`height` in its inline `style`, so the host can size a surface (a card,
    /// the comms pane) from the view rather than re-stamping geometry each frame.
    #[test]
    fn overlay_rect_carries_inline_geometry() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &()| overlay_rect::<_, (), ()>(30.0, 15.0, 200.0, 120.0, "card"),
            (),
        );
        let ov = runner.root();

        assert_eq!(dom.borrow().element_name(ov).unwrap().local.as_ref(), "div");
        assert_eq!(text_child(&dom.borrow(), ov).as_deref(), Some("card"));
        assert_eq!(
            attr(&dom.borrow(), ov, "style").as_deref(),
            Some("position: absolute; left: 30px; top: 15px; width: 200px; height: 120px;"),
        );
    }

    /// `select`: clicking the box opens the option list (an absolute `top: 100%`
    /// child appears); clicking an option sets `selected` and closes the list.
    #[test]
    fn select_opens_picks_and_closes() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let opts = ["red", "green", "blue"];
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |s: &SelectState| select(s, &opts),
            SelectState::new(0),
        );
        let root = runner.root();

        // Closed: selected = 0, the root has only the box child (no list).
        assert_eq!(runner.state().selected, 0);
        assert!(!runner.state().open);
        assert_eq!(
            dom.borrow().dom_children(root).count(),
            1,
            "closed: box only"
        );

        // Click the box (root's first child) → the list opens.
        let box_node = dom.borrow().dom_children(root).next().expect("box");
        runner.dispatch_click(box_node, PointerClick::at((0.0, 0.0)));
        assert!(runner.state().open, "clicking the box opens the list");
        assert_eq!(
            dom.borrow().dom_children(root).count(),
            2,
            "open: box + list"
        );

        // Click the second option (index 1) → selected = 1, list closes.
        let opt1 = {
            let dom = dom.borrow();
            let list = dom.dom_children(root).nth(1).expect("list");
            dom.dom_children(list).nth(1).expect("second option")
        };
        runner.dispatch_click(opt1, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state().selected, 1, "picking option 1 selects it");
        assert!(!runner.state().open, "picking an option closes the list");
        assert_eq!(
            dom.borrow().dom_children(root).count(),
            1,
            "closed again: box only"
        );

        // The box now shows the new selection.
        let box_node = dom.borrow().dom_children(root).next().expect("box");
        assert_eq!(
            text_child(&dom.borrow(), box_node).as_deref(),
            Some("green")
        );
    }

    /// `Box<dyn AnyView>` whose inner view changes *type* across a rebuild
    /// (`<div>` → `<span>`): the element node is swapped in place via
    /// `AnyElement::replace_inner`, staying attached under the document. Proves
    /// erased/dynamic views work on serval's uniform element type.
    #[test]
    fn any_view_swaps_node_on_type_change() {
        // The two branches are *different concrete View types* (their children
        // sequences differ: a single text vs a two-text tuple → `El<&str,…>` vs
        // `El<(&str,&str),…>`), so `AnyView` sees a type change and must take the
        // `replace_inner` path — not a same-type rebuild (which wouldn't even
        // re-tag the element).
        fn view(on: &bool) -> Box<dyn AnyView<bool, (), ServalCtx, ServalElement>> {
            if *on {
                Box::new(el::<_, bool, ()>("span", ("on", "!")))
            } else {
                Box::new(el::<_, bool, ()>("div", "off"))
            }
        }
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            view as fn(&bool) -> Box<dyn AnyView<bool, (), ServalCtx, ServalElement>>,
            false,
        );

        let root0 = runner.root();
        {
            let dom = dom.borrow();
            assert_eq!(dom.element_name(root0).unwrap().local.as_ref(), "div");
            assert_eq!(text_child(&dom, root0).as_deref(), Some("off"));
            assert!(dom.dom_children(dom.document()).any(|c| c == root0));
        }

        // Flip → the boxed view switches <div> → <span>: a type change, so the
        // node is replaced in place under the document.
        runner.update(|s| *s = true);
        let root1 = runner.root();
        assert_ne!(root0, root1, "a type change swaps the node");
        {
            let dom = dom.borrow();
            assert_eq!(dom.element_name(root1).unwrap().local.as_ref(), "span");
            assert_eq!(
                dom.dom_children(root1).count(),
                2,
                "span has its two text children"
            );
            assert!(
                dom.dom_children(dom.document()).any(|c| c == root1),
                "new node attached"
            );
            assert!(
                !dom.dom_children(dom.document()).any(|c| c == root0),
                "old node detached"
            );
        }
    }

    /// `radio_group`: clicking an option selects it (and only it), reflected in
    /// `selected` and `aria-checked`.
    #[test]
    fn radio_group_selects_one() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let opts = ["a", "b", "c"];
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |s: &RadioGroup| radio_group(s, &opts),
            RadioGroup::new(0),
        );
        let root = runner.root();
        assert_eq!(runner.state().selected, 0);

        // Click the third option (index 2).
        let opt2 = {
            dom.borrow()
                .dom_children(root)
                .nth(2)
                .expect("third option")
        };
        runner.dispatch_click(opt2, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state().selected, 2, "clicking option 2 selects it");

        // aria-checked reflects the selection: option 2 true, option 0 false.
        let opt0 = dom
            .borrow()
            .dom_children(root)
            .next()
            .expect("first option");
        let opt2 = dom
            .borrow()
            .dom_children(root)
            .nth(2)
            .expect("third option");
        assert_eq!(
            attr(&dom.borrow(), opt2, "aria-checked").as_deref(),
            Some("true")
        );
        assert_eq!(
            attr(&dom.borrow(), opt0, "aria-checked").as_deref(),
            Some("false")
        );
    }

    /// `TextInput` multi-line navigation (the textarea model): up/down move between
    /// `\n`-delimited lines keeping a **sticky goal column** (Tier 2), clamped to each
    /// line's length but restored on a longer line; home/end scope to the current line.
    #[test]
    fn textinput_line_navigation() {
        use crate::TextInput;
        // "abc\nde\nfghi": chars a0 b1 c2 \n3 d4 e5 \n6 f7 g8 h9 i10 (len 11).
        let mut t = TextInput::new("abc\nde\nfghi");
        assert_eq!(t.caret(), 11, "new() puts the caret at the end");
        t.move_up(false);
        assert_eq!(
            t.caret(),
            6,
            "up: goal col 4 clamps to end of 'de' (offset 6)"
        );
        t.move_up(false);
        // Tier 2: the goal column stays 4 (the original), so it clamps to the end of the
        // 3-char 'abc' (offset 3) — it does *not* drift to the clamped col 2 of 'de'.
        assert_eq!(
            t.caret(),
            3,
            "up: sticky goal col 4 clamps to end of 'abc' (offset 3)"
        );
        t.move_down(false);
        assert_eq!(
            t.caret(),
            6,
            "down: sticky goal col 4 clamps to end of 'de' (offset 6)"
        );
        t.home_line(false);
        assert_eq!(
            t.caret(),
            4,
            "home: start of 'de' (offset 4); resets the goal column"
        );
        t.end_line(false);
        assert_eq!(t.caret(), 6, "end: end of 'de' (offset 6)");
    }

    /// Pointer drag: `down` captures the element and routes a `Down`, `move`
    /// routes to the captured element (cursor wherever), `up` routes `Up` and
    /// clears capture — the foundation under sliders / scrollbar-drag.
    #[test]
    fn pointer_drag_captures_and_routes() {
        #[derive(Default)]
        struct Drag {
            phases: Vec<PointerPhase>,
            last_x: f32,
        }
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &Drag| {
                on_pointer(
                    el::<_, Drag, ()>("div", "track"),
                    |s: &mut Drag, e: PointerEvent| {
                        s.phases.push(e.phase);
                        s.last_x = e.local.0;
                    },
                )
            },
            Drag::default(),
        );
        let node = runner.root();

        runner.dispatch_pointer_down(
            node,
            PointerEvent::new(PointerPhase::Down, (5.0, 0.0), (100.0, 10.0)),
        );
        assert_eq!(
            runner.pointer_capture(),
            Some(node),
            "down captures the element"
        );

        runner.dispatch_pointer_move(PointerEvent::new(
            PointerPhase::Move,
            (40.0, 0.0),
            (100.0, 10.0),
        ));
        runner.dispatch_pointer_up(PointerEvent::new(
            PointerPhase::Up,
            (40.0, 0.0),
            (100.0, 10.0),
        ));
        assert_eq!(runner.pointer_capture(), None, "up clears capture");
        assert_eq!(
            runner.state().phases,
            vec![PointerPhase::Down, PointerPhase::Move, PointerPhase::Up]
        );
        assert_eq!(
            runner.state().last_x,
            40.0,
            "the captured handler saw the move's local x"
        );
    }

    /// Pointer cancellation (G1.3): a drag handler that calls
    /// `e.prop.prevent_default()` is recorded, and — the regression this closes —
    /// each pointer pass records its *own* value, so a later un-preventing event
    /// resets `default_prevented` rather than inheriting the press's stale `true`
    /// (or a prior click/key's).
    #[test]
    fn each_pointer_event_records_its_own_default_prevented() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &()| {
                on_pointer(
                    el::<_, (), ()>("div", "track"),
                    |_s: &mut (), e: PointerEvent| {
                        // Prevent the default on the press only; the move does not.
                        if e.phase == PointerPhase::Down {
                            e.prop.prevent_default();
                        }
                    },
                )
            },
            (),
        );
        let node = runner.root();

        runner.dispatch_pointer_down(
            node,
            PointerEvent::new(PointerPhase::Down, (5.0, 0.0), (100.0, 10.0)),
        );
        assert!(
            runner.default_prevented(),
            "the press handler's prevent_default is recorded"
        );

        runner.dispatch_pointer_move(PointerEvent::new(
            PointerPhase::Move,
            (40.0, 0.0),
            (100.0, 10.0),
        ));
        assert!(
            !runner.default_prevented(),
            "the move records its own un-prevented value, not the press's stale true",
        );
    }

    /// Wheel: a notch routes to the nearest ancestor (including the hit node)
    /// carrying an `on_wheel` handler, with no capture — each notch resolves its
    /// own target and accumulates. A notch whose ancestors carry no handler is a
    /// no-op. The scroll foundation parallel to `on_pointer`.
    #[test]
    fn wheel_routes_to_nearest_handler_no_capture() {
        #[derive(Default)]
        struct Scroll {
            total_y: f32,
            last_local: (f32, f32),
            notches: u32,
        }
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &Scroll| {
                on_wheel(
                    el::<_, Scroll, ()>("div", "scroller"),
                    |s: &mut Scroll, e: WheelEvent| {
                        s.total_y += e.delta.1;
                        s.last_local = e.local;
                        s.notches += 1;
                    },
                )
            },
            Scroll::default(),
        );
        let scroller = runner.root();
        assert_eq!(
            runner.wheel_target(scroller),
            Some(scroller),
            "the scroller is its own target"
        );

        // A notch on the handler node routes to it, carrying delta + cursor-local.
        runner.dispatch_wheel(
            scroller,
            WheelEvent::new((0.0, -30.0), (5.0, 8.0), (100.0, 50.0)),
        );
        assert_eq!(runner.state().notches, 1, "the wheel reached the handler");
        assert_eq!(runner.state().total_y, -30.0);
        assert_eq!(
            runner.state().last_local,
            (5.0, 8.0),
            "the handler saw the cursor-local point"
        );

        // A second notch accumulates — no leftover capture state between notches.
        runner.dispatch_wheel(
            scroller,
            WheelEvent::new((0.0, -12.0), (5.0, 8.0), (100.0, 50.0)),
        );
        assert_eq!(runner.state().notches, 2);
        assert_eq!(
            runner.state().total_y,
            -42.0,
            "successive notches accumulate"
        );

        // A notch whose ancestor chain has no handler (the document, above the
        // scroller) resolves nothing and mutates nothing.
        let doc = dom.borrow().document();
        assert_eq!(
            runner.wheel_target(doc),
            None,
            "no handler above the scroller"
        );
        runner.dispatch_wheel(doc, WheelEvent::new((0.0, -99.0), (0.0, 0.0), (0.0, 0.0)));
        assert_eq!(runner.state().notches, 2, "a handler-less notch is a no-op");
        assert_eq!(runner.state().total_y, -42.0);
    }

    /// Wheel default-blocking guard: a native wheel handler can cancel the host
    /// scroll default for one notch, and the next unprevented notch resets the
    /// runner flag. This mirrors the JS passive-wheel guard in script-runtime.
    #[test]
    fn wheel_prevent_default_blocks_one_scroll_notch() {
        #[derive(Default)]
        struct Scroll {
            prevent_next: bool,
            notches: u32,
        }
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |_: &Scroll| {
                on_wheel(
                    el::<_, Scroll, ()>("div", "scroller"),
                    |s: &mut Scroll, e: WheelEvent| {
                        s.notches += 1;
                        if s.prevent_next {
                            e.prevent_default();
                            s.prevent_next = false;
                        }
                    },
                )
            },
            Scroll {
                prevent_next: true,
                notches: 0,
            },
        );
        let scroller = runner.root();

        runner.dispatch_wheel(
            scroller,
            WheelEvent::new((0.0, -30.0), (5.0, 8.0), (100.0, 50.0)),
        );
        assert!(
            runner.default_prevented(),
            "prevented notch blocks host scroll default"
        );

        runner.dispatch_wheel(
            scroller,
            WheelEvent::new((0.0, -12.0), (5.0, 8.0), (100.0, 50.0)),
        );
        assert!(
            !runner.default_prevented(),
            "the next notch records its own unprevented default state"
        );
        assert_eq!(runner.state().notches, 2);
    }

    /// `slider`: a press sets the value to the pointer fraction, and a drag
    /// (move while captured) tracks it.
    #[test]
    fn slider_drag_sets_value() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &Slider| slider(s),
            Slider::default(),
        );
        let track = runner.root();

        // Press at 50/100 → value 0.5.
        runner.dispatch_pointer_down(
            track,
            PointerEvent::new(PointerPhase::Down, (50.0, 0.0), (100.0, 12.0)),
        );
        assert!((runner.state().value - 0.5).abs() < 0.001, "press sets 0.5");

        // Drag to 80/100 → value 0.8.
        runner.dispatch_pointer_move(PointerEvent::new(
            PointerPhase::Move,
            (80.0, 0.0),
            (100.0, 12.0),
        ));
        assert!(
            (runner.state().value - 0.8).abs() < 0.001,
            "drag tracks to 0.8"
        );

        // Past the right edge clamps to 1.0.
        runner.dispatch_pointer_move(PointerEvent::new(
            PointerPhase::Move,
            (130.0, 0.0),
            (100.0, 12.0),
        ));
        assert!((runner.state().value - 1.0).abs() < 0.001, "clamps to 1.0");
        runner.dispatch_pointer_up(PointerEvent::new(
            PointerPhase::Up,
            (130.0, 0.0),
            (100.0, 12.0),
        ));
        assert_eq!(runner.pointer_capture(), None, "release ends the drag");
    }

    /// IME preedit (T2): the composing text renders spliced at the caret but
    /// stays out of the committed buffer, and the caret sits after it.
    #[test]
    fn textinput_preedit_renders_inline() {
        use crate::TextInput;
        let mut t = TextInput::new("ab"); // caret at end (2)
        t.move_left(false); // caret between 'a' and 'b' (1)
        t.set_preedit("XY");
        assert_eq!(t.render_text(), "aXYb", "preedit spliced at the caret");
        assert_eq!(t.text(), "ab", "buffer stays clean (preedit not committed)");
        // In "aXYb", the caret sits after "XY": byte_of(1)=1 + preedit "XY"=2 → 3.
        assert_eq!(t.caret_byte_in_render(), 3);

        t.clear_preedit();
        assert_eq!(t.render_text(), "ab", "cleared preedit → plain buffer");
        assert_eq!(t.caret_byte_in_render(), 1);
    }

    /// Ghost autocomplete: the suffix is shown but stays out of the committed
    /// buffer and the caret geometry, so submitting evaluates only the typed text.
    /// `accept_ghost` (the host's → / Tab) is the only path that commits it.
    #[test]
    fn textinput_ghost_is_uncommitted_until_accepted() {
        use crate::TextInput;
        let mut t = TextInput::new(">ros"); // caret at end (4)
        t.set_ghost("ter");
        // The ghost is visible via ghost(), but never via the buffer or render.
        assert_eq!(t.ghost(), "ter");
        assert_eq!(t.text(), ">ros", "ghost is not in the committed buffer");
        assert_eq!(
            t.render_text(),
            ">ros",
            "ghost is not in the rendered (caret) text"
        );
        assert_eq!(
            t.caret_byte_in_render(),
            4,
            "the caret sits before the ghost"
        );

        // Accepting splices it in, moves the caret to the end, and clears it.
        t.accept_ghost();
        assert_eq!(t.text(), ">roster");
        assert_eq!(t.ghost(), "", "ghost cleared after accept");
        assert_eq!(t.caret(), 7, "caret moved to the new end");

        // Accepting with no ghost is a no-op.
        t.accept_ghost();
        assert_eq!(t.text(), ">roster");
    }

    /// Ctrl / Cmd + A selects the whole buffer (anchor at start, caret at end).
    #[test]
    fn textinput_select_all_covers_the_buffer() {
        use crate::TextInput;
        let mut t = TextInput::new("hello");
        t.move_left(false); // a collapsed caret, no selection
        assert!(!t.has_selection());
        t.select_all();
        assert!(t.has_selection());
        assert_eq!(t.selection(), (0, 5));
        assert_eq!(t.selected_text(), "hello");
    }

    // --- selection (model + keyboard) -----------------------------------------

    /// A `Shift`-held key event (extends the selection).
    fn shift_named(k: NamedKey) -> KeyEvent {
        KeyEvent::with_mods(
            Key::Named(k),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        )
    }

    /// `TextInput` selection: extending, replacing, deleting, and collapsing —
    /// exercised directly on the model.
    #[test]
    fn text_input_selection_model() {
        // Select all (Home, then Shift+End) and replace it.
        let mut t = TextInput::new("hello");
        assert!(!t.has_selection());
        assert_eq!(t.selected_text(), "", "no selection → empty");
        t.home(false);
        t.end(true);
        assert!(t.has_selection());
        assert_eq!(t.selection(), (0, 5));
        assert_eq!(t.selected_text(), "hello"); // copy/cut source
        t.insert_str("X"); // replaces the whole selection
        assert_eq!(t.text(), "X");
        assert_eq!(t.caret(), 1);
        assert!(!t.has_selection());

        // Select "bc" in "abcd" (Shift+→ from index 1) and backspace it away.
        let mut t = TextInput::new("abcd");
        t.home(false);
        t.move_right(false); // caret 1
        t.move_right(true); // sel 1..2
        t.move_right(true); // sel 1..3 = "bc"
        assert_eq!(t.selection(), (1, 3));
        assert_eq!(t.selected_text(), "bc");
        t.backspace();
        assert_eq!(t.text(), "ad");
        assert_eq!(t.caret(), 1);
        assert!(!t.has_selection());

        // Shift+← extends; a plain ← collapses to the selection's left edge.
        let mut t = TextInput::new("abc");
        t.move_left(true);
        t.move_left(true); // sel 1..3
        assert_eq!(t.selection(), (1, 3));
        t.move_left(false); // collapse to the left edge, no extra move
        assert!(!t.has_selection());
        assert_eq!(t.caret(), 1);
    }

    /// Keyboard selection through the field: type, `Shift+←` to select, then type
    /// to replace — proving the edit handler reads `ev.mods.shift`.
    #[test]
    fn text_field_keyboard_selection_replaces() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            |s: &TextInput| text_field(s),
            TextInput::default(),
        );
        let input = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "input").expect("an <input>")
        };
        runner.set_focus(Some(input));

        runner.key(ch("a"));
        runner.key(ch("b"));
        runner.key(ch("c")); // "abc", caret 3
        runner.key(shift_named(NamedKey::ArrowLeft)); // sel 2..3
        runner.key(shift_named(NamedKey::ArrowLeft)); // sel 1..3 = "bc"
        assert!(runner.state().has_selection());
        assert_eq!(runner.state().selection(), (1, 3));

        runner.key(ch("X")); // replaces the selection
        assert_eq!(runner.state().text(), "aX");
        assert!(!runner.state().has_selection());
        assert_eq!(
            field_text(&dom.borrow(), runner.root()),
            "aX",
            "DOM text is the clean buffer after the replace"
        );
    }
}

// --- MARK: Stage 3 — capture phase (backend-only) -----------------------------
//
// The headless twin of `pelt-live`'s capture-phase suite: per-listener phase
// (`.capture(true)` vs the default bubble), and the dispatch order it produces
// (capture → target → bubble). Each handler appends a label to a shared
// `Vec<String>` log on the app state, so the assertions read the literal firing
// order — proven over the `ScriptedDom` with no serval-layout/netrender.

#[cfg(test)]
mod capture {
    use std::cell::RefCell;
    use std::rc::Rc;

    use layout_dom_api::{LayoutDom, NodeKind};
    use serval_scripted_dom::{NodeId, ScriptedDom};

    use crate::{
        DomHandle, Key, KeyEvent, PointerClick, ServalAppRunner, ServalCtx, ServalElement, View,
        el, on_click, on_key,
    };

    /// The app state: an ordered firing log every handler appends to.
    struct Log {
        events: Vec<String>,
    }

    /// The first element in `node`'s subtree (pre-order) named `name`.
    fn find_element_by_name(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom
                .element_name(node)
                .is_some_and(|q| q.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|c| find_element_by_name(dom, c, name))
    }

    fn ch(s: &str) -> KeyEvent {
        KeyEvent::new(Key::Character(s.to_string()))
    }

    // --- click ordering: capture parent before bubble child -------------------

    /// `<div on_click(capture) ><button on_click /></div>`: a capture-phase
    /// handler on the parent div, a default (bubble) handler on the child button.
    /// Each logs its label. Clicking the button must fire the capture parent
    /// *before* the bubble child.
    fn click_phase_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>(
                "div",
                on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
                    s.events.push("bubble-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-parent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn click_capture_parent_fires_before_bubble_child() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_phase_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec!["capture-parent".to_string(), "bubble-child".to_string()],
            "capture ancestor fires before the bubble target (capture → target → bubble)"
        );
    }

    // --- stacked listeners: a node carries several of one kind ----------------

    /// `on_click(on_click(el("button"), inner), outer)`: two click listeners stack
    /// on one element. Both must fire — a second `on_click` no longer silently
    /// clobbers the first (the Vec-per-node registry, G2.3). Within the shared
    /// bubble phase they fire in registration order, and `build` registers the
    /// inner before the outer.
    fn stacked_click_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
                s.events.push("inner".to_string());
            }),
            |s: &mut Log, _ev| s.events.push("outer".to_string()),
        )
    }

    #[test]
    fn stacked_click_listeners_all_fire() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            stacked_click_view,
            Log { events: Vec::new() },
        );
        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, runner.root(), "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec!["inner".to_string(), "outer".to_string()],
            "both stacked click listeners fire in registration order; neither clobbers the other",
        );
    }

    // --- key ordering: same, via dispatch_key ---------------------------------

    /// `<div on_key(capture)><span on_key /></div>`: a capture-phase key handler
    /// on the div, a default (bubble) key handler on the span. Focus the span and
    /// dispatch a key — the capture div must fire before the bubble span.
    fn key_phase_view(_s: &Log) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_key(
            el::<_, Log, ()>(
                "div",
                on_key(el::<_, Log, ()>("span", "x"), |s: &mut Log, _ev| {
                    s.events.push("bubble-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-parent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn key_capture_parent_fires_before_bubble_child() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            key_phase_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let span = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "span").expect("a <span>")
        };

        // The span carries a (bubble) key handler, so it is focusable.
        runner.set_focus(Some(span));
        runner.dispatch_key(ch("a"));

        assert_eq!(
            runner.state().events,
            vec!["capture-parent".to_string(), "bubble-child".to_string()],
            "capture key ancestor fires before the bubble focused node"
        );
    }

    // --- default is bubble ----------------------------------------------------

    /// A default `on_click` (no `.capture()`) on the parent, a `.capture(true)`
    /// handler on... the same parent? No — to show the default sits in the bubble
    /// pass *after* a capture ancestor, the capture handler is on the (outer)
    /// grandparent and the default on the (inner) child. Click the child: the
    /// capture grandparent fires first, the default child last, proving the
    /// no-`.capture()` listener only runs in the bubble pass.
    fn default_is_bubble_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>(
                "section",
                on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
                    // No `.capture()` → default bubble. Must fire AFTER the
                    // capture grandparent, never before it.
                    s.events.push("default-child".to_string());
                }),
            ),
            |s: &mut Log, _ev| s.events.push("capture-grandparent".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn default_on_click_only_fires_in_bubble_pass() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            default_is_bubble_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec![
                "capture-grandparent".to_string(),
                "default-child".to_string()
            ],
            "a default (no `.capture()`) listener fires only in the bubble pass, \
             after the capture ancestor"
        );
    }

    // --- capture-only ancestor fires on a descendant click --------------------

    /// `<div on_click(capture)><button /></div>`: only the div carries a handler,
    /// and it is capture-phase. The button has no handler. Clicking the button (a
    /// descendant) must still fire the capture ancestor.
    fn capture_only_ancestor_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>("div", el::<_, Log, ()>("button", "+")),
            |s: &mut Log, _ev| s.events.push("capture-ancestor".to_string()),
        )
        .capture(true)
    }

    #[test]
    fn capture_only_ancestor_fires_on_descendant_click() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            capture_only_ancestor_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();

        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };
        assert_ne!(button, root, "button is a descendant of the capture div");

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec!["capture-ancestor".to_string()],
            "a capture-only ancestor fires when a handler-less descendant is clicked"
        );
    }

    // --- a node's listener fires in exactly one phase -------------------------

    /// A single capture handler on a node that is itself the click target fires
    /// exactly once (in the capture pass), never twice — the target-node listener
    /// is in whichever single phase it registered.
    fn single_capture_target_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(el::<_, Log, ()>("button", "+"), |s: &mut Log, _ev| {
            s.events.push("fired".to_string());
        })
        .capture(true)
    }

    #[test]
    fn target_listener_fires_in_exactly_one_phase() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            single_capture_target_view,
            Log { events: Vec::new() },
        );
        let button = runner.root();

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec!["fired".to_string()],
            "a capture listener on the target itself fires once, not in both passes"
        );
    }

    // --- MARK: native cancellation (stopPropagation / preventDefault) ---------
    //
    // The native twin of dom.rs's `__stop` / `__canceled`. These mirror the JS
    // dispatcher's behaviour so the two paths satisfy one contract
    // (docs/2026-06-01_event_model_convergence_plan.md). The JS-side equivalents
    // live in `script-runtime-api`'s `dom_node_events_work` (run on Boa + Nova);
    // the assertions here are the native column of the same scenario table.

    /// `<div on_click><button on_click(stops) /></div>`: the child (target,
    /// bubble) calls `stop_propagation`, so the parent's bubble handler must NOT
    /// fire — the native counterpart of the JS `stop` scenario.
    fn click_stop_view(_s: &Log) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>(
                "div",
                on_click(
                    el::<_, Log, ()>("button", "+"),
                    |s: &mut Log, ev: PointerClick| {
                        s.events.push("child".to_string());
                        ev.stop_propagation();
                    },
                ),
            ),
            |s: &mut Log, _ev| s.events.push("parent-SHOULD-NOT".to_string()),
        )
    }

    #[test]
    fn stop_propagation_halts_the_bubble_walk() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_stop_view,
            Log { events: Vec::new() },
        );
        let root = runner.root();
        let button = {
            let d = dom.borrow();
            find_element_by_name(&d, root, "button").expect("a <button>")
        };

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));

        assert_eq!(
            runner.state().events,
            vec!["child".to_string()],
            "stop_propagation on the target halts the bubble to the parent"
        );
    }

    /// A handler that calls `prevent_default`; the caller reads the shared
    /// Propagation cell *after* dispatch (cloning the handle before the event is
    /// moved in) and sees the cancellation — the seam the host uses to gate a
    /// default action (form activation, drag start).
    fn click_prevent_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>("button", "+"),
            |s: &mut Log, ev: PointerClick| {
                s.events.push("fired".to_string());
                ev.prevent_default();
            },
        )
    }

    #[test]
    fn prevent_default_is_visible_to_the_caller() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_prevent_view,
            Log { events: Vec::new() },
        );
        let button = runner.root();

        // Build the event and keep a clone of its propagation handle; the event
        // is moved into dispatch but the handle shares the same cell, so the
        // host reads the handler's cancellation back here.
        let ev = PointerClick::at((0.0, 0.0));
        let prop = ev.prop.clone();
        assert!(!prop.default_prevented(), "not canceled before dispatch");

        runner.dispatch_click(button, ev);

        assert_eq!(runner.state().events, vec!["fired".to_string()]);
        assert!(
            prop.default_prevented(),
            "the handler's prevent_default is visible through the shared cell after dispatch"
        );
    }

    /// A click handler that fires but does NOT prevent the default.
    fn click_plain_view(
        _s: &Log,
    ) -> impl View<Log, (), ServalCtx, Element = ServalElement> + use<> {
        on_click(
            el::<_, Log, ()>("button", "+"),
            |s: &mut Log, _ev: PointerClick| {
                s.events.push("fired".to_string());
            },
        )
    }

    /// The host-friendly half of the contract: after dispatch, the *runner*
    /// reports `default_prevented()` directly, so a host gates its default action
    /// without having to clone the Propagation handle before dispatch.
    #[test]
    fn runner_reports_default_prevented_after_dispatch() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_prevent_view,
            Log { events: Vec::new() },
        );
        let button = runner.root();
        assert!(!runner.default_prevented(), "false before any dispatch");

        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));
        assert_eq!(runner.state().events, vec!["fired".to_string()]);
        assert!(
            runner.default_prevented(),
            "the handler's prevent_default is visible via the runner accessor"
        );
    }

    /// A handler that does not prevent leaves `default_prevented()` false, so the
    /// host proceeds with its default action; a fresh dispatch resets the flag.
    #[test]
    fn runner_default_prevented_is_false_without_prevent() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = ServalAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            click_plain_view,
            Log { events: Vec::new() },
        );
        let button = runner.root();
        runner.dispatch_click(button, PointerClick::at((0.0, 0.0)));
        assert_eq!(
            runner.state().events,
            vec!["fired".to_string()],
            "handler ran"
        );
        assert!(
            !runner.default_prevented(),
            "no prevent_default → the host's default action proceeds"
        );
    }
}
