/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Graph rendering module using egui_graphs.
//!
//! Delegates graph visualization and interaction to the egui_graphs crate,
//! which provides built-in navigation (zoom/pan), node dragging, and selection.

use crate::app::{EdgeCommand, GraphBrowserApp, GraphIntent, PendingTileOpenMode};
use crate::graph::egui_adapter::{EguiGraphState, GraphNodeShape};
use crate::graph::{NodeKey, NodeLifecycle};
use egui::{Color32, Key, Stroke, Ui, Vec2, Window};
use egui_graphs::events::Event;
use egui_graphs::{
    DefaultEdgeShape, FruchtermanReingoldWithCenterGravity,
    FruchtermanReingoldWithCenterGravityState, GraphView,
    LayoutForceDirected, MetadataFrame, SettingsInteraction, SettingsNavigation, SettingsStyle,
    get_layout_state, set_layout_state,
};
use euclid::default::Point2D;
use petgraph::stable_graph::NodeIndex;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Graph interaction action (resolved from egui_graphs events).
///
/// Decouples event conversion (needs `egui_state` for NodeIndex→NodeKey
/// lookups) from action application (pure state mutation), making
/// graph interactions testable without an egui rendering context.
pub enum GraphAction {
    FocusNode(NodeKey),
    FocusNodeSplit(NodeKey),
    DragStart,
    DragEnd(NodeKey, Point2D<f32>),
    MoveNode(NodeKey, Point2D<f32>),
    SelectNode { key: NodeKey, multi_select: bool },
    Zoom(f32),
}

/// Render graph info and controls hint overlay text into the current UI.
pub fn render_graph_info_in_ui(ui: &mut Ui, app: &GraphBrowserApp) {
    draw_graph_info(ui, app);
}

/// Render graph content and return resolved interaction actions.
///
/// This lets callers customize how specific actions are handled
/// (e.g. routing double-click to tile opening instead of detail view).
pub fn render_graph_in_ui_collect_actions(
    ui: &mut Ui,
    app: &mut GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    search_filter_mode: bool,
    search_query_active: bool,
) -> Vec<GraphAction> {
    let ctrl_pressed = ui.input(|i| i.modifiers.ctrl);
    let filtered_graph = if search_filter_mode && search_query_active {
        Some(filtered_graph_for_search(app, search_matches))
    } else {
        None
    };
    let graph_for_render = filtered_graph.as_ref().unwrap_or(&app.graph);

    // Build or reuse egui_graphs state (rebuild always when filtering is active).
    if app.egui_state.is_none() || app.egui_state_dirty || filtered_graph.is_some() {
        app.egui_state = Some(EguiGraphState::from_graph(
            graph_for_render,
            &app.selected_nodes,
        ));
        app.egui_state_dirty = false;
    }

    apply_search_node_visuals(
        app,
        search_matches,
        active_search_match,
        search_query_active,
    );

    // Event collection buffer
    let events: Rc<RefCell<Vec<Event>>> = Rc::new(RefCell::new(Vec::new()));

    // Navigation: use egui_graphs built-in zoom/pan
    let nav = SettingsNavigation::new()
        .with_fit_to_screen_enabled(app.fit_to_screen_requested)
        .with_zoom_and_pan_enabled(true)
        .with_zoom_speed(0.05);

    // Interaction: dragging, selection, clicking
    let interaction = SettingsInteraction::new()
        .with_dragging_enabled(true)
        .with_node_selection_enabled(true)
        .with_node_clicking_enabled(true);

    // Style: always show labels
    let style = SettingsStyle::new().with_labels_always(true);

    // Keep egui_graphs layout cache aligned with app-owned FR state.
    set_layout_state::<FruchtermanReingoldWithCenterGravityState>(ui, app.physics.clone(), None);

    // Render the graph (nested scope for mutable borrow)
    {
        let state = app
            .egui_state
            .as_mut()
            .expect("egui_state should be initialized");

        ui.add(
            &mut GraphView::<
                _,
                _,
                _,
                _,
                GraphNodeShape,
                DefaultEdgeShape,
                FruchtermanReingoldWithCenterGravityState,
                LayoutForceDirected<FruchtermanReingoldWithCenterGravity>,
            >::new(&mut state.graph)
            .with_navigations(&nav)
            .with_interactions(&interaction)
            .with_styles(&style)
            .with_event_sink(&events),
        );
    } // Drop mutable borrow of app.egui_state here

    // Pull latest FR state from egui_graphs after this frame's layout step.
    app.physics = get_layout_state::<FruchtermanReingoldWithCenterGravityState>(ui, None);
    app.hovered_graph_node = app.egui_state.as_ref().and_then(|state| {
        state
            .graph
            .hovered_node()
            .and_then(|idx| state.get_key(idx))
    });
    draw_highlighted_edge_overlay(ui, app);

    // Reset fit_to_screen flag (one-shot behavior for 'C' key)
    app.fit_to_screen_requested = false;

    // Post-frame zoom clamp: enforce min/max bounds on egui_graphs zoom
    clamp_zoom(ui.ctx(), app);

    let split_open_modifier = ui.input(|i| i.modifiers.shift);
    collect_graph_actions(app, &events, split_open_modifier, ctrl_pressed)
}

fn draw_highlighted_edge_overlay(ui: &mut Ui, app: &GraphBrowserApp) {
    let Some((from, to)) = app.highlighted_graph_edge else {
        return;
    };
    let Some(state) = app.egui_state.as_ref() else {
        return;
    };
    let Some(from_node) = state.graph.node(from) else {
        return;
    };
    let Some(to_node) = state.graph.node(to) else {
        return;
    };
    let meta_id = egui::Id::new("egui_graphs_metadata_");
    let (from_screen, to_screen) = if let Some(meta) = ui
        .ctx()
        .data_mut(|d| d.get_persisted::<MetadataFrame>(meta_id))
    {
        (
            meta.canvas_to_screen_pos(from_node.location()),
            meta.canvas_to_screen_pos(to_node.location()),
        )
    } else {
        (from_node.location(), to_node.location())
    };
    ui.painter().line_segment(
        [from_screen, to_screen],
        Stroke::new(6.0, Color32::from_rgba_unmultiplied(10, 30, 40, 120)),
    );
    ui.painter().line_segment(
        [from_screen, to_screen],
        Stroke::new(5.0, Color32::from_rgb(80, 220, 255)),
    );
    // Draw endpoint markers so edge-search selection is obvious even on dense graphs.
    ui.painter()
        .circle_filled(from_screen, 6.0, Color32::from_rgb(80, 220, 255));
    ui.painter()
        .circle_filled(to_screen, 6.0, Color32::from_rgb(80, 220, 255));
}

fn filtered_graph_for_search(
    app: &GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
) -> crate::graph::Graph {
    let mut filtered = app.graph.clone();
    let to_remove: Vec<NodeKey> = filtered
        .nodes()
        .map(|(key, _)| key)
        .filter(|key| !search_matches.contains(key))
        .collect();
    for key in to_remove {
        filtered.remove_node(key);
    }
    filtered
}

fn lifecycle_color(lifecycle: NodeLifecycle) -> Color32 {
    match lifecycle {
        NodeLifecycle::Active => Color32::from_rgb(100, 200, 255),
        NodeLifecycle::Cold => Color32::from_rgb(140, 140, 165),
    }
}

fn apply_search_node_visuals(
    app: &mut GraphBrowserApp,
    search_matches: &HashSet<NodeKey>,
    active_search_match: Option<NodeKey>,
    search_query_active: bool,
) {
    let hovered = app.hovered_graph_node;
    let highlighted_edge = app.highlighted_graph_edge;
    let colors: Vec<(NodeKey, Color32)> = app
        .graph
        .nodes()
        .map(|(key, node)| {
            let mut color = lifecycle_color(node.lifecycle);
            if app.selected_nodes.contains(&key) {
                color = Color32::from_rgb(255, 200, 100);
            }
            if search_query_active && search_matches.contains(&key) {
                color = if active_search_match == Some(key) {
                    Color32::from_rgb(140, 255, 140)
                } else {
                    Color32::from_rgb(95, 220, 130)
                };
            }
            if let Some((from, to)) = highlighted_edge
                && (key == from || key == to)
            {
                color = Color32::from_rgb(80, 220, 255);
            }
            if hovered == Some(key) {
                // Visual cue for command-target disambiguation while hovering.
                color = Color32::from_rgb(255, 150, 80);
            }
            (key, color)
        })
        .collect();

    let Some(state) = app.egui_state.as_mut() else {
        return;
    };
    for (key, color) in colors {
        if let Some(node) = state.graph.node_mut(key) {
            node.set_color(color);
        }
    }
}

/// Clamp the egui_graphs zoom to the camera's min/max bounds.
/// Reads MetadataFrame from egui's persisted data, clamps zoom, writes back if changed.
fn clamp_zoom(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    let meta_id = egui::Id::new("egui_graphs_metadata_");
    ctx.data_mut(|data| {
        if let Some(mut meta) = data.get_persisted::<MetadataFrame>(meta_id) {
            let clamped = app.camera.clamp(meta.zoom);
            app.camera.current_zoom = clamped;
            if (meta.zoom - clamped).abs() > f32::EPSILON {
                meta.zoom = clamped;
                data.insert_persisted(meta_id, meta);
            }
        }
    });
}

/// Convert egui_graphs events to resolved GraphActions and apply them.
fn collect_graph_actions(
    app: &GraphBrowserApp,
    events: &Rc<RefCell<Vec<Event>>>,
    split_open_modifier: bool,
    multi_select_modifier: bool,
) -> Vec<GraphAction> {
    let mut actions = Vec::new();

    for event in events.borrow_mut().drain(..) {
        match event {
            Event::NodeDoubleClick(p) => {
                if let Some(state) = app.egui_state.as_ref() {
                    let idx = NodeIndex::new(p.id);
                    if let Some(key) = state.get_key(idx) {
                        if split_open_modifier {
                            actions.push(GraphAction::FocusNodeSplit(key));
                        } else {
                            actions.push(GraphAction::FocusNode(key));
                        }
                    }
                }
            },
            Event::NodeDragStart(_) => {
                actions.push(GraphAction::DragStart);
            },
            Event::NodeDragEnd(p) => {
                // Resolve final position from egui_state
                let idx = NodeIndex::new(p.id);
                if let Some(state) = app.egui_state.as_ref() {
                    if let Some(key) = state.get_key(idx) {
                        let pos = state
                            .graph
                            .node(idx)
                            .map(|n| Point2D::new(n.location().x, n.location().y))
                            .unwrap_or_default();
                        actions.push(GraphAction::DragEnd(key, pos));
                    }
                }
            },
            Event::NodeMove(p) => {
                let idx = NodeIndex::new(p.id);
                if let Some(state) = app.egui_state.as_ref() {
                    if let Some(key) = state.get_key(idx) {
                        actions.push(GraphAction::MoveNode(
                            key,
                            Point2D::new(p.new_pos[0], p.new_pos[1]),
                        ));
                    }
                }
            },
            Event::NodeSelect(p) => {
                if let Some(state) = app.egui_state.as_ref() {
                    let idx = NodeIndex::new(p.id);
                    if let Some(key) = state.get_key(idx) {
                        actions.push(GraphAction::SelectNode {
                            key,
                            multi_select: multi_select_modifier,
                        });
                    }
                }
            },
            Event::NodeDeselect(_) => {
                // Selection clearing handled by the next SelectNode action
            },
            Event::Zoom(p) => {
                actions.push(GraphAction::Zoom(p.new_zoom));
            },
            _ => {},
        }
    }

    actions
}

/// Convert resolved graph actions to graph intents without applying them.
pub fn intents_from_graph_actions(actions: Vec<GraphAction>) -> Vec<GraphIntent> {
    let mut intents = Vec::with_capacity(actions.len());
    for action in actions {
        match action {
            GraphAction::FocusNode(key) => {
                intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            },
            GraphAction::FocusNodeSplit(key) => {
                intents.push(GraphIntent::SelectNode {
                    key,
                    multi_select: false,
                });
            },
            GraphAction::DragStart => {
                intents.push(GraphIntent::SetInteracting { interacting: true });
            },
            GraphAction::DragEnd(key, pos) => {
                intents.push(GraphIntent::SetInteracting { interacting: false });
                intents.push(GraphIntent::SetNodePosition { key, position: pos });
            },
            GraphAction::MoveNode(key, pos) => {
                intents.push(GraphIntent::SetNodePosition { key, position: pos });
            },
            GraphAction::SelectNode { key, multi_select } => {
                intents.push(GraphIntent::SelectNode { key, multi_select });
            },
            GraphAction::Zoom(new_zoom) => {
                intents.push(GraphIntent::SetZoom { zoom: new_zoom });
            },
        }
    }
    intents
}

/// Sync node positions from egui_graphs layout state back into app graph state.
///
/// Pinned nodes keep their app-authored positions; their visual positions are
/// restored after layout so FR simulation does not move them.
pub(crate) fn sync_graph_positions_from_layout(app: &mut GraphBrowserApp) {
    let Some(state) = app.egui_state.as_ref() else {
        return;
    };

    let layout_positions: Vec<(NodeKey, Point2D<f32>)> = app
        .graph
        .nodes()
        .filter_map(|(key, _)| {
            state
                .graph
                .node(key)
                .map(|n| (key, Point2D::new(n.location().x, n.location().y)))
        })
        .collect();

    let mut pinned_positions = Vec::new();
    for (key, pos) in layout_positions {
        if let Some(node_mut) = app.graph.get_node_mut(key) {
            if node_mut.is_pinned {
                pinned_positions.push((key, node_mut.position));
            } else {
                node_mut.position = pos;
            }
        }
    }

    if let Some(state_mut) = app.egui_state.as_mut() {
        for (key, pos) in pinned_positions {
            if let Some(egui_node) = state_mut.graph.node_mut(key) {
                egui_node.set_location(egui::Pos2::new(pos.x, pos.y));
            }
        }
    }
}

/// Draw graph information overlay
fn draw_graph_info(ui: &mut egui::Ui, app: &GraphBrowserApp) {
    let info_text = format!(
        "Nodes: {} | Edges: {} | Physics: {} | Zoom: {:.1}x",
        app.graph.node_count(),
        app.graph.edge_count(),
        if app.physics.base.is_running {
            "Running"
        } else {
            "Paused"
        },
        app.camera.current_zoom
    );

    ui.painter().text(
        ui.available_rect_before_wrap().left_top() + Vec2::new(10.0, 10.0),
        egui::Align2::LEFT_TOP,
        info_text,
        egui::FontId::monospace(12.0),
        Color32::from_rgb(200, 200, 200),
    );

    // Draw controls hint
    let controls_text = "Shortcuts: Ctrl+Click Multi-select | Double-click Open | Drag tab out to split | N New Node | Del Remove | T Physics | C Fit | Ctrl+F Search | G Edge Ops | F2 Commands | F3 Radial | Ctrl+Z/Y Undo/Redo | F1/? Help";
    ui.painter().text(
        ui.available_rect_before_wrap().left_bottom() + Vec2::new(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        controls_text,
        egui::FontId::proportional(10.0),
        Color32::from_rgb(150, 150, 150),
    );
}

/// Render physics configuration panel
pub fn render_physics_panel(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    if !app.show_physics_panel {
        return;
    }

    Window::new("Physics Configuration")
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.heading("Force Parameters");

            let mut config = app.physics.clone();
            let mut config_changed = false;

            ui.add_space(8.0);

            ui.label("Repulsion (c_repulse):");
            if ui
                .add(egui::Slider::new(&mut config.base.c_repulse, 0.0..=10.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Attraction (c_attract):");
            if ui
                .add(egui::Slider::new(&mut config.base.c_attract, 0.0..=10.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Ideal Distance Scale (k_scale):");
            if ui
                .add(egui::Slider::new(&mut config.base.k_scale, 0.1..=5.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);
            ui.label("Center Gravity:");
            if ui
                .add(egui::Slider::new(&mut config.extras.0.params.c, 0.0..=1.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Max Step:");
            if ui
                .add(egui::Slider::new(&mut config.base.max_step, 0.1..=100.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.heading("Damping & Convergence");
            ui.add_space(8.0);

            ui.label("Damping:");
            if ui
                .add(egui::Slider::new(&mut config.base.damping, 0.01..=1.0))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Time Step (dt):");
            if ui
                .add(egui::Slider::new(&mut config.base.dt, 0.001..=1.0).logarithmic(true))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(4.0);

            ui.label("Epsilon:");
            if ui
                .add(egui::Slider::new(&mut config.base.epsilon, 1e-6..=0.1).logarithmic(true))
                .changed()
            {
                config_changed = true;
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Reset button
            ui.horizontal(|ui| {
                if ui.button("Reset to Defaults").clicked() {
                    let running = config.base.is_running;
                    config = GraphBrowserApp::default_physics_state();
                    config.base.is_running = running;
                    config_changed = true;
                }

                ui.label(if app.physics.base.is_running {
                    "Status: Running"
                } else {
                    "Status: Paused"
                });
            });

            if let Some(last_avg) = app.physics.base.last_avg_displacement {
                ui.label(format!("Last avg displacement: {:.4}", last_avg));
            }
            ui.label(format!("Step count: {}", app.physics.base.step_count));

            // Apply config changes
            if config_changed {
                app.update_physics_config(config);
            }
        });
}

/// Render keyboard shortcut help panel
pub fn render_help_panel(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    if !app.show_help_panel {
        return;
    }

    let mut open = app.show_help_panel;
    Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .default_width(350.0)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("shortcut_grid")
                .num_columns(2)
                .spacing([20.0, 6.0])
                .show(ui, |ui| {
                    let shortcuts = [
                        ("Home / Esc", "Toggle Graph / Detail view"),
                        ("N", "Create new node"),
                        ("Delete", "Remove selected nodes"),
                        ("Ctrl+Shift+Delete", "Clear entire graph"),
                        ("T", "Toggle physics simulation"),
                        ("C", "Fit graph to screen"),
                        ("P", "Physics settings panel"),
                        ("Ctrl+F", "Show graph search"),
                        ("F2", "Toggle edge command palette"),
                        ("F3", "Toggle radial command menu"),
                        ("Ctrl+Z / Ctrl+Y", "Undo / Redo"),
                        ("G", "Connect selected pair"),
                        ("Shift+G", "Connect both directions"),
                        ("Alt+G", "Remove user edge"),
                        ("I / U", "Pin / Unpin selected node(s)"),
                        ("Search Up/Down", "Cycle graph matches"),
                        ("Search Enter", "Select active search match"),
                        ("F1 / ?", "This help panel"),
                        ("Ctrl+L / Alt+D", "Focus address bar"),
                        ("Double-click node", "Open node in detail view"),
                        ("Drag tab out", "Detach tab into split pane"),
                        ("Shift + Double-click node", "Fallback split-open gesture"),
                        ("Click + drag", "Move a node"),
                        ("Scroll wheel", "Zoom in / out"),
                    ];

                    for (key, desc) in shortcuts {
                        ui.strong(key);
                        ui.label(desc);
                        ui.end_row();
                    }
                });
        });
    app.show_help_panel = open;
}

/// Render edge command palette panel (keyboard-first palette; radial UI can reuse this dispatch).
pub fn render_command_palette_panel(
    ctx: &egui::Context,
    app: &mut GraphBrowserApp,
    hovered_node: Option<NodeKey>,
    focused_pane_node: Option<NodeKey>,
) {
    if !app.show_command_palette {
        return;
    }

    let mut open = app.show_command_palette;
    let mut intents = Vec::new();
    let mut should_close = false;
    let pair_context = resolve_pair_command_context(app, hovered_node, focused_pane_node);
    let any_selected = !app.selected_nodes.is_empty();
    let source_context = resolve_source_node_context(app, hovered_node, focused_pane_node);

    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        should_close = true;
    }

    Window::new("Edge Commands")
        .open(&mut open)
        .default_width(320.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label("Selection-driven graph commands");
            ui.add_space(6.0);

            if ui
                .add_enabled(
                    pair_context.is_some(),
                    egui::Button::new("Connect Source -> Target"),
                )
                .clicked()
            {
                if let Some((from, to)) = pair_context {
                    intents.push(GraphIntent::ExecuteEdgeCommand {
                        command: EdgeCommand::ConnectPair { from, to },
                    });
                    should_close = true;
                }
            }
            if ui
                .add_enabled(
                    pair_context.is_some(),
                    egui::Button::new("Connect Both Directions"),
                )
                .clicked()
            {
                if let Some((a, b)) = pair_context {
                    intents.push(GraphIntent::ExecuteEdgeCommand {
                        command: EdgeCommand::ConnectBothDirectionsPair { a, b },
                    });
                    should_close = true;
                }
            }
            if ui
                .add_enabled(
                    pair_context.is_some(),
                    egui::Button::new("Remove User Edge"),
                )
                .clicked()
            {
                if let Some((a, b)) = pair_context {
                    intents.push(GraphIntent::ExecuteEdgeCommand {
                        command: EdgeCommand::RemoveUserEdgePair { a, b },
                    });
                    should_close = true;
                }
            }
            ui.separator();
            if ui
                .add_enabled(any_selected, egui::Button::new("Pin Selected"))
                .clicked()
            {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::PinSelected,
                });
                should_close = true;
            }
            if ui
                .add_enabled(any_selected, egui::Button::new("Unpin Selected"))
                .clicked()
            {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::UnpinSelected,
                });
                should_close = true;
            }
            ui.separator();
            if ui.button("Toggle Physics Panel").clicked() {
                intents.push(GraphIntent::TogglePhysicsPanel);
                should_close = true;
            }
            if ui.button("Toggle Physics Simulation").clicked() {
                intents.push(GraphIntent::TogglePhysics);
                should_close = true;
            }
            if ui.button("Fit Graph to Screen").clicked() {
                intents.push(GraphIntent::RequestFitToScreen);
                should_close = true;
            }
            if ui.button("Open Persistence Hub").clicked() {
                intents.push(GraphIntent::TogglePersistencePanel);
                should_close = true;
            }
            ui.separator();
            if ui
                .add_enabled(
                    focused_pane_node.is_some(),
                    egui::Button::new("Detach Focused to Split"),
                )
                .clicked()
                && let Some(focused) = focused_pane_node
            {
                app.request_detach_node_to_split(focused);
                should_close = true;
            }
            if ui.button("Create Node").clicked() {
                intents.push(GraphIntent::CreateNodeNearCenter);
                should_close = true;
            }
            if ui.button("Create Node as Tab").clicked() {
                intents.push(GraphIntent::CreateNodeNearCenter);
                app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
                should_close = true;
            }
            ui.separator();
            if ui
                .add_enabled(
                    source_context.is_some(),
                    egui::Button::new("Open Connected as Tabs"),
                )
                .clicked()
                && let Some(source) = source_context
            {
                app.request_open_connected_from(source, PendingTileOpenMode::Tab);
                should_close = true;
            }
            ui.separator();
            if ui.button("Close").clicked() {
                should_close = true;
            }
            ui.add_space(6.0);
            ui.small("Keyboard: G, Shift+G, Alt+G, I, U");
        });

    app.show_command_palette = open && !should_close;
    apply_ui_intents_with_checkpoint(app, intents);
}

pub fn render_radial_command_menu(
    ctx: &egui::Context,
    app: &mut GraphBrowserApp,
    hovered_node: Option<NodeKey>,
    focused_pane_node: Option<NodeKey>,
) {
    if !app.show_radial_menu {
        return;
    }

    let pair_context = resolve_pair_command_context(app, hovered_node, focused_pane_node);
    let any_selected = !app.selected_nodes.is_empty();
    let source_context = resolve_source_node_context(app, hovered_node, focused_pane_node);
    let mut intents = Vec::new();
    let mut should_close = false;
    let center_id = egui::Id::new("radial_menu_center");
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let center = ctx
        .data_mut(|d| d.get_persisted::<egui::Pos2>(center_id))
        .or(pointer)
        .unwrap_or(egui::pos2(320.0, 220.0));
    ctx.data_mut(|d| d.insert_persisted(center_id, center));

    let mut hovered_domain = None;
    let mut hovered_command = None;
    if let Some(pos) = pointer {
        let delta = pos - center;
        let r = delta.length();
        if r > 40.0 {
            let angle = delta.y.atan2(delta.x);
            hovered_domain = Some(domain_from_angle(angle));
            if r > 120.0 && let Some(domain) = hovered_domain {
                hovered_command = nearest_command_for_pointer(
                    domain,
                    center,
                    pos,
                    pair_context,
                    any_selected,
                    source_context,
                );
            }
        }
    }

    let mut clicked_command = None;
    if ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary)) {
        clicked_command = hovered_command;
        should_close = true;
    }
    if ctx.input(|i| i.key_pressed(Key::Escape) || i.pointer.button_released(egui::PointerButton::Secondary)) {
        should_close = true;
    }

    egui::Area::new("radial_command_menu".into())
        .fixed_pos(center - egui::vec2(220.0, 220.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_min_size(egui::vec2(440.0, 440.0));
            let painter = ui.painter();
            painter.circle_filled(center, 36.0, Color32::from_rgb(28, 32, 36));
            painter.circle_stroke(center, 36.0, Stroke::new(2.0, Color32::from_rgb(90, 110, 125)));
            painter.text(
                center,
                egui::Align2::CENTER_CENTER,
                "Cmd",
                egui::FontId::proportional(16.0),
                Color32::from_rgb(210, 230, 245),
            );

            for domain in RadialDomain::ALL {
                let base = domain_anchor(center, domain, 92.0);
                let color = if Some(domain) == hovered_domain {
                    Color32::from_rgb(70, 130, 170)
                } else {
                    Color32::from_rgb(50, 66, 80)
                };
                painter.circle_filled(base, 26.0, color);
                painter.text(
                    base,
                    egui::Align2::CENTER_CENTER,
                    domain.label(),
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }

            if let Some(domain) = hovered_domain {
                let cmds = commands_for_domain(domain);
                for (idx, cmd) in cmds.iter().enumerate() {
                    let enabled =
                        is_command_enabled(*cmd, pair_context, any_selected, source_context);
                    let anchor = command_anchor(center, domain, idx, cmds.len());
                    let color = if Some(*cmd) == hovered_command {
                        Color32::from_rgb(80, 170, 215)
                    } else if enabled {
                        Color32::from_rgb(64, 82, 98)
                    } else {
                        Color32::from_rgb(42, 48, 54)
                    };
                    painter.circle_filled(anchor, 22.0, color);
                    painter.text(
                        anchor,
                        egui::Align2::CENTER_CENTER,
                        cmd.label(),
                        egui::FontId::proportional(10.0),
                        if enabled {
                            Color32::from_rgb(230, 240, 248)
                        } else {
                            Color32::from_rgb(120, 125, 130)
                        },
                    );
                }
            }
        });

    if let Some(cmd) = clicked_command {
        execute_radial_command(
            app,
            cmd,
            pair_context,
            any_selected,
            source_context,
            &mut intents,
        );
    }

    app.show_radial_menu = !should_close;
    if !app.show_radial_menu {
        ctx.data_mut(|d| d.remove::<egui::Pos2>(center_id));
    }
    apply_ui_intents_with_checkpoint(app, intents);
}

fn apply_ui_intents_with_checkpoint(app: &mut GraphBrowserApp, intents: Vec<GraphIntent>) {
    if intents.is_empty() {
        return;
    }
    if intents.iter().any(is_user_undoable_intent) {
        let layout = app.load_workspace_layout_json(GraphBrowserApp::SESSION_WORKSPACE_LAYOUT_NAME);
        app.capture_undo_checkpoint(layout);
    }
    app.apply_intents(intents);
}

fn is_user_undoable_intent(intent: &GraphIntent) -> bool {
    matches!(
        intent,
        GraphIntent::CreateNodeNearCenter
            | GraphIntent::CreateNodeAtUrl { .. }
            | GraphIntent::RemoveSelectedNodes
            | GraphIntent::ClearGraph
            | GraphIntent::SetNodePosition { .. }
            | GraphIntent::SetNodeUrl { .. }
            | GraphIntent::CreateUserGroupedEdge { .. }
            | GraphIntent::RemoveEdge { .. }
            | GraphIntent::ExecuteEdgeCommand { .. }
            | GraphIntent::SetNodePinned { .. }
            | GraphIntent::PromoteNodeToActive { .. }
            | GraphIntent::DemoteNodeToCold { .. }
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RadialDomain {
    Node,
    Edge,
    Graph,
    Persistence,
}

impl RadialDomain {
    const ALL: [Self; 4] = [
        Self::Node,
        Self::Edge,
        Self::Graph,
        Self::Persistence,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Edge => "Edge",
            Self::Graph => "Graph",
            Self::Persistence => "Persist",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RadialCommand {
    NodeNew,
    NodePinToggle,
    NodeDelete,
    NodeOpenTab,
    NodeOpenSplit,
    NodeMoveToActivePane,
    EdgeConnectPair,
    EdgeConnectBoth,
    EdgeRemoveUser,
    GraphFit,
    GraphTogglePhysics,
    GraphPhysicsConfig,
    GraphCommandPalette,
    PersistUndo,
    PersistRedo,
    PersistSaveSnapshot,
    PersistRestoreSession,
    PersistSaveGraph,
    PersistRestoreLatestGraph,
    PersistOpenHub,
}

impl RadialCommand {
    fn label(self) -> &'static str {
        match self {
            Self::NodeNew => "New",
            Self::NodePinToggle => "Pin",
            Self::NodeDelete => "Delete",
            Self::NodeOpenTab => "Tab",
            Self::NodeOpenSplit => "Split",
            Self::NodeMoveToActivePane => "Move",
            Self::EdgeConnectPair => "Pair",
            Self::EdgeConnectBoth => "Both",
            Self::EdgeRemoveUser => "Remove",
            Self::GraphFit => "Fit",
            Self::GraphTogglePhysics => "Physics",
            Self::GraphPhysicsConfig => "Config",
            Self::GraphCommandPalette => "Cmd",
            Self::PersistUndo => "Undo",
            Self::PersistRedo => "Redo",
            Self::PersistSaveSnapshot => "Save W",
            Self::PersistRestoreSession => "Restore W",
            Self::PersistSaveGraph => "Save G",
            Self::PersistRestoreLatestGraph => "Latest G",
            Self::PersistOpenHub => "Hub",
        }
    }
}

fn commands_for_domain(domain: RadialDomain) -> &'static [RadialCommand] {
    match domain {
        RadialDomain::Node => &[
            RadialCommand::NodeNew,
            RadialCommand::NodePinToggle,
            RadialCommand::NodeDelete,
            RadialCommand::NodeOpenTab,
            RadialCommand::NodeOpenSplit,
            RadialCommand::NodeMoveToActivePane,
        ],
        RadialDomain::Edge => &[
            RadialCommand::EdgeConnectPair,
            RadialCommand::EdgeConnectBoth,
            RadialCommand::EdgeRemoveUser,
        ],
        RadialDomain::Graph => &[
            RadialCommand::GraphFit,
            RadialCommand::GraphTogglePhysics,
            RadialCommand::GraphPhysicsConfig,
            RadialCommand::GraphCommandPalette,
        ],
        RadialDomain::Persistence => &[
            RadialCommand::PersistUndo,
            RadialCommand::PersistRedo,
            RadialCommand::PersistSaveSnapshot,
            RadialCommand::PersistRestoreSession,
            RadialCommand::PersistSaveGraph,
            RadialCommand::PersistRestoreLatestGraph,
            RadialCommand::PersistOpenHub,
        ],
    }
}

fn is_command_enabled(
    command: RadialCommand,
    pair_context: Option<(NodeKey, NodeKey)>,
    any_selected: bool,
    source_context: Option<NodeKey>,
) -> bool {
    match command {
        RadialCommand::NodePinToggle
        | RadialCommand::NodeDelete
        | RadialCommand::NodeOpenTab
        | RadialCommand::NodeOpenSplit
        | RadialCommand::NodeMoveToActivePane => any_selected || source_context.is_some(),
        RadialCommand::EdgeConnectPair
        | RadialCommand::EdgeConnectBoth
        | RadialCommand::EdgeRemoveUser => pair_context.is_some(),
        _ => true,
    }
}

fn execute_radial_command(
    app: &mut GraphBrowserApp,
    command: RadialCommand,
    pair_context: Option<(NodeKey, NodeKey)>,
    any_selected: bool,
    source_context: Option<NodeKey>,
    intents: &mut Vec<GraphIntent>,
) {
    if !is_command_enabled(command, pair_context, any_selected, source_context) {
        return;
    }

    match command {
        RadialCommand::NodeNew => intents.push(GraphIntent::CreateNodeNearCenter),
        RadialCommand::NodePinToggle => {
            if app.selected_nodes.iter().copied().all(|key| {
                app.graph
                    .get_node(key)
                    .is_some_and(|node| node.is_pinned)
            }) {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::UnpinSelected,
                });
            } else {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::PinSelected,
                });
            }
        },
        RadialCommand::NodeDelete => intents.push(GraphIntent::RemoveSelectedNodes),
        RadialCommand::NodeOpenTab => {
            app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
        },
        RadialCommand::NodeOpenSplit => {
            app.request_open_selected_tile_mode(PendingTileOpenMode::SplitHorizontal);
        },
        RadialCommand::NodeMoveToActivePane => {
            app.request_open_selected_tile_mode(PendingTileOpenMode::Tab);
        },
        RadialCommand::EdgeConnectPair => {
            if let Some((from, to)) = pair_context {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::ConnectPair { from, to },
                });
            }
        },
        RadialCommand::EdgeConnectBoth => {
            if let Some((a, b)) = pair_context {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::ConnectBothDirectionsPair { a, b },
                });
            }
        },
        RadialCommand::EdgeRemoveUser => {
            if let Some((a, b)) = pair_context {
                intents.push(GraphIntent::ExecuteEdgeCommand {
                    command: EdgeCommand::RemoveUserEdgePair { a, b },
                });
            }
        },
        RadialCommand::GraphFit => intents.push(GraphIntent::RequestFitToScreen),
        RadialCommand::GraphTogglePhysics => intents.push(GraphIntent::TogglePhysics),
        RadialCommand::GraphPhysicsConfig => intents.push(GraphIntent::TogglePhysicsPanel),
        RadialCommand::GraphCommandPalette => intents.push(GraphIntent::ToggleCommandPalette),
        RadialCommand::PersistUndo => intents.push(GraphIntent::Undo),
        RadialCommand::PersistRedo => intents.push(GraphIntent::Redo),
        RadialCommand::PersistSaveSnapshot => app.request_save_workspace_snapshot(),
        RadialCommand::PersistRestoreSession => {
            app.request_restore_workspace_snapshot_named(
                GraphBrowserApp::SESSION_WORKSPACE_LAYOUT_NAME.to_string(),
            );
        },
        RadialCommand::PersistSaveGraph => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            app.request_save_graph_snapshot_named(format!("radial-graph-{now}"));
        },
        RadialCommand::PersistRestoreLatestGraph => app.request_restore_graph_snapshot_latest(),
        RadialCommand::PersistOpenHub => intents.push(GraphIntent::TogglePersistencePanel),
    }
}

fn domain_from_angle(angle: f32) -> RadialDomain {
    let mut best = RadialDomain::Node;
    let mut best_dist = f32::MAX;
    for domain in RadialDomain::ALL {
        let target = domain_angle(domain);
        let mut d = (angle - target).abs();
        if d > std::f32::consts::PI {
            d = 2.0 * std::f32::consts::PI - d;
        }
        if d < best_dist {
            best_dist = d;
            best = domain;
        }
    }
    best
}

fn domain_angle(domain: RadialDomain) -> f32 {
    match domain {
        RadialDomain::Node => -std::f32::consts::FRAC_PI_2,
        RadialDomain::Edge => -0.25,
        RadialDomain::Graph => 1.45,
        RadialDomain::Persistence => 2.7,
    }
}

fn domain_anchor(center: egui::Pos2, domain: RadialDomain, radius: f32) -> egui::Pos2 {
    let a = domain_angle(domain);
    center + egui::vec2(a.cos() * radius, a.sin() * radius)
}

fn command_anchor(
    center: egui::Pos2,
    domain: RadialDomain,
    idx: usize,
    len: usize,
) -> egui::Pos2 {
    let base = domain_angle(domain);
    let spread = 0.8_f32;
    let t = if len <= 1 {
        0.0
    } else {
        idx as f32 / (len.saturating_sub(1) as f32) - 0.5
    };
    let angle = base + t * spread;
    center + egui::vec2(angle.cos() * 165.0, angle.sin() * 165.0)
}

fn nearest_command_for_pointer(
    domain: RadialDomain,
    center: egui::Pos2,
    pointer: egui::Pos2,
    pair_context: Option<(NodeKey, NodeKey)>,
    any_selected: bool,
    source_context: Option<NodeKey>,
) -> Option<RadialCommand> {
    let cmds = commands_for_domain(domain);
    let mut best: Option<(f32, RadialCommand)> = None;
    for (idx, cmd) in cmds.iter().enumerate() {
        if !is_command_enabled(*cmd, pair_context, any_selected, source_context) {
            continue;
        }
        let anchor = command_anchor(center, domain, idx, cmds.len());
        let d = (pointer - anchor).length_sq();
        match best {
            Some((best_d, _)) if d >= best_d => {},
            _ => best = Some((d, *cmd)),
        }
    }
    best.map(|(_, cmd)| cmd)
}

pub fn render_persistence_panel(ctx: &egui::Context, app: &mut GraphBrowserApp) {
    if !app.show_persistence_panel {
        return;
    }

    let mut open = app.show_persistence_panel;
    Window::new("Persistence Hub")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label("Workspaces");
            ui.horizontal(|ui| {
                ui.label("Autosave every (sec):");
                let autosave_interval_id = ui.make_persistent_id("workspace_autosave_interval_input");
                let mut autosave_interval = ui
                    .data_mut(|d| d.get_persisted::<String>(autosave_interval_id))
                    .unwrap_or_else(|| app.workspace_autosave_interval_secs().to_string());
                if ui
                    .add(egui::TextEdit::singleline(&mut autosave_interval).desired_width(72.0))
                    .changed()
                {
                    ui.data_mut(|d| d.insert_persisted(autosave_interval_id, autosave_interval.clone()));
                }
                if ui.button("Apply").clicked()
                    && let Ok(secs) = autosave_interval.trim().parse::<u64>()
                {
                    let _ = app.set_workspace_autosave_interval_secs(secs);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Autosave retention:");
                let mut retention = app.workspace_autosave_retention() as u32;
                if ui
                    .add(egui::Slider::new(&mut retention, 0..=5).suffix(" previous"))
                    .changed()
                {
                    let _ = app.set_workspace_autosave_retention(retention as u8);
                }
            });
            if ui.button("Pin Workspace Snapshot").clicked() {
                app.request_save_workspace_snapshot();
            }
            if ui.button("Prune Session Workspace").clicked() {
                let _ = app.clear_session_workspace_layout();
            }
            ui.separator();
            let workspace_name_id = ui.make_persistent_id("workspace_name_input");
            let mut workspace_name = ui
                .data_mut(|d| d.get_persisted::<String>(workspace_name_id))
                .unwrap_or_default();
            let workspace_name_changed = ui
                .add(
                    egui::TextEdit::singleline(&mut workspace_name)
                        .hint_text("workspace name (e.g. research-1)"),
                )
                .changed();
            if workspace_name_changed {
                ui.data_mut(|d| d.insert_persisted(workspace_name_id, workspace_name.clone()));
            }
            let workspace_name = workspace_name.trim().to_string();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!workspace_name.is_empty(), egui::Button::new("Save Named"))
                    .clicked()
                {
                    app.request_save_workspace_snapshot_named(workspace_name.clone());
                }
                if ui
                    .add_enabled(!workspace_name.is_empty(), egui::Button::new("Restore Named"))
                    .clicked()
                {
                    app.request_restore_workspace_snapshot_named(workspace_name.clone());
                }
                if ui
                    .add_enabled(!workspace_name.is_empty(), egui::Button::new("Delete Named"))
                    .clicked()
                {
                    if !GraphBrowserApp::is_reserved_workspace_layout_name(&workspace_name) {
                        let _ = app.delete_workspace_layout(&workspace_name);
                    }
                }
            });
            let mut workspace_names = app.list_workspace_layout_names();
            workspace_names.sort();
            if workspace_names.is_empty() {
                ui.small("No workspaces saved.");
            } else {
                ui.small("Saved:");
                for name in workspace_names {
                    let is_reserved = GraphBrowserApp::is_reserved_workspace_layout_name(&name);
                    let label = if name == GraphBrowserApp::SESSION_WORKSPACE_LAYOUT_NAME {
                        "session-latest (autosave)"
                    } else if name == "latest" {
                        "latest (autosave)"
                    } else if let Some(idx) = name.strip_prefix("workspace:session-prev-") {
                        if idx.chars().all(|c| c.is_ascii_digit()) {
                            "session-previous (autosave)"
                        } else {
                            &name
                        }
                    } else {
                        &name
                    };
                    ui.horizontal(|ui| {
                        if ui.button(label).clicked() {
                            app.request_restore_workspace_snapshot_named(name.clone());
                        }
                        if ui.small_button("Load").clicked() {
                            app.request_restore_workspace_snapshot_named(name.clone());
                        }
                        if ui
                            .add_enabled(!is_reserved, egui::Button::new("Del").small())
                            .clicked()
                        {
                            let _ = app.delete_workspace_layout(&name);
                        }
                    });
                }
            }

            ui.separator();
            ui.label("Graphs");
            let graph_name_id = ui.make_persistent_id("graph_name_input");
            let mut graph_name = ui
                .data_mut(|d| d.get_persisted::<String>(graph_name_id))
                .unwrap_or_default();
            let graph_name_changed = ui
                .add(
                    egui::TextEdit::singleline(&mut graph_name)
                        .hint_text("graph snapshot name (e.g. ideation-v1)"),
                )
                .changed();
            if graph_name_changed {
                ui.data_mut(|d| d.insert_persisted(graph_name_id, graph_name.clone()));
            }
            let graph_name = graph_name.trim().to_string();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!graph_name.is_empty(), egui::Button::new("Save Graph"))
                    .clicked()
                {
                    app.request_save_graph_snapshot_named(graph_name.clone());
                }
                if ui
                    .add_enabled(!graph_name.is_empty(), egui::Button::new("Load Graph"))
                    .clicked()
                {
                    app.request_restore_graph_snapshot_named(graph_name.clone());
                }
                if ui
                    .add_enabled(!graph_name.is_empty(), egui::Button::new("Delete Graph"))
                    .clicked()
                {
                    app.request_delete_graph_snapshot_named(graph_name.clone());
                }
            });
            let mut named_graphs = app.list_named_graph_snapshot_names();
            named_graphs.sort();
            let has_latest_graph = app.has_latest_graph_snapshot();
            if named_graphs.is_empty() && !has_latest_graph {
                ui.small("No graph snapshots saved.");
            } else {
                ui.small("Saved:");
                if has_latest_graph {
                    ui.horizontal(|ui| {
                        if ui.button("latest (autosave)").clicked() {
                            app.request_restore_graph_snapshot_latest();
                        }
                        if ui.small_button("Load").clicked() {
                            app.request_restore_graph_snapshot_latest();
                        }
                        ui.add_enabled(false, egui::Button::new("Del").small());
                    });
                }
                for name in named_graphs {
                    ui.horizontal(|ui| {
                        if ui.button(&name).clicked() {
                            app.request_restore_graph_snapshot_named(name.clone());
                        }
                        if ui.small_button("Load").clicked() {
                            app.request_restore_graph_snapshot_named(name.clone());
                        }
                        if ui.small_button("Del").clicked() {
                            app.request_delete_graph_snapshot_named(name.clone());
                        }
                    });
                }
            }
        });
    app.show_persistence_panel = open;
}

/// Resolve pair edge command context using precedence:
/// selected pair > (selected primary + hovered node) > (selected primary + focused pane node).
fn resolve_pair_command_context(
    app: &GraphBrowserApp,
    hovered_node: Option<NodeKey>,
    focused_pane_node: Option<NodeKey>,
) -> Option<(NodeKey, NodeKey)> {
    if let Some((from, to)) = app.selected_nodes.ordered_pair() {
        return Some((from, to));
    }

    if app.selected_nodes.len() == 1 {
        let from = app.selected_nodes.primary()?;
        if let Some(to) = hovered_node.filter(|to| *to != from) {
            return Some((from, to));
        }
        if let Some(to) = focused_pane_node.filter(|to| *to != from) {
            return Some((from, to));
        }
    }

    None
}

fn resolve_source_node_context(
    app: &GraphBrowserApp,
    hovered_node: Option<NodeKey>,
    focused_pane_node: Option<NodeKey>,
) -> Option<NodeKey> {
    app.selected_nodes
        .primary()
        .or(hovered_node)
        .or(focused_pane_node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> GraphBrowserApp {
        GraphBrowserApp::new_for_testing()
    }

    #[test]
    fn test_focus_node_action() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents = intents_from_graph_actions(vec![GraphAction::FocusNode(key)]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&key));
    }

    #[test]
    fn test_drag_start_sets_interacting() {
        let mut app = test_app();
        assert!(!app.is_interacting);

        let intents = intents_from_graph_actions(vec![GraphAction::DragStart]);
        app.apply_intents(intents);

        assert!(app.is_interacting);
    }

    #[test]
    fn test_drag_end_clears_interacting_and_updates_position() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));
        app.set_interacting(true);

        let intents =
            intents_from_graph_actions(vec![GraphAction::DragEnd(key, Point2D::new(150.0, 250.0))]);
        app.apply_intents(intents);

        assert!(!app.is_interacting);
        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.position, Point2D::new(150.0, 250.0));
    }

    #[test]
    fn test_move_node_updates_position() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents =
            intents_from_graph_actions(vec![GraphAction::MoveNode(key, Point2D::new(42.0, 84.0))]);
        app.apply_intents(intents);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.position, Point2D::new(42.0, 84.0));
    }

    #[test]
    fn test_select_node_action() {
        let mut app = test_app();
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let intents = intents_from_graph_actions(vec![GraphAction::SelectNode {
            key,
            multi_select: false,
        }]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&key));
    }

    #[test]
    fn test_zoom_action_clamps() {
        let mut app = test_app();

        let intents = intents_from_graph_actions(vec![GraphAction::Zoom(0.01)]);
        app.apply_intents(intents);

        // Should be clamped to min zoom
        assert!(app.camera.current_zoom >= app.camera.zoom_min);
    }

    #[test]
    fn test_multiple_actions_sequence() {
        let mut app = test_app();
        let k1 = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let k2 = app.add_node_and_sync("b".into(), Point2D::new(100.0, 100.0));

        let intents = intents_from_graph_actions(vec![
            GraphAction::SelectNode {
                key: k1,
                multi_select: false,
            },
            GraphAction::MoveNode(k2, Point2D::new(200.0, 300.0)),
            GraphAction::Zoom(1.5),
        ]);
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&k1));
        assert_eq!(
            app.graph.get_node(k2).unwrap().position,
            Point2D::new(200.0, 300.0)
        );
        assert!((app.camera.current_zoom - 1.5).abs() < 0.01);
    }

    #[test]
    fn test_empty_actions_is_noop() {
        let mut app = test_app();
        let key = app.add_node_and_sync("a".into(), Point2D::new(50.0, 60.0));
        let pos_before = app.graph.get_node(key).unwrap().position;

        let intents = intents_from_graph_actions(vec![]);
        app.apply_intents(intents);

        assert_eq!(app.graph.get_node(key).unwrap().position, pos_before);
    }

    #[test]
    fn test_pair_command_context_prefers_selected_pair() {
        let mut app = test_app();
        let a = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("b".into(), Point2D::new(100.0, 0.0));
        app.select_node(b, false);
        app.select_node(a, true);

        let resolved = resolve_pair_command_context(&app, Some(b), Some(b));
        assert_eq!(resolved, Some((b, a)));
    }

    #[test]
    fn test_pair_command_context_falls_back_to_focused_node() {
        let mut app = test_app();
        let a = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("b".into(), Point2D::new(100.0, 0.0));
        app.select_node(a, false);

        let resolved = resolve_pair_command_context(&app, None, Some(b));
        assert_eq!(resolved, Some((a, b)));
    }

    #[test]
    fn test_pair_command_context_prefers_hover_over_focused_for_single_select() {
        let mut app = test_app();
        let a = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let hovered = app.add_node_and_sync("hovered".into(), Point2D::new(100.0, 0.0));
        let focused = app.add_node_and_sync("focused".into(), Point2D::new(200.0, 0.0));
        app.select_node(a, false);

        let resolved = resolve_pair_command_context(&app, Some(hovered), Some(focused));
        assert_eq!(resolved, Some((a, hovered)));
    }

    #[test]
    fn test_source_context_prefers_selected_then_hover_then_focused() {
        let mut app = test_app();
        let selected = app.add_node_and_sync("selected".into(), Point2D::new(0.0, 0.0));
        let hovered = app.add_node_and_sync("hovered".into(), Point2D::new(10.0, 0.0));
        let focused = app.add_node_and_sync("focused".into(), Point2D::new(20.0, 0.0));

        app.select_node(selected, false);
        assert_eq!(
            resolve_source_node_context(&app, Some(hovered), Some(focused)),
            Some(selected)
        );
        app.selected_nodes.clear();
        assert_eq!(
            resolve_source_node_context(&app, Some(hovered), Some(focused)),
            Some(hovered)
        );
        assert_eq!(
            resolve_source_node_context(&app, None, Some(focused)),
            Some(focused)
        );
    }
}
