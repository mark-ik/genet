/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Frisket: a pane frame — splits and tab-stacks — as one composition.
//!
//! On a hand press the *frisket* is the hinged frame whose cut-out apertures
//! decide what prints where; this is the same frame, over a window. The name
//! comes from turnstone's crate, which this module retires: the family had spelled
//! "a tree of resizable panes" four times (this view trapped in `ports/pelt`, a
//! second tree in turnstone, the contract in `genet-host-api`, the furniture here),
//! and they resolve to one implementation. Direction:
//! `genet:docs/2026-07-24_frisket_pane_component_direction.md`.
//!
//! **The tree is the caller's state**, like the split's ratio and the strip's
//! selection: [`frisket`] renders whatever tree the caller hands it and reports
//! gestures as [`TileEvent`]s for the caller to apply through
//! [`TileTree::apply`]. The component never mutates the tree, so one host can
//! hold a plain tree while another projects one from richer truth (mere's platen
//! projects a forme arrangement onto it) and both drive the same view.
//!
//! **No layout dependency, by design.** The parts here are the view and the DOM
//! *semantics*: which element is a divider, which is a tab, which tab bar
//! belongs to which stack. A host lays the DOM out and hit-tests it, then asks
//! [`divider_target`] / [`tab_target`] / [`stack_target`] what it hit. That
//! keeps the walking logic in one place — it was duplicated per host before —
//! while layout stays where the host's engine is. The one gesture that needs
//! real geometry, resolving a tab drop to an insertion index, takes the rects
//! through a closure ([`tab_drop_index`]).
//!
//! Content is a hole. The component draws a placeholder marked with the active
//! tile's id and nothing else; the host composites a document scene, an
//! external texture, or its own surface into that rect. So a browser pane, a
//! practice-set pane, and a tactical map pane are the same frame.

use genet_host_api::tile::{SplitAxis, TabStack, TileEvent, TileId, TilePath, TileTree};
use layout_dom_api::{LayoutDom, LocalName, Namespace};

use crate::pod::GenetElement;
use crate::{AnyView, GenetCtx, PointerClick, View, el, on_click};

/// The erased child-view type a pane frame nests: a split holds splits or
/// stacks, so the recursion needs one boxed type.
pub type PaneView<State, AppAction> = Box<dyn AnyView<State, AppAction, GenetCtx, GenetElement>>;

/// `data-divider`: the split path a divider resizes.
const ATTR_DIVIDER: &str = "data-divider";
/// `data-dindex`: which boundary within that split.
const ATTR_DINDEX: &str = "data-dindex";
/// `data-tabid`: the tile a tab represents.
const ATTR_TABID: &str = "data-tabid";
/// `data-stack`: the path of the stack a tab bar belongs to.
const ATTR_STACK: &str = "data-stack";
/// `data-tile`: the active tile whose content area this is. A host finds this
/// element's rect to composite the tile's content into it.
pub const FRISKET_TILE_ATTR: &str = "data-tile";

/// The default structural stylesheet: flex skeleton, tab bar, divider, and the
/// content hole. A theme layers over it; nothing here is decorative beyond the
/// minimum needed to be usable unstyled.
pub const FRISKET_CSS: &str = "\
    .frisket-split { width: 100%; height: 100%; } \
    .frisket-branch { display: flex; } \
    .frisket-stack { width: 100%; height: 100%; } \
    .frisket-tabbar { display: flex; align-items: stretch; height: 44px; padding: 4px 2px 0 2px; background: #33333a; } \
    .frisket-tab { display: flex; align-items: center; min-width: 0; padding: 8px 14px; font-size: 15px; line-height: 1.2; color: #cccccc; background: #2a2a30; margin-right: 3px; } \
    .frisket-tab.active { color: #ffffff; background: #4a4a55; } \
    .frisket-label { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; } \
    .frisket-close { flex: 0 0 auto; margin-left: 10px; padding: 0 5px; font-size: 15px; color: #999999; } \
    .frisket-content { flex: 1 1 0; min-height: 0; background: #ffffff; } \
    .frisket-divider { flex: 0 0 10px; background: #1a1a1f; }";

/// Encode a split path (`[0, 1]`) as an attribute string (`"0.1"`); the root
/// split is the empty string.
pub fn encode_pane_path(path: &[usize]) -> String {
    path.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Decode a `data-divider` / `data-stack` value back to a [`TilePath`].
pub fn decode_pane_path(s: &str) -> TilePath {
    if s.is_empty() {
        TilePath(Vec::new())
    } else {
        TilePath(s.split('.').filter_map(|p| p.parse().ok()).collect())
    }
}

/// Render `tree` as a pane frame. `on_event` receives every gesture the frame
/// raises (a tab activated, a tab closed); the caller applies it to its own
/// authoritative tree.
///
/// The returned view fills its parent, so a host wraps it in whatever frame it
/// wants — a status bar under it, chrome above it.
pub fn frisket<State, AppAction, Ev>(
    tree: &TileTree,
    on_event: Ev,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Ev: Fn(&mut State, TileEvent) + Clone + 'static,
{
    el::<_, State, AppAction>("div", render_node(tree, &[], &on_event))
        .attr("class", "frisket-body")
        .attr(
            "style",
            "display: flex; width: 100%; height: 100%; min-height: 0;",
        )
}

fn render_node<State, AppAction, Ev>(
    node: &TileTree,
    path: &[usize],
    on_event: &Ev,
) -> PaneView<State, AppAction>
where
    State: 'static,
    AppAction: 'static,
    Ev: Fn(&mut State, TileEvent) + Clone + 'static,
{
    match node {
        TileTree::Split { axis, children } => {
            let dir = match axis {
                SplitAxis::Row => "row",
                SplitAxis::Column => "column",
            };
            let path_attr = encode_pane_path(path);
            // A draggable divider between adjacent children, each carrying its
            // split's path and boundary index so a host can resolve a drag to a
            // `DividerMoved` without keeping its own map.
            let mut items: Vec<PaneView<State, AppAction>> = Vec::new();
            for (index, branch) in children.iter().enumerate() {
                let mut child_path = path.to_vec();
                child_path.push(index);
                let inner = render_node(&branch.tree, &child_path, on_event);
                items.push(Box::new(
                    el::<_, State, AppAction>("div", inner)
                        .attr("class", "frisket-branch")
                        .attr(
                            "style",
                            format!(
                                "flex: {frac} {frac} 0; min-width: 0; min-height: 0;",
                                frac = branch.fraction
                            ),
                        ),
                ));
                if index + 1 < children.len() {
                    items.push(Box::new(
                        el::<_, State, AppAction>("div", ())
                            .attr("class", "frisket-divider")
                            // An ARIA separator so the divider is reachable and
                            // announced, not just draggable.
                            .attr("role", "separator")
                            .attr(
                                "aria-orientation",
                                match axis {
                                    SplitAxis::Row => "vertical",
                                    SplitAxis::Column => "horizontal",
                                },
                            )
                            .attr(ATTR_DIVIDER, path_attr.clone())
                            .attr(ATTR_DINDEX, index.to_string()),
                    ));
                }
            }
            Box::new(
                el::<_, State, AppAction>("div", items)
                    .attr("class", "frisket-split")
                    .attr("style", format!("display: flex; flex-direction: {dir};")),
            )
        },
        TileTree::Stack(stack) => render_stack(stack, path, on_event),
    }
}

fn render_stack<State, AppAction, Ev>(
    stack: &TabStack,
    path: &[usize],
    on_event: &Ev,
) -> PaneView<State, AppAction>
where
    State: 'static,
    AppAction: 'static,
    Ev: Fn(&mut State, TileEvent) + Clone + 'static,
{
    let tabs: Vec<PaneView<State, AppAction>> = stack
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tile)| {
            let id = tile.id;
            let active = index == stack.active;
            let label = el::<_, State, AppAction>("span", tile.title.clone())
                .attr("class", "frisket-label");
            // The × is its own control, announced as "Close <title>", and it
            // stops propagation so closing does not also activate.
            let close_event = on_event.clone();
            let close = on_click(
                el::<_, State, AppAction>("span", "\u{00d7}")
                    .attr("class", "frisket-close")
                    .attr("role", "button")
                    .attr("aria-label", format!("Close {}", tile.title)),
                move |state: &mut State, ev: PointerClick| {
                    ev.stop_propagation();
                    close_event(state, TileEvent::Closed(id));
                },
            );
            // A host accent paints the tab inline, so a product can tint a tab
            // to match its content without forking the component's CSS.
            let style = match tile.accent {
                Some(accent) => format!(
                    "background-color: rgb({}, {}, {}); color: rgb({}, {}, {});",
                    accent.background[0],
                    accent.background[1],
                    accent.background[2],
                    accent.foreground[0],
                    accent.foreground[1],
                    accent.foreground[2],
                ),
                None => String::new(),
            };
            let activate = on_event.clone();
            Box::new(on_click(
                el::<_, State, AppAction>("div", (label, close))
                    .attr(
                        "class",
                        if active {
                            "frisket-tab active"
                        } else {
                            "frisket-tab"
                        },
                    )
                    .attr(ATTR_TABID, id.0.to_string())
                    // The title lives in a child span, so the tab names itself
                    // with aria-label; selection is state, not a class name.
                    .attr("role", "tab")
                    .attr("aria-label", tile.title.clone())
                    .attr("aria-selected", if active { "true" } else { "false" })
                    .attr("style", style),
                move |state: &mut State, _: PointerClick| activate(state, TileEvent::Activated(id)),
            )) as PaneView<State, AppAction>
        })
        .collect();

    // The bar carries its stack's path, so a tab dropped on it resolves to
    // `DropTarget::Stack` (merge into this stack) rather than an edge split.
    let tab_bar = el::<_, State, AppAction>("div", tabs)
        .attr("class", "frisket-tabbar")
        .attr("role", "tablist")
        .attr(ATTR_STACK, encode_pane_path(path));

    // The content hole: marked with the active tile's id and otherwise empty.
    // What fills it is the host's business, which is what lets one frame serve
    // a document, an external texture, or an app's own surface.
    let active = stack.tabs.get(stack.active).map(|t| t.id.0).unwrap_or(0);
    let content = el::<_, State, AppAction>("div", ())
        .attr("class", "frisket-content")
        .attr(FRISKET_TILE_ATTR, active.to_string());

    Box::new(
        el::<_, State, AppAction>("div", (tab_bar, content))
            .attr("class", "frisket-stack")
            .attr("style", "display: flex; flex-direction: column;"),
    )
}

/// What a divider press resolves to: the split it resizes and which boundary.
/// The host supplies the split's pixel extent from its own layout to turn a drag
/// delta into a fraction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DividerTarget {
    pub path: TilePath,
    pub index: usize,
}

fn attr<D: LayoutDom>(dom: &D, node: D::NodeId, name: &str) -> Option<String> {
    dom.attribute(node, &Namespace::default(), &LocalName::from(name))
        .map(|value| value.to_string())
}

fn has_class<D: LayoutDom>(dom: &D, node: D::NodeId, class: &str) -> bool {
    attr(dom, node, "class")
        .is_some_and(|value| value.split_whitespace().any(|token| token == class))
}

/// Walk up from a hit node to the divider it belongs to, if any.
///
/// The host hit-tests its laid-out DOM and hands the node here rather than
/// re-implementing the walk; this is the duplication the module exists to end.
pub fn divider_target<D: LayoutDom>(dom: &D, hit: D::NodeId) -> Option<DividerTarget> {
    let mut node = hit;
    loop {
        if let Some(path) = attr(dom, node, ATTR_DIVIDER) {
            return Some(DividerTarget {
                path: decode_pane_path(&path),
                index: attr(dom, node, ATTR_DINDEX)?.parse().ok()?,
            });
        }
        node = dom.parent(node)?;
    }
}

/// Walk up from a hit node to the tab it belongs to. A press on the close ×
/// resolves to `None`, because that press is a close and not the start of a tab
/// drag.
pub fn tab_target<D: LayoutDom>(dom: &D, hit: D::NodeId) -> Option<TileId> {
    if has_class(dom, hit, "frisket-close") {
        return None;
    }
    let mut node = hit;
    loop {
        if let Some(id) = attr(dom, node, ATTR_TABID).and_then(|s| s.parse::<u64>().ok()) {
            return Some(TileId(id));
        }
        node = dom.parent(node)?;
    }
}

/// Walk up from a hit node to the stack whose tab bar it is in.
pub fn stack_target<D: LayoutDom>(dom: &D, hit: D::NodeId) -> Option<TilePath> {
    let mut node = hit;
    loop {
        if let Some(path) = attr(dom, node, ATTR_STACK) {
            return Some(decode_pane_path(&path));
        }
        node = dom.parent(node)?;
    }
}

/// Where a tab dropped at `x` would land in the tab bar under `hit`: the stack's
/// path and the insertion index, counting the tabs whose horizontal centre sits
/// left of the cursor.
///
/// The one gesture that needs real geometry, so the rects arrive through
/// `rect_of` (`(x, y, w, h)`, host space) instead of this module growing a
/// layout dependency.
pub fn tab_drop_index<D: LayoutDom>(
    dom: &D,
    hit: D::NodeId,
    x: f32,
    rect_of: impl Fn(D::NodeId) -> Option<(f32, f32, f32, f32)>,
) -> Option<(TilePath, usize)> {
    let mut node = hit;
    let bar = loop {
        if attr(dom, node, ATTR_STACK).is_some() {
            break node;
        }
        node = dom.parent(node)?;
    };
    let path = decode_pane_path(&attr(dom, bar, ATTR_STACK)?);
    let mut before = 0usize;
    let mut stack = vec![bar];
    while let Some(current) = stack.pop() {
        if attr(dom, current, ATTR_TABID).is_some() {
            if let Some((rx, _, rw, _)) = rect_of(current) {
                if rx + rw / 2.0 < x {
                    before += 1;
                }
            }
        }
        for child in dom.dom_children(current) {
            stack.push(child);
        }
    }
    Some((path, before))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use genet_host_api::tile::{ContentSource, DocumentRef, Tile, TileBranch};
    use genet_scripted_dom::{NodeId, ScriptedDom};

    use super::*;
    use crate::{DomHandle, GenetAppRunner};

    type TestView = Box<dyn AnyView<State, (), GenetCtx, GenetElement>>;

    struct State {
        tree: TileTree,
        events: Vec<TileEvent>,
    }

    fn tile(id: u64, title: &str) -> Tile {
        Tile {
            id: TileId(id),
            title: title.to_string(),
            content: ContentSource::Document(DocumentRef(format!("doc:{id}"))),
            accent: None,
        }
    }

    /// A row split of two stacks; the left one holds two tabs, second active.
    fn sample() -> TileTree {
        TileTree::Split {
            axis: SplitAxis::Row,
            children: vec![
                TileBranch {
                    fraction: 0.7,
                    tree: TileTree::Stack(TabStack {
                        tabs: vec![tile(1, "First"), tile(2, "Second")],
                        active: 1,
                    }),
                },
                TileBranch {
                    fraction: 0.3,
                    tree: TileTree::Stack(TabStack {
                        tabs: vec![tile(3, "Third")],
                        active: 0,
                    }),
                },
            ],
        }
    }

    fn view(state: &State) -> TestView {
        Box::new(frisket(&state.tree, |state: &mut State, event| {
            state.events.push(event)
        }))
    }

    fn harness() -> (
        DomHandle,
        GenetAppRunner<State, fn(&State) -> TestView, TestView, ()>,
    ) {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = State {
            tree: sample(),
            events: Vec::new(),
        };
        let runner = GenetAppRunner::new(dom.clone(), view as fn(&State) -> TestView, state);
        (dom, runner)
    }

    fn attr_of<'a>(dom: &'a ScriptedDom, node: NodeId, name: &str) -> Option<&'a str> {
        dom.attribute(node, &Namespace::default(), &LocalName::from(name))
    }

    fn find_attr(dom: &ScriptedDom, root: NodeId, name: &str, value: &str) -> Option<NodeId> {
        if attr_of(dom, root, name) == Some(value) {
            return Some(root);
        }
        dom.dom_children(root)
            .find_map(|child| find_attr(dom, child, name, value))
    }

    fn find_class(dom: &ScriptedDom, root: NodeId, class: &str) -> Option<NodeId> {
        if has_class(dom, root, class) {
            return Some(root);
        }
        dom.dom_children(root)
            .find_map(|child| find_class(dom, child, class))
    }

    fn count_class(dom: &ScriptedDom, root: NodeId, class: &str) -> usize {
        usize::from(has_class(dom, root, class))
            + dom
                .dom_children(root)
                .map(|child| count_class(dom, child, class))
                .sum::<usize>()
    }

    #[test]
    fn a_pane_path_round_trips() {
        for path in [vec![], vec![0], vec![1, 0, 2]] {
            let encoded = encode_pane_path(&path);
            assert_eq!(decode_pane_path(&encoded), TilePath(path));
        }
    }

    #[test]
    fn the_frame_draws_splits_dividers_stacks_and_one_content_hole_per_stack() {
        let (dom, runner) = harness();
        let root = runner.root();
        let dom = dom.borrow();

        assert_eq!(count_class(&dom, root, "frisket-stack"), 2);
        // N children means N-1 dividers, never one per child.
        assert_eq!(count_class(&dom, root, "frisket-divider"), 1);
        assert_eq!(count_class(&dom, root, "frisket-tabbar"), 2);
        assert_eq!(count_class(&dom, root, "frisket-content"), 2);
        assert_eq!(count_class(&dom, root, "frisket-tab"), 3);

        // The content hole names the ACTIVE tile, the second tab here.
        let hole = find_attr(&dom, root, FRISKET_TILE_ATTR, "2").expect("active content hole");
        assert!(has_class(&*dom, hole, "frisket-content"));

        // Fractions ride on the branch as flex-grow, so layout sizes the panes.
        let split = find_class(&dom, root, "frisket-split").expect("split");
        let branch = dom
            .dom_children(split)
            .find(|child| has_class(&*dom, *child, "frisket-branch"))
            .expect("first branch");
        assert!(
            attr_of(&dom, branch, "style").is_some_and(|s| s.contains("flex: 0.7 0.7 0")),
            "style {:?}",
            attr_of(&dom, branch, "style")
        );
    }

    #[test]
    fn a_tab_is_an_aria_tab_and_only_the_active_one_is_selected() {
        let (dom, runner) = harness();
        let root = runner.root();
        let dom = dom.borrow();
        let first = find_attr(&dom, root, ATTR_TABID, "1").expect("first tab");
        let second = find_attr(&dom, root, ATTR_TABID, "2").expect("second tab");
        assert_eq!(attr_of(&dom, first, "role"), Some("tab"));
        assert_eq!(attr_of(&dom, first, "aria-selected"), Some("false"));
        assert_eq!(attr_of(&dom, second, "aria-selected"), Some("true"));
        assert_eq!(attr_of(&dom, first, "aria-label"), Some("First"));
        // The divider announces itself as a separator with an orientation.
        let divider = find_class(&dom, root, "frisket-divider").expect("divider");
        assert_eq!(attr_of(&dom, divider, "role"), Some("separator"));
        assert_eq!(attr_of(&dom, divider, "aria-orientation"), Some("vertical"));
    }

    #[test]
    fn gestures_report_events_and_never_mutate_the_tree() {
        let (dom, mut runner) = harness();
        let root = runner.root();
        let before = runner.state().tree.clone();

        let first = find_attr(&dom.borrow(), root, ATTR_TABID, "1").expect("first tab");
        runner.dispatch_click(first, PointerClick::at((2.0, 2.0)));
        assert_eq!(runner.state().events, [TileEvent::Activated(TileId(1))]);

        // The close x reports a close and stops propagation, so the tab's own
        // activate handler must not also fire.
        let close = find_attr(&dom.borrow(), root, "aria-label", "Close Second").expect("close");
        runner.dispatch_click(close, PointerClick::at((2.0, 2.0)));
        assert_eq!(
            runner.state().events,
            [
                TileEvent::Activated(TileId(1)),
                TileEvent::Closed(TileId(2)),
            ],
            "the close does not also activate its tab"
        );

        assert_eq!(
            runner.state().tree,
            before,
            "the component reports gestures; the caller owns the tree"
        );
    }

    #[test]
    fn a_hit_resolves_to_the_divider_it_belongs_to() {
        let (dom, runner) = harness();
        let root = runner.root();
        let dom = dom.borrow();
        let divider = find_class(&dom, root, "frisket-divider").expect("divider");
        assert_eq!(
            divider_target(&*dom, divider),
            Some(DividerTarget {
                path: TilePath(vec![]),
                index: 0,
            }),
            "the root split's first boundary"
        );
        let tab = find_attr(&dom, root, ATTR_TABID, "1").expect("tab");
        assert_eq!(divider_target(&*dom, tab), None);
    }

    #[test]
    fn a_hit_inside_a_tab_resolves_to_its_tile_but_the_close_does_not() {
        let (dom, runner) = harness();
        let root = runner.root();
        let dom = dom.borrow();

        // The label is a child of the tab, so the walk has to climb.
        let tab = find_attr(&dom, root, ATTR_TABID, "3").expect("third tab");
        let label = dom
            .dom_children(tab)
            .find(|child| has_class(&*dom, *child, "frisket-label"))
            .expect("label");
        assert_eq!(tab_target(&*dom, label), Some(TileId(3)));

        let close = find_attr(&dom, root, "aria-label", "Close Third").expect("close");
        assert_eq!(tab_target(&*dom, close), None);
    }

    #[test]
    fn a_tab_bar_hit_resolves_to_its_stack_and_a_drop_index() {
        let (dom, runner) = harness();
        let root = runner.root();
        let dom = dom.borrow();
        let bar = find_class(&dom, root, "frisket-tabbar").expect("first tab bar");
        assert_eq!(stack_target(&*dom, bar), Some(TilePath(vec![0])));

        // Two tabs 100 wide at x=0 and x=100, so centres at 50 and 150. The
        // rects arrive through the closure: no layout engine in this crate.
        let rect_of = |node: NodeId| -> Option<(f32, f32, f32, f32)> {
            match attr_of(&dom, node, ATTR_TABID) {
                Some("1") => Some((0.0, 0.0, 100.0, 40.0)),
                Some("2") => Some((100.0, 0.0, 100.0, 40.0)),
                _ => None,
            }
        };
        assert_eq!(
            tab_drop_index(&*dom, bar, 10.0, rect_of),
            Some((TilePath(vec![0]), 0)),
            "left of both centres inserts first"
        );
        assert_eq!(
            tab_drop_index(&*dom, bar, 120.0, rect_of),
            Some((TilePath(vec![0]), 1))
        );
        assert_eq!(
            tab_drop_index(&*dom, bar, 400.0, rect_of),
            Some((TilePath(vec![0]), 2)),
            "past both centres appends"
        );
    }
}
