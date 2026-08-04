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
    FocusEvent, FocusPhase, GenetCtx, GenetElement, HoverEvent, HoverPhase, PointerClick,
    PointerEvent, PointerPhase, View, custom_leaf, el, focusable, on_click, on_focus, on_hover,
    on_pointer,
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
    touch-action: none;
    user-select: none;
}
.graph-canvas-swatch-node:focus-visible {
    outline: 1px solid currentColor;
    outline-offset: 1px;
}
.graph-canvas-swatch-relation {
    background: transparent;
    border: 0;
    cursor: pointer;
    padding: 0;
    touch-action: manipulation;
}
.graph-canvas-swatch-relation:focus-visible {
    outline: 1px solid currentColor;
    outline-offset: 1px;
}
.graph-canvas-swatch-relation.emphasized::after {
    background-color: currentColor;
    content: "";
    height: 3px;
    left: 0;
    opacity: 0.72;
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 100%;
}
.graph-canvas-swatch-label {
    font-size: 10px;
    line-height: 1;
    white-space: nowrap;
    opacity: 0.85;
    pointer-events: none;
}
.graph-canvas-swatch-label.selected {
    font-weight: bold;
    opacity: 1;
}
.graph-canvas-swatch-label.focused,
.graph-canvas-swatch-label.hovered {
    opacity: 1;
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
    /// An optional stable key emitted as `data-key` on the node's hit target,
    /// for targeting by identity when the `label` is not unique (two nodes may
    /// share a title). The component does not interpret it — a driver or test
    /// selects on it; a screen reader still reads `label`.
    pub key: Option<String>,
}

/// One captured node-motion event from a [`GraphCanvasSwatch`].
///
/// `position` is in the graph's normalized `0..=1` coordinate system, after
/// the Swatch has inverted its current viewport. A consumer owns the response:
/// it may write a local position override, hand the position to a solver, or
/// ignore the gesture. The component never edits graph truth.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphCanvasNodeDrag<Id> {
    pub id: Id,
    pub phase: PointerPhase,
    pub position: (f32, f32),
}

/// One app-facing edge. Endpoints that are absent from the subgraph are skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCanvasEdge<Id> {
    pub from: Id,
    pub to: Id,
}

/// One independently curatable relation cell in a graph-canvas Swatch.
///
/// `id` is owned by the consumer and must remain stable for the lifetime of
/// the source relation. Unlike [`GraphCanvasEdge`], it deliberately does not
/// collapse two relations that share endpoints. Visibility and emphasis are
/// projection state: they do not change graph truth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphCanvasRelation<Id> {
    pub id: String,
    pub from: Id,
    pub to: Id,
    pub kind: String,
    pub label: String,
    pub visible: bool,
    pub emphasized: bool,
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
    /// Relation cells rendered in preference to the legacy endpoint-only
    /// [`GraphCanvasSubgraph::edges`] when nonempty.
    pub relations: Vec<GraphCanvasRelation<Id>>,
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
    /// Whether the Expand affordance renders. Defaults on (the original
    /// behavior); a consumer whose swatch has no "fuller view" to expand into
    /// switches it off rather than wiring a lying no-op chip.
    pub show_expand: bool,
    /// Whether each node's `label` also renders as visible text beside its hit
    /// target (it is always the accessible name). Defaults off — a dense
    /// minimap reads better bare; an overview where identity is the point
    /// (sessions, clusters) switches it on.
    ///
    /// Visible labels carry the same `selected` / `focused` / `hovered`
    /// modifiers as the node buttons, so a consumer can emphasize the label of
    /// the node that matters without hand-rolling a parallel label layer.
    pub show_labels: bool,
}

impl<Id, Kind> GraphCanvasSwatch<Id, Kind> {
    /// Build a quiet, card-sized viewport. Dimensions remain configurable so a
    /// host can match its panel density without forking the component.
    pub fn new(leaf_key: u64, graph: GraphCanvasSubgraph<Id, Kind>) -> Self {
        Self {
            leaf_key,
            graph,
            relations: Vec::new(),
            selected: None,
            focus: None,
            hovered: None,
            viewport: GraphViewport::default(),
            width: 260,
            height: 128,
            node_radius: 5.0,
            edge_width: 1.0,
            // Keep the painted dot compact while leaving a touch-sized native
            // target around it. Consumers can opt into a larger visual radius
            // without sacrificing a dependable base interaction area.
            hit_size: 44.0,
            label: "Related graph".to_string(),
            show_expand: true,
            show_labels: false,
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

    /// Show or hide the Expand affordance (see [`Self::show_expand`]).
    #[must_use]
    pub fn with_expand(mut self, on: bool) -> Self {
        self.show_expand = on;
        self
    }

    /// Render each node's label as visible text (see [`Self::show_labels`]).
    #[must_use]
    pub fn with_node_labels(mut self, on: bool) -> Self {
        self.show_labels = on;
        self
    }

    /// Supply identity-bearing relation cells. When present, these replace the
    /// legacy endpoint-only edges in the retained paint projection.
    #[must_use]
    pub fn with_relations(mut self, relations: Vec<GraphCanvasRelation<Id>>) -> Self {
        self.relations = relations;
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
        let edges = if self.relations.is_empty() {
            self.graph
                .edges
                .iter()
                .filter_map(|edge| {
                    let from = self.node_index(Some(&edge.from))?;
                    let to = self.node_index(Some(&edge.to))?;
                    Some((from, to))
                })
                .collect()
        } else {
            self.relations
                .iter()
                .filter(|relation| relation.visible)
                .filter_map(|relation| {
                    let from = self.node_index(Some(&relation.from))?;
                    let to = self.node_index(Some(&relation.to))?;
                    Some((from, to))
                })
                .collect()
        };
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

    /// Visible relation cells with their projected route endpoints. This is
    /// shared by retained paint and native relation targets, so a relation is
    /// neither hittable when hidden nor represented at a stale position.
    pub fn projected_relations(&self) -> Vec<(&GraphCanvasRelation<Id>, (f32, f32), (f32, f32))> {
        let size = Size {
            width: self.width as f32,
            height: self.height as f32,
        };
        let inset = self.node_radius + self.edge_width;
        self.relations
            .iter()
            .filter(|relation| relation.visible)
            .filter_map(|relation| {
                let from = self
                    .graph
                    .nodes
                    .iter()
                    .find(|node| node.id == relation.from)?;
                let to = self
                    .graph
                    .nodes
                    .iter()
                    .find(|node| node.id == relation.to)?;
                Some((
                    relation,
                    self.viewport.project(from.position, size, inset),
                    self.viewport.project(to.position, size, inset),
                ))
            })
            .collect()
    }

    /// Convert a leaf-local pointer position back into the graph's normalized
    /// coordinate system. This is the inverse of the projection used by paint
    /// and native node targets, clamped to the graph's visible bounds so a
    /// captured drag cannot create an unreachable position.
    pub fn graph_position_at(&self, local: (f32, f32)) -> (f32, f32) {
        graph_position_at(
            self.viewport,
            Size {
                width: self.width as f32,
                height: self.height as f32,
            },
            self.node_radius + self.edge_width,
            local,
        )
    }
}

fn graph_position_at(
    viewport: GraphViewport,
    size: Size,
    inset: f32,
    local: (f32, f32),
) -> (f32, f32) {
    let width = (size.width - 2.0 * inset).max(0.0);
    let height = (size.height - 2.0 * inset).max(0.0);
    let zoom = viewport.zoom.max(0.01);
    let x = if width > 0.0 {
        (local.0 - inset) / width
    } else {
        0.5
    };
    let y = if height > 0.0 {
        (local.1 - inset) / height
    } else {
        0.5
    };
    (
        ((x - 0.5 - viewport.pan.0) / zoom + 0.5).clamp(0.0, 1.0),
        ((y - 0.5 - viewport.pan.1) / zoom + 0.5).clamp(0.0, 1.0),
    )
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
    graph_canvas_swatch_with_focus_and_drag_and_relations(
        swatch,
        on_node_click,
        on_node_hover,
        |_state: &mut State, _id: Option<Id>| {},
        |_state: &mut State, _event: GraphCanvasNodeDrag<Id>| {},
        |_state: &mut State, _id: String| {},
        |_state: &mut State, _id: Option<String>| {},
        on_expand,
    )
}

/// Render a graph-canvas Swatch with captured node motion. This is the
/// interaction variant of [`graph_canvas_swatch`]: click and hover retain their
/// existing meanings while `on_node_drag` receives Down/Move/Up in normalized
/// graph coordinates.
pub fn graph_canvas_swatch_with_drag<State, AppAction, Id, Kind, Click, Hover, Drag, Expand>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_node_drag: Drag,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Drag: Fn(&mut State, GraphCanvasNodeDrag<Id>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    graph_canvas_swatch_with_focus_and_drag_and_relations(
        swatch,
        on_node_click,
        on_node_hover,
        |_state: &mut State, _id: Option<Id>| {},
        on_node_drag,
        |_state: &mut State, _id: String| {},
        |_state: &mut State, _id: Option<String>| {},
        on_expand,
    )
}

/// Render a graph-canvas swatch whose retained paint focus follows the native
/// node button that pointer, keyboard, or programmatic focus selected.
///
/// `on_node_focus` normally writes its value into
/// [`GraphCanvasSwatch::focus`] before rebuilding the matching paint leaf.
pub fn graph_canvas_swatch_with_focus<State, AppAction, Id, Kind, Click, Hover, Focus, Expand>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_node_focus: Focus,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Focus: Fn(&mut State, Option<Id>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    graph_canvas_swatch_with_focus_and_drag_and_relations(
        swatch,
        on_node_click,
        on_node_hover,
        on_node_focus,
        |_state: &mut State, _event: GraphCanvasNodeDrag<Id>| {},
        |_state: &mut State, _id: String| {},
        |_state: &mut State, _id: Option<String>| {},
        on_expand,
    )
}

/// Render a graph-canvas Swatch with captured node motion and independently
/// targetable relation cells. Relation callbacks receive a consumer-owned
/// stable relation id, so equal endpoint pairs remain separate cells.
pub fn graph_canvas_swatch_with_drag_and_relations<
    State,
    AppAction,
    Id,
    Kind,
    Click,
    Hover,
    Drag,
    RelationClick,
    RelationHover,
    Expand,
>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_node_drag: Drag,
    on_relation_click: RelationClick,
    on_relation_hover: RelationHover,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Drag: Fn(&mut State, GraphCanvasNodeDrag<Id>) + Clone + 'static,
    RelationClick: Fn(&mut State, String) + Clone + 'static,
    RelationHover: Fn(&mut State, Option<String>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    graph_canvas_swatch_with_focus_and_drag_and_relations(
        swatch,
        on_node_click,
        on_node_hover,
        |_state: &mut State, _id: Option<Id>| {},
        on_node_drag,
        on_relation_click,
        on_relation_hover,
        on_expand,
    )
}

/// Render a graph-canvas Swatch with focus and captured node-motion callbacks.
///
/// The component emits motion only; the consumer keeps ownership of position,
/// solver, and persistence policy. Existing callers can keep using
/// [`graph_canvas_swatch`] or [`graph_canvas_swatch_with_focus`].
pub fn graph_canvas_swatch_with_focus_and_drag<
    State,
    AppAction,
    Id,
    Kind,
    Click,
    Hover,
    Focus,
    Drag,
    Expand,
>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_node_focus: Focus,
    on_node_drag: Drag,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Focus: Fn(&mut State, Option<Id>) + Clone + 'static,
    Drag: Fn(&mut State, GraphCanvasNodeDrag<Id>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    graph_canvas_swatch_with_focus_and_drag_and_relations(
        swatch,
        on_node_click,
        on_node_hover,
        on_node_focus,
        on_node_drag,
        |_state: &mut State, _id: String| {},
        |_state: &mut State, _id: Option<String>| {},
        on_expand,
    )
}

/// The full graph-canvas interaction surface, including independently
/// targetable relation cells.
pub fn graph_canvas_swatch_with_focus_and_drag_and_relations<
    State,
    AppAction,
    Id,
    Kind,
    Click,
    Hover,
    Focus,
    Drag,
    RelationClick,
    RelationHover,
    Expand,
>(
    swatch: &GraphCanvasSwatch<Id, Kind>,
    on_node_click: Click,
    on_node_hover: Hover,
    on_node_focus: Focus,
    on_node_drag: Drag,
    on_relation_click: RelationClick,
    on_relation_hover: RelationHover,
    on_expand: Expand,
) -> impl View<State, AppAction, GenetCtx, Element = GenetElement>
where
    State: 'static,
    AppAction: 'static,
    Id: Clone + PartialEq + 'static,
    Click: Fn(&mut State, Id) + Clone + 'static,
    Hover: Fn(&mut State, Option<Id>) + Clone + 'static,
    Focus: Fn(&mut State, Option<Id>) + Clone + 'static,
    Drag: Fn(&mut State, GraphCanvasNodeDrag<Id>) + Clone + 'static,
    RelationClick: Fn(&mut State, String) + Clone + 'static,
    RelationHover: Fn(&mut State, Option<String>) + Clone + 'static,
    Expand: Fn(&mut State) + Clone + 'static,
{
    let positions = swatch.projected_positions();
    let hit_size = swatch.hit_size.max(1.0);
    let size = Size {
        width: swatch.width as f32,
        height: swatch.height as f32,
    };
    let inset = swatch.node_radius + swatch.edge_width;
    let viewport = swatch.viewport;
    // Visible node labels (opt-in): plain positioned text beside each node,
    // aria-hidden (the button already carries the accessible name) and
    // pointer-transparent (they must not steal the node's clicks).
    let labels: Vec<_> = if swatch.show_labels {
        swatch
            .graph
            .nodes
            .iter()
            .zip(swatch.projected_positions())
            .map(|(node, (_, (x, y)))| {
                // Same state modifiers the node buttons carry. A label layer
                // that cannot say which node is selected forces a consumer to
                // keep hand-rolling the whole layer for the sake of one
                // emphasis, which is what Isometry's pointcrawl was doing: the
                // party's location is the first thing that layer has to say.
                let mut class = String::from("graph-canvas-swatch-label");
                if swatch.selected.as_ref() == Some(&node.id) {
                    class.push_str(" selected");
                }
                if swatch.focus.as_ref() == Some(&node.id) {
                    class.push_str(" focused");
                }
                if swatch.hovered.as_ref() == Some(&node.id) {
                    class.push_str(" hovered");
                }
                el::<_, State, AppAction>("span", node.label.clone())
                    .attr("class", class)
                    .attr("aria-hidden", "true")
                    .attr(
                        "style",
                        format!(
                            "position:absolute;left:{}px;top:{}px;",
                            x + swatch.node_radius + 4.0,
                            y - 5.0,
                        ),
                    )
            })
            .collect()
    } else {
        Vec::new()
    };
    let relation_targets: Vec<_> = swatch
        .projected_relations()
        .into_iter()
        .map(|(relation, (from_x, from_y), (to_x, to_y))| {
            let dx = to_x - from_x;
            let dy = to_y - from_y;
            let length = (dx * dx + dy * dy).sqrt().max(1.0);
            let midpoint_x = (from_x + to_x) / 2.0;
            let midpoint_y = (from_y + to_y) / 2.0;
            let angle = dy.atan2(dx).to_degrees();
            let mut class = String::from("graph-canvas-swatch-relation");
            if relation.emphasized {
                class.push_str(" emphasized");
            }
            let mut target = el::<_, State, AppAction>("button", ())
                .attr("class", class)
                .attr("type", "button")
                .attr("aria-label", format!("{}: {}", relation.kind, relation.label))
                .attr("data-relation-id", relation.id.clone())
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:{}px;top:{}px;width:{length}px;height:44px;transform:rotate({angle}deg);",
                        midpoint_x - length / 2.0,
                        midpoint_y - 22.0,
                    ),
                );
            if relation.emphasized {
                target = target.attr("aria-current", "true");
            }
            let click = on_relation_click.clone();
            let click_id = relation.id.clone();
            let hover = on_relation_hover.clone();
            let hover_id = relation.id.clone();
            on_hover(
                on_click(target, move |state: &mut State, _: PointerClick| {
                    click(state, click_id.clone());
                }),
                move |state: &mut State, event: HoverEvent| match event.phase {
                    HoverPhase::Enter => hover(state, Some(hover_id.clone())),
                    HoverPhase::Leave => hover(state, None),
                    HoverPhase::Move => {}
                },
            )
        })
        .collect();
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
            if let Some(key) = &node.key {
                target = target.attr("data-key", key.clone());
            }

            let click = on_node_click.clone();
            let click_id = node.id.clone();
            let hover = on_node_hover.clone();
            let enter_id = node.id.clone();
            let focus = on_node_focus.clone();
            let focus_id = node.id.clone();
            let drag = on_node_drag.clone();
            let drag_id = node.id.clone();
            let target_left = x - hit_size / 2.0;
            let target_top = y - hit_size / 2.0;
            on_pointer(focusable(on_focus(
                on_hover(
                    on_click(target, move |state: &mut State, _: PointerClick| {
                        click(state, click_id.clone());
                    }),
                    move |state: &mut State, event: HoverEvent| match event.phase {
                        HoverPhase::Enter => hover(state, Some(enter_id.clone())),
                        HoverPhase::Leave => hover(state, None),
                        HoverPhase::Move => {}
                    },
                ),
                move |state: &mut State, event: FocusEvent| match event.phase {
                    FocusPhase::Gained => focus(state, Some(focus_id.clone())),
                    FocusPhase::Lost => focus(state, None),
                },
            )), move |state: &mut State, event: PointerEvent| {
                let position = graph_position_at(
                    viewport,
                    size,
                    inset,
                    (target_left + event.local.0, target_top + event.local.1),
                );
                drag(
                    state,
                    GraphCanvasNodeDrag {
                        id: drag_id.clone(),
                        phase: event.phase,
                        position,
                    },
                );
            })
        })
        .collect();

    let expand = on_expand.clone();
    let expand_buttons: Vec<_> = if swatch.show_expand {
        vec![on_click(
            el::<_, State, AppAction>("button", "Expand")
                .attr("class", "graph-canvas-swatch-expand")
                .attr("type", "button")
                .attr("aria-label", "Expand graph")
                .attr("style", "position:absolute;right:5px;top:5px;"),
            move |state: &mut State, _: PointerClick| expand(state),
        )]
    } else {
        Vec::new()
    };

    el(
        "div",
        (
            custom_leaf::<State, AppAction>(swatch.leaf_key, swatch.width, swatch.height)
                .attr("aria-hidden", "true"),
            el("div", labels)
                .attr("class", "graph-canvas-swatch-labels")
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:0;top:0;width:{}px;height:{}px;",
                        swatch.width, swatch.height
                    ),
                ),
            el("div", relation_targets)
                .attr("class", "graph-canvas-swatch-relation-targets")
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:0;top:0;width:{}px;height:{}px;",
                        swatch.width, swatch.height
                    ),
                ),
            el("div", targets)
                .attr("class", "graph-canvas-swatch-targets")
                .attr(
                    "style",
                    format!(
                        "position:absolute;left:0;top:0;width:{}px;height:{}px;",
                        swatch.width, swatch.height
                    ),
                ),
            expand_buttons,
        ),
    )
    .attr("class", "graph-canvas-swatch")
    .attr("role", "group")
    .attr("aria-label", swatch.label.clone())
    .attr(
        "style",
        format!(
            "position:relative;display:block;width:{}px;height:{}px;max-width:100%;",
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
        focused: Option<u8>,
        dragged: Vec<GraphCanvasNodeDrag<u8>>,
        relation_clicked: Vec<String>,
        relation_hovered: Option<String>,
        expanded: bool,
    }

    type TestView = Box<dyn AnyView<State, (), GenetCtx, GenetElement>>;

    fn model(hovered: Option<u8>, focused: Option<u8>) -> GraphCanvasSwatch<u8, &'static str> {
        let mut swatch = GraphCanvasSwatch::new(
            77,
            GraphCanvasSubgraph {
                nodes: vec![
                    GraphCanvasNode {
                        id: 1,
                        kind: "document",
                        position: (0.1, 0.5),
                        label: "First node".into(),
                        key: None,
                    },
                    GraphCanvasNode {
                        id: 2,
                        kind: "person",
                        position: (0.9, 0.5),
                        label: "Second node".into(),
                        key: None,
                    },
                ],
                edges: vec![GraphCanvasEdge { from: 1, to: 2 }],
            },
        )
        .with_size(240, 112);
        swatch.selected = Some(1);
        swatch.focus = focused;
        swatch.hovered = hovered;
        swatch
    }

    fn view(state: &State) -> TestView {
        let swatch = model(state.hovered, state.focused);
        Box::new(graph_canvas_swatch_with_focus(
            &swatch,
            |state: &mut State, id| state.clicked.push(id),
            |state: &mut State, id| state.hovered = id,
            |state: &mut State, id| state.focused = id,
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
        assert_eq!(runner.focus(), Some(first));
        assert_eq!(runner.state().focused, Some(1));

        let expand =
            find_attr(&dom.borrow(), root, "aria-label", "Expand graph").expect("expand target");
        runner.dispatch_click(expand, PointerClick::at((2.0, 2.0)));
        assert!(runner.state().expanded);
    }

    fn labelled_view(state: &State) -> TestView {
        let swatch = model(state.hovered, state.focused).with_node_labels(true);
        Box::new(graph_canvas_swatch_with_focus(
            &swatch,
            |state: &mut State, id| state.clicked.push(id),
            |state: &mut State, id| state.hovered = id,
            |state: &mut State, id| state.focused = id,
            |state: &mut State| state.expanded = true,
        ))
    }

    /// Collect the class attribute of every element whose *base* class is
    /// `base`. Matching the first token rather than a prefix keeps the
    /// `-labels` container out of a search for `-label` elements.
    fn classes_of(dom: &ScriptedDom, root: NodeId, base: &str, out: &mut Vec<String>) {
        if let Some(class) = attr(dom, root, "class") {
            if class.split_whitespace().next() == Some(base) {
                out.push(class.to_string());
            }
        }
        for child in dom.dom_children(root) {
            classes_of(dom, child, base, out);
        }
    }

    /// Visible labels carry the node's state, not just its text.
    ///
    /// Without this a consumer that needs to emphasize one label (which node is
    /// "here") has to hand-roll the entire label layer to get it, which defeats
    /// the point of the component rendering labels at all.
    #[test]
    fn visible_labels_carry_node_state() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = State {
            hovered: Some(2),
            ..State::default()
        };
        let mut runner = GenetAppRunner::<_, _, _, ()>::new(dom.clone(), labelled_view, state);
        let root = runner.root();
        let mut found = Vec::new();
        classes_of(&dom.borrow(), root, "graph-canvas-swatch-label", &mut found);

        assert_eq!(found.len(), 2, "one label per node: {found:?}");
        // `model` selects node 1; the test state hovers node 2.
        assert!(
            found.iter().any(|c| c == "graph-canvas-swatch-label selected"),
            "the selected node's label must say so: {found:?}"
        );
        assert!(
            found.iter().any(|c| c == "graph-canvas-swatch-label hovered"),
            "the hovered node's label must say so: {found:?}"
        );
    }

    #[test]
    fn paint_leaf_and_hit_targets_share_projection() {
        let swatch = model(Some(1), Some(2));
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

    #[test]
    fn graph_position_inverts_the_viewport_projection() {
        let mut swatch = model(None, None);
        swatch.viewport = GraphViewport {
            pan: (0.12, -0.08),
            zoom: 1.35,
        };
        let point = (0.73, 0.21);
        let size = Size {
            width: swatch.width as f32,
            height: swatch.height as f32,
        };
        let leaf = swatch
            .viewport
            .project(point, size, swatch.node_radius + swatch.edge_width);
        let recovered = swatch.graph_position_at(leaf);
        assert!((recovered.0 - point.0).abs() < 1e-5, "{recovered:?}");
        assert!((recovered.1 - point.1).abs() < 1e-5, "{recovered:?}");
    }

    fn drag_view(state: &State) -> TestView {
        let swatch = model(state.hovered, state.focused);
        Box::new(graph_canvas_swatch_with_drag(
            &swatch,
            |state: &mut State, id| state.clicked.push(id),
            |state: &mut State, id| state.hovered = id,
            |state: &mut State, event| state.dragged.push(event),
            |state: &mut State| state.expanded = true,
        ))
    }

    fn relation_view(state: &State) -> TestView {
        let swatch = model(state.hovered, state.focused).with_relations(vec![
            GraphCanvasRelation {
                id: "cites".into(),
                from: 1,
                to: 2,
                kind: "Citation".into(),
                label: "First node cites Second node".into(),
                visible: true,
                emphasized: true,
            },
            GraphCanvasRelation {
                id: "quotes".into(),
                from: 1,
                to: 2,
                kind: "Quotation".into(),
                label: "First node quotes Second node".into(),
                visible: true,
                emphasized: false,
            },
            GraphCanvasRelation {
                id: "hidden".into(),
                from: 1,
                to: 2,
                kind: "Hidden".into(),
                label: "This relation is hidden".into(),
                visible: false,
                emphasized: false,
            },
        ]);
        Box::new(graph_canvas_swatch_with_drag_and_relations(
            &swatch,
            |state: &mut State, id| state.clicked.push(id),
            |state: &mut State, id| state.hovered = id,
            |state: &mut State, event| state.dragged.push(event),
            |state: &mut State, id| state.relation_clicked.push(id),
            |state: &mut State, id| state.relation_hovered = id,
            |state: &mut State| state.expanded = true,
        ))
    }

    #[test]
    fn relation_cells_keep_parallel_endpoints_independent_and_hideable() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            GenetAppRunner::<_, _, _, ()>::new(dom.clone(), relation_view, State::default());
        let root = runner.root();
        let cites = find_attr(&dom.borrow(), root, "data-relation-id", "cites")
            .expect("citation relation target");
        let quotes = find_attr(&dom.borrow(), root, "data-relation-id", "quotes")
            .expect("quotation relation target");
        assert_ne!(cites, quotes, "parallel cells must get separate targets");
        assert!(
            find_attr(&dom.borrow(), root, "data-relation-id", "hidden").is_none(),
            "hidden cells must not remain hittable"
        );
        assert_eq!(attr(&dom.borrow(), cites, "aria-current"), Some("true"));

        runner.dispatch_hover(
            quotes,
            HoverEvent::new(HoverPhase::Enter, (2.0, 2.0), (20.0, 20.0)),
        );
        runner.dispatch_click(cites, PointerClick::at((2.0, 2.0)));
        assert_eq!(runner.state().relation_hovered.as_deref(), Some("quotes"));
        assert_eq!(runner.state().relation_clicked, ["cites"]);

        let swatch = model(None, None).with_relations(vec![GraphCanvasRelation {
            id: "hidden".into(),
            from: 1,
            to: 2,
            kind: "Hidden".into(),
            label: "This relation is hidden".into(),
            visible: false,
            emphasized: false,
        }]);
        assert!(swatch.projected_relations().is_empty());
    }

    #[test]
    fn node_drag_captures_and_reports_normalized_graph_positions() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner =
            GenetAppRunner::<_, _, _, ()>::new(dom.clone(), drag_view, State::default());
        let root = runner.root();
        let first =
            find_attr(&dom.borrow(), root, "aria-label", "First node").expect("first node target");

        runner.dispatch_pointer_down(
            first,
            PointerEvent::new(PointerPhase::Down, (10.0, 10.0), (20.0, 20.0)),
        );
        assert_eq!(runner.pointer_capture(), Some(first));
        runner.dispatch_pointer_move(PointerEvent::new(
            PointerPhase::Move,
            (120.0, 10.0),
            (20.0, 20.0),
        ));
        runner.dispatch_pointer_up(PointerEvent::new(
            PointerPhase::Up,
            (120.0, 10.0),
            (20.0, 20.0),
        ));

        let dragged = &runner.state().dragged;
        assert_eq!(
            dragged.iter().map(|event| event.phase).collect::<Vec<_>>(),
            vec![PointerPhase::Down, PointerPhase::Move, PointerPhase::Up]
        );
        assert!(
            dragged[1].position.0 > dragged[0].position.0,
            "a rightward drag advances the normalized x position: {dragged:?}"
        );
        assert!(
            dragged.iter().all(|event| event.id == 1
                && (0.0..=1.0).contains(&event.position.0)
                && (0.0..=1.0).contains(&event.position.1)),
            "every emitted event keeps the node id and clamps graph coordinates: {dragged:?}"
        );
        assert_eq!(runner.pointer_capture(), None, "Up releases capture");
    }
}
