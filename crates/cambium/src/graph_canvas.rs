/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A bounded, interactive viewport over Sprigging's shared graph canvas.
//!
//! The graph remains one paint leaf. Each node also gets a small, absolutely
//! positioned native button in the view layer, projected through the same
//! [`GraphViewport`] as the leaf. Paint, hit targets, keyboard focus, and hover
//! therefore stay aligned without teaching a paint leaf about app actions.

use sprigging::{ColorF, GraphCanvas, GraphGlyphNode, GraphViewport, Size};

use crate::{
    GenetCtx, GenetElement, HoverEvent, HoverPhase, PointerClick, View, custom_leaf, el, on_click,
    on_hover,
};

/// Structural classes emitted by [`graph_canvas_swatch`]. Hosts own the palette.
pub const GRAPH_CANVAS_SWATCH_CSS: &str = r#"
.graph-canvas-swatch {
    background-color: rgba(127, 127, 127, 0.06);
    border: 1px solid rgba(127, 127, 127, 0.28);
    border-radius: 7px;
}
.graph-canvas-swatch-node {
    background-color: transparent;
    border: 0;
    border-radius: 999px;
    cursor: pointer;
    padding: 0;
}
.graph-canvas-swatch-node:focus-visible {
    outline: 1px solid currentColor;
    outline-offset: 1px;
}
.graph-canvas-swatch-expand {
    background-color: rgba(127, 127, 127, 0.10);
    border: 0;
    border-radius: 4px;
    cursor: pointer;
    font-size: 10px;
    padding: 2px 5px;
}
"#;

/// One node in the app-facing subgraph contract.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphCanvasNode<Id, Kind> {
    pub id: Id,
    pub kind: Kind,
    /// Normalized `0..1` scene position.
    pub position: (f32, f32),
    /// Accessible name for the node's native hit target.
    pub label: String,
}

/// One app-facing edge. Endpoints that are absent from the subgraph are skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCanvasEdge<Id> {
    pub from: Id,
    pub to: Id,
}

/// The bounded graph data, independent of view state and palette.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphCanvasSubgraph<Id, Kind> {
    pub nodes: Vec<GraphCanvasNode<Id, Kind>>,
    pub edges: Vec<GraphCanvasEdge<Id>>,
}

/// A card-sized graph viewport. The consumer stores this beside app state,
/// rebuilds its leaf with [`GraphCanvasSwatch::paint_leaf`], and renders the
/// matching view with [`graph_canvas_swatch`].
#[derive(Clone, Debug, PartialEq)]
pub struct GraphCanvasSwatch<Id, Kind> {
    pub leaf_key: u64,
    pub graph: GraphCanvasSubgraph<Id, Kind>,
    pub selected: Option<Id>,
    pub focus: Option<Id>,
    pub hovered: Option<Id>,
    pub viewport: GraphViewport,
    pub width: u32,
    pub height: u32,
    pub node_radius: f32,
    pub edge_width: f32,
    pub hit_size: f32,
    pub label: String,
}

impl<Id, Kind> GraphCanvasSwatch<Id, Kind> {
    /// Build a quiet, card-sized viewport. Dimensions remain configurable so a
    /// host can match its panel density without forking the component.
    pub fn new(leaf_key: u64, graph: GraphCanvasSubgraph<Id, Kind>) -> Self {
        Self {
            leaf_key,
            graph,
            selected: None,
            focus: None,
            hovered: None,
            viewport: GraphViewport::default(),
            width: 260,
            height: 128,
            node_radius: 5.0,
            edge_width: 1.0,
            hit_size: 20.0,
            label: "Related graph".to_string(),
        }
    }

    #[must_use]
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

impl<Id: PartialEq, Kind> GraphCanvasSwatch<Id, Kind> {
    fn node_index(&self, id: Option<&Id>) -> Option<u16> {
        let id = id?;
        self.graph
            .nodes
            .iter()
            .position(|node| &node.id == id)
            .and_then(|index| u16::try_from(index).ok())
    }

    /// Build the Sprigging leaf for this viewport. Node kind is kept in the
    /// contract; the caller's palette resolves it to paint, so product-specific
    /// kinds do not leak into Cambium.
    pub fn paint_leaf(&self, color_for_kind: impl Fn(&Kind) -> ColorF) -> GraphCanvas {
        let nodes = self
            .graph
            .nodes
            .iter()
            .map(|node| GraphGlyphNode {
                x: node.position.0,
                y: node.position.1,
                color: color_for_kind(&node.kind),
            })
            .collect();
        let edges = self
            .graph
            .edges
            .iter()
            .filter_map(|edge| {
                let from = self.node_index(Some(&edge.from))?;
                let to = self.node_index(Some(&edge.to))?;
                Some((from, to))
            })
            .collect();
        let mut leaf = GraphCanvas::new(
            nodes,
            edges,
            Size {
                width: self.width as f32,
                height: self.height as f32,
            },
        );
        leaf.node_radius = self.node_radius;
        leaf.edge_width = self.edge_width;
        leaf.set_viewport(self.viewport);
        leaf.set_emphasis(
            self.node_index(self.selected.as_ref()),
            self.node_index(self.focus.as_ref()),
            self.node_index(self.hovered.as_ref()),
        );
        leaf
    }

    /// The exact leaf-local point for every node. Useful to hosts that need a
    /// second overlay beside the built-in hit targets.
    pub fn projected_positions(&self) -> Vec<(&Id, (f32, f32))> {
        let size = Size {
            width: self.width as f32,
            height: self.height as f32,
        };
        let inset = self.node_radius + self.edge_width;
        self.graph
            .nodes
            .iter()
            .map(|node| (&node.id, self.viewport.project(node.position, size, inset)))
            .collect()
    }
}

/// Render a bounded graph canvas with one native node target per painted node.
///
/// `on_node_click` owns navigation or staging. `on_node_hover` normally writes
/// the supplied id into [`GraphCanvasSwatch::hovered`] (and clears it on leave),
/// after which the host refreshes the registered leaf. `on_expand` switches to
/// the app's full-canvas route. The component does not invent those policies.
pub fn graph_canvas_swatch<State, AppAction, Id, Kind, Click, Hover, Expand>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    let positions = swatch.projected_positions();
    let hit_size = swatch.hit_size.max(1.0);
    let targets: Vec<_> = swatch
        .graph
        .nodes
        .iter()
        .zip(positions)
        .enumerate()
        .map(|(index, (node, (_, (x, y))))| {
            let selected = swatch.selected.as_ref() == Some(&node.id);
            let focused = swatch.focus.as_ref() == Some(&node.id);
            let hovered = swatch.hovered.as_ref() == Some(&node.id);
            let mut class = String::from("graph-canvas-swatch-node");
            if selected {
                class.push_str(" selected");
            }
            if focused {
                class.push_str(" focused");
            }
            if hovered {
                class.push_str(" hovered");
            }
            let mut target = el::<_, State, AppAction>("button", ())
                .attr("class", class)
                .attr("type", "button")
                .attr("aria-label", node.label.clone())
                .attr("data-node-index", index.to_string())
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:{}px;top:{}px;width:{hit_size}px;height:{hit_size}px;",
                        x - hit_size / 2.0,
                        y - hit_size / 2.0,
                    ),
                );
            if selected {
                target = target.attr("aria-current", "true");
            }

            let click = on_node_click.clone();
            let click_id = node.id.clone();
            let hover = on_node_hover.clone();
            let enter_id = node.id.clone();
            on_hover(
                on_click(target, move |state: &mut State, _: PointerClick| {
                    click(state, click_id.clone());
                }),
                move |state: &mut State, event: HoverEvent| match event.phase {
                    HoverPhase::Enter => hover(state, Some(enter_id.clone())),
                    HoverPhase::Leave => hover(state, None),
                    HoverPhase::Move => {}
                },
            )
        })
        .collect();

    let expand = on_expand.clone();
    let expand_button = on_click(
        el::<_, State, AppAction>("button", "Expand")
            .attr("class", "graph-canvas-swatch-expand")
            .attr("type", "button")
            .attr("aria-label", "Expand graph")
            .attr("style", "position:absolute;right:5px;top:5px;"),
        move |state: &mut State, _: PointerClick| expand(state),
    );

    el(
        "div",
        (
            custom_leaf::<State, AppAction>(swatch.leaf_key, swatch.width, swatch.height)
                .attr("aria-hidden", "true"),
            el("div", targets)
                .attr("class", "graph-canvas-swatch-targets")
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:0;top:0;width:{}px;height:{}px;",
                        swatch.width, swatch.height
                    ),
                ),
            expand_button,
        ),
    )
    .attr("class", "graph-canvas-swatch")
    .attr("role", "group")
    .attr("aria-label", swatch.label.clone())
    .attr(
        "style",
        format!(
            "position:relative;display:inline-block;width:{}px;height:{}px;max-width:100%;overflow:hidden;",
            swatch.width, swatch.height
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyView, DomHandle, GenetAppRunner};
    use genet_scripted_dom::{NodeId, ScriptedDom};
    use layout_dom_api::{LayoutDom, LocalName, Namespace};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct State {
        clicked: Vec<u8>,
        hovered: Option<u8>,
        expanded: bool,
    }

    type TestView = Box<dyn AnyView<State, (), GenetCtx, GenetElement>>;

    fn model(hovered: Option<u8>) -> GraphCanvasSwatch<u8, &'static str> {
        let mut swatch = GraphCanvasSwatch::new(
            77,
            GraphCanvasSubgraph {
                nodes: vec![
                    GraphCanvasNode {
                        id: 1,
                        kind: "document",
                        position: (0.1, 0.5),
                        label: "First node".into(),
                    },
                    GraphCanvasNode {
                        id: 2,
                        kind: "person",
                        position: (0.9, 0.5),
                        label: "Second node".into(),
                    },
                ],
                edges: vec![GraphCanvasEdge { from: 1, to: 2 }],
            },
        )
        .with_size(240, 112);
        swatch.selected = Some(1);
        swatch.focus = Some(2);
        swatch.hovered = hovered;
        swatch
    }

    fn view(state: &State) -> TestView {
        let swatch = model(state.hovered);
        Box::new(graph_canvas_swatch(
            &swatch,
            |state: &mut State, id| state.clicked.push(id),
            |state: &mut State, id| state.hovered = id,
            |state: &mut State| state.expanded = true,
        ))
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
    fn node_targets_route_click_hover_and_expand() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), view, State::default());
        let root = runner.root();
        let first =
            find_attr(&dom.borrow(), root, "aria-label", "First node").expect("first node target");
        runner.dispatch_hover(
            first,
            HoverEvent::new(HoverPhase::Enter, (2.0, 2.0), (20.0, 20.0)),
        );
        assert_eq!(runner.state().hovered, Some(1));
        runner.dispatch_click(first, PointerClick::at((2.0, 2.0)));
        assert_eq!(runner.state().clicked, [1]);

        let expand =
            find_attr(&dom.borrow(), root, "aria-label", "Expand graph").expect("expand target");
        runner.dispatch_click(expand, PointerClick::at((2.0, 2.0)));
        assert!(runner.state().expanded);
    }

    #[test]
    fn paint_leaf_and_hit_targets_share_projection() {
        let swatch = model(Some(1));
        let leaf = swatch.paint_leaf(|kind| match *kind {
            "document" => ColorF {
                r: 0.2,
                g: 0.4,
                b: 0.8,
                a: 1.0,
            },
            _ => ColorF {
                r: 0.7,
                g: 0.4,
                b: 0.3,
                a: 1.0,
            },
        });
        let projected = swatch.projected_positions();
        assert_eq!(
            leaf.node_local_position(
                0,
                Size {
                    width: 240.0,
                    height: 112.0
                }
            ),
            Some(projected[0].1)
        );
    }
}
