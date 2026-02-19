/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Input handling for the graph browser.
//!
//! Keyboard shortcuts are handled here. Mouse interaction (drag, pan, zoom,
//! selection) is handled by egui_graphs via the GraphView widget.

use crate::app::{EdgeCommand, GraphIntent};
use egui::Key;

/// Keyboard actions collected from egui input events.
///
/// This struct decouples input detection (requires `egui::Context`) from
/// action application (pure state mutation), making actions testable.
#[derive(Default)]
pub struct KeyboardActions {
    pub toggle_physics: bool,
    pub toggle_view: bool,
    pub fit_to_screen: bool,
    pub toggle_physics_panel: bool,
    pub toggle_help_panel: bool,
    pub toggle_command_palette: bool,
    pub toggle_radial_menu: bool,
    pub create_node: bool,
    pub connect_selected_pair: bool,
    pub connect_both_directions: bool,
    pub remove_user_edge: bool,
    pub pin_selected: bool,
    pub unpin_selected: bool,
    pub delete_selected: bool,
    pub clear_graph: bool,
    pub undo: bool,
    pub redo: bool,
}

/// Collect keyboard actions from the egui context (input detection only).
pub(crate) fn collect_actions(ctx: &egui::Context) -> KeyboardActions {
    // Don't handle shortcuts when a text field (e.g., URL bar) has focus
    let text_field_focused = ctx.memory(|m| m.focused().is_some());
    let mut actions = KeyboardActions::default();

    ctx.input(|i| {
        // Escape always works: unfocus text field or toggle view
        if i.key_pressed(Key::Escape) {
            if text_field_focused {
                // Escape will unfocus the text field (handled by egui)
                return;
            }
            actions.toggle_view = true;
        }

        // Home: Toggle view (always works)
        if i.key_pressed(Key::Home) {
            actions.toggle_view = true;
        }

        // Skip remaining shortcuts if a text field is focused
        if text_field_focused {
            return;
        }

        // T: Toggle physics
        if i.key_pressed(Key::T) {
            actions.toggle_physics = true;
        }

        // C: Fit graph to screen
        if i.key_pressed(Key::C) {
            actions.fit_to_screen = true;
        }

        // P: Toggle physics config panel
        if i.key_pressed(Key::P) {
            actions.toggle_physics_panel = true;
        }

        // N: Create new node
        if i.key_pressed(Key::N) {
            actions.create_node = true;
        }

        // G: connect selected pair, Shift+G: connect both directions, Alt+G: remove user edge
        if i.key_pressed(Key::G) {
            if i.modifiers.shift {
                actions.connect_both_directions = true;
            } else if i.modifiers.alt {
                actions.remove_user_edge = true;
            } else {
                actions.connect_selected_pair = true;
            }
        }

        // I: pin selected node(s)
        if i.key_pressed(Key::I) {
            actions.pin_selected = true;
        }

        // U: unpin selected node(s)
        if i.key_pressed(Key::U) {
            actions.unpin_selected = true;
        }

        // F1 or ?: Toggle keyboard shortcut help panel
        if i.key_pressed(Key::F1) || i.key_pressed(Key::Questionmark) {
            actions.toggle_help_panel = true;
        }

        // F2: Toggle edge command palette
        if i.key_pressed(Key::F2) {
            actions.toggle_command_palette = true;
        }

        // F3: Toggle radial command menu.
        if i.key_pressed(Key::F3) {
            actions.toggle_radial_menu = true;
        }

        // Ctrl+Shift+Delete: Clear entire graph
        // Delete (no modifiers): Remove selected nodes
        if i.key_pressed(Key::Delete) {
            if i.modifiers.ctrl && i.modifiers.shift {
                actions.clear_graph = true;
            } else if !i.modifiers.ctrl && !i.modifiers.shift {
                actions.delete_selected = true;
            }
        }

        if i.modifiers.ctrl && i.key_pressed(Key::Z) {
            if i.modifiers.shift {
                actions.redo = true;
            } else {
                actions.undo = true;
            }
        }
        if i.modifiers.ctrl && i.key_pressed(Key::Y) {
            actions.redo = true;
        }
    });

    actions
}

/// Convert keyboard actions to graph intents without applying them.
pub fn intents_from_actions(actions: &KeyboardActions) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    if actions.toggle_physics {
        intents.push(GraphIntent::TogglePhysics);
    }
    // View toggling is owned by GUI tile logic.
    if actions.fit_to_screen {
        intents.push(GraphIntent::RequestFitToScreen);
    }
    if actions.toggle_physics_panel {
        intents.push(GraphIntent::TogglePhysicsPanel);
    }
    if actions.toggle_help_panel {
        intents.push(GraphIntent::ToggleHelpPanel);
    }
    if actions.toggle_command_palette {
        intents.push(GraphIntent::ToggleCommandPalette);
    }
    if actions.toggle_radial_menu {
        intents.push(GraphIntent::ToggleRadialMenu);
    }
    if actions.create_node {
        intents.push(GraphIntent::CreateNodeNearCenter);
    }
    if actions.connect_selected_pair {
        intents.push(GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::ConnectSelectedPair,
        });
    }
    if actions.connect_both_directions {
        intents.push(GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::ConnectBothDirections,
        });
    }
    if actions.remove_user_edge {
        intents.push(GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::RemoveUserEdge,
        });
    }
    if actions.pin_selected {
        intents.push(GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::PinSelected,
        });
    }
    if actions.unpin_selected {
        intents.push(GraphIntent::ExecuteEdgeCommand {
            command: EdgeCommand::UnpinSelected,
        });
    }
    if actions.delete_selected {
        intents.push(GraphIntent::RemoveSelectedNodes);
    }
    if actions.clear_graph {
        intents.push(GraphIntent::ClearGraph);
    }
    if actions.undo {
        intents.push(GraphIntent::Undo);
    }
    if actions.redo {
        intents.push(GraphIntent::Redo);
    }
    intents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::GraphBrowserApp;

    fn test_app() -> GraphBrowserApp {
        GraphBrowserApp::new_for_testing()
    }

    #[test]
    fn test_toggle_view_action_is_gui_owned() {
        let mut app = test_app();
        use euclid::default::Point2D;
        app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));
        let selected_before = app.selected_nodes.clone();
        let count_before = app.graph.node_count();

        let intents = intents_from_actions(&KeyboardActions {
            toggle_view: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_eq!(app.selected_nodes, selected_before);
        assert_eq!(app.graph.node_count(), count_before);
    }

    #[test]
    fn test_toggle_physics_action() {
        let mut app = test_app();
        let was_running = app.physics.base.is_running;

        let intents = intents_from_actions(&KeyboardActions {
            toggle_physics: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_ne!(app.physics.base.is_running, was_running);
    }

    #[test]
    fn test_fit_to_screen_action() {
        let mut app = test_app();
        assert!(!app.fit_to_screen_requested);

        let intents = intents_from_actions(&KeyboardActions {
            fit_to_screen: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(app.fit_to_screen_requested);
    }

    #[test]
    fn test_toggle_physics_panel_action() {
        let mut app = test_app();
        let was_shown = app.show_physics_panel;

        let intents = intents_from_actions(&KeyboardActions {
            toggle_physics_panel: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_ne!(app.show_physics_panel, was_shown);
    }

    #[test]
    fn test_toggle_help_panel_action() {
        let mut app = test_app();
        assert!(!app.show_help_panel);

        let intents = intents_from_actions(&KeyboardActions {
            toggle_help_panel: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(app.show_help_panel);

        let intents = intents_from_actions(&KeyboardActions {
            toggle_help_panel: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(!app.show_help_panel);
    }

    #[test]
    fn test_toggle_command_palette_action() {
        let mut app = test_app();
        assert!(!app.show_command_palette);

        let intents = intents_from_actions(&KeyboardActions {
            toggle_command_palette: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(app.show_command_palette);

        let intents = intents_from_actions(&KeyboardActions {
            toggle_command_palette: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(!app.show_command_palette);
    }

    #[test]
    fn test_create_node_action() {
        let mut app = test_app();
        assert_eq!(app.graph.node_count(), 0);

        let intents = intents_from_actions(&KeyboardActions {
            create_node: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_eq!(app.graph.node_count(), 1);
    }

    #[test]
    fn test_connect_selected_pair_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            connect_selected_pair: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(
            i,
            GraphIntent::ExecuteEdgeCommand {
                command: EdgeCommand::ConnectSelectedPair
            }
        )));
    }

    #[test]
    fn test_connect_both_directions_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            connect_both_directions: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(
            i,
            GraphIntent::ExecuteEdgeCommand {
                command: EdgeCommand::ConnectBothDirections
            }
        )));
    }

    #[test]
    fn test_remove_user_edge_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            remove_user_edge: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(
            i,
            GraphIntent::ExecuteEdgeCommand {
                command: EdgeCommand::RemoveUserEdge
            }
        )));
    }

    #[test]
    fn test_pin_selected_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            pin_selected: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(
            i,
            GraphIntent::ExecuteEdgeCommand {
                command: EdgeCommand::PinSelected
            }
        )));
    }

    #[test]
    fn test_unpin_selected_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            unpin_selected: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(
            i,
            GraphIntent::ExecuteEdgeCommand {
                command: EdgeCommand::UnpinSelected
            }
        )));
    }

    #[test]
    fn test_delete_selected_action() {
        let mut app = test_app();
        use euclid::default::Point2D;
        let key = app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);
        assert_eq!(app.graph.node_count(), 1);

        let intents = intents_from_actions(&KeyboardActions {
            delete_selected: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_eq!(app.graph.node_count(), 0);
    }

    #[test]
    fn test_clear_graph_action() {
        let mut app = test_app();
        use euclid::default::Point2D;
        app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        app.add_node_and_sync("b".into(), Point2D::new(100.0, 0.0));
        assert_eq!(app.graph.node_count(), 2);

        let intents = intents_from_actions(&KeyboardActions {
            clear_graph: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert_eq!(app.graph.node_count(), 0);
    }

    #[test]
    fn test_no_actions_is_noop() {
        let mut app = test_app();
        use euclid::default::Point2D;
        app.add_node_and_sync("https://example.com".into(), Point2D::new(0.0, 0.0));

        let before_count = app.graph.node_count();
        let before_physics = app.physics.base.is_running;

        let intents = intents_from_actions(&KeyboardActions::default());
        app.apply_intents(intents);

        assert_eq!(app.graph.node_count(), before_count);
        assert_eq!(app.physics.base.is_running, before_physics);
    }
}
