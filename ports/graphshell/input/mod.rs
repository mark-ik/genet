/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Input handling for the graph browser.
//!
//! Keyboard shortcuts are handled here. Mouse interaction (drag, pan, zoom,
//! selection) is handled by egui_graphs via the GraphView widget.

use crate::app::{
    CommandPaletteShortcut, EdgeCommand, GraphBrowserApp, GraphIntent, HelpPanelShortcut,
    RadialMenuShortcut,
};
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
    pub toggle_pin_primary: bool,
    pub zoom_in: bool,
    pub zoom_out: bool,
    pub zoom_reset: bool,
    pub zoom_to_selected: bool,
    pub reheat_physics: bool,
    pub delete_selected: bool,
    pub clear_graph: bool,
    pub select_all: bool,
    pub undo: bool,
    pub redo: bool,
}

/// Collect keyboard actions from the egui context (input detection only).
pub(crate) fn collect_actions(ctx: &egui::Context, graph_app: &GraphBrowserApp) -> KeyboardActions {
    // Don't handle shortcuts while egui is actively capturing keyboard input
    // (for example, URL bar text editing).
    let keyboard_captured_by_egui = ctx.wants_keyboard_input();
    let mut actions = KeyboardActions::default();

    ctx.input(|i| {
        // Escape always works: unfocus text field or toggle view
        if i.key_pressed(Key::Escape) {
            if keyboard_captured_by_egui {
                // Escape will unfocus the text field (handled by egui)
                return;
            }
            actions.toggle_view = true;
        }

        // Home: Toggle view (always works)
        if i.key_pressed(Key::Home) {
            actions.toggle_view = true;
        }

        // Skip remaining shortcuts while egui is consuming keyboard input.
        if keyboard_captured_by_egui {
            return;
        }

        // T: Toggle physics
        if i.key_pressed(Key::T) {
            actions.toggle_physics = true;
        }

        // + / - / 0: keyboard zoom controls
        if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
            actions.zoom_in = true;
        }
        if i.key_pressed(Key::Minus) {
            actions.zoom_out = true;
        }
        if i.key_pressed(Key::Num0) {
            actions.zoom_reset = true;
        }

        // P: Toggle physics config panel
        if i.key_pressed(Key::P) {
            actions.toggle_physics_panel = true;
        }

        // N: Create new node
        if i.key_pressed(Key::N) {
            actions.create_node = true;
        }

        // Z: zoom to selected (without Ctrl modifier)
        if i.key_pressed(Key::Z) && !i.modifiers.ctrl {
            actions.zoom_to_selected = true;
        }

        // R: manual physics reheat (no modifiers).
        if i.key_pressed(Key::R)
            && !i.modifiers.ctrl
            && !i.modifiers.shift
            && !i.modifiers.alt
            && !i.modifiers.command
            && !matches!(graph_app.radial_menu_shortcut, RadialMenuShortcut::R)
        {
            actions.reheat_physics = true;
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

        // L: toggle pin state on primary selected node
        if i.key_pressed(Key::L)
            && !i.modifiers.ctrl
            && !i.modifiers.shift
            && !i.modifiers.alt
            && !i.modifiers.command
        {
            actions.toggle_pin_primary = true;
        }

        // Toggle keyboard shortcut help panel.
        match graph_app.help_panel_shortcut {
            HelpPanelShortcut::F1OrQuestion => {
                if i.key_pressed(Key::F1) || i.key_pressed(Key::Questionmark) {
                    actions.toggle_help_panel = true;
                }
            },
            HelpPanelShortcut::H => {
                if i.key_pressed(Key::H) {
                    actions.toggle_help_panel = true;
                }
            },
        }

        // Toggle edge command palette.
        match graph_app.command_palette_shortcut {
            CommandPaletteShortcut::F2 => {
                if i.key_pressed(Key::F2) {
                    actions.toggle_command_palette = true;
                }
            },
            CommandPaletteShortcut::CtrlK => {
                if i.modifiers.ctrl && i.key_pressed(Key::K) {
                    actions.toggle_command_palette = true;
                }
            },
        }

        // Toggle radial command menu.
        match graph_app.radial_menu_shortcut {
            RadialMenuShortcut::F3 => {
                if i.key_pressed(Key::F3) {
                    actions.toggle_radial_menu = true;
                }
            },
            RadialMenuShortcut::R => {
                if i.key_pressed(Key::R) {
                    actions.toggle_radial_menu = true;
                }
            },
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

        // Ctrl+A: select all nodes
        if i.modifiers.ctrl && i.key_pressed(Key::A) {
            actions.select_all = true;
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
    if actions.zoom_in {
        intents.push(GraphIntent::RequestZoomIn);
    }
    if actions.zoom_out {
        intents.push(GraphIntent::RequestZoomOut);
    }
    if actions.zoom_reset {
        intents.push(GraphIntent::RequestZoomReset);
    }
    if actions.zoom_to_selected {
        intents.push(GraphIntent::RequestZoomToSelected);
    }
    if actions.reheat_physics {
        intents.push(GraphIntent::ReheatPhysics);
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
    if actions.toggle_pin_primary {
        intents.push(GraphIntent::TogglePrimaryNodePin);
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
    if actions.select_all {
        intents.push(GraphIntent::SelectAll);
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
    fn test_zoom_in_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            zoom_in: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::RequestZoomIn))
        );
    }

    #[test]
    fn test_zoom_out_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            zoom_out: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::RequestZoomOut))
        );
    }

    #[test]
    fn test_zoom_reset_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            zoom_reset: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::RequestZoomReset))
        );
    }

    #[test]
    fn test_zoom_to_selected_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            zoom_to_selected: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::RequestZoomToSelected))
        );
    }

    #[test]
    fn test_reheat_physics_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            reheat_physics: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::ReheatPhysics))
        );
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
    fn test_toggle_pin_primary_action_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            toggle_pin_primary: true,
            ..Default::default()
        });
        assert!(
            intents
                .iter()
                .any(|i| matches!(i, GraphIntent::TogglePrimaryNodePin))
        );
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

    #[test]
    fn test_select_all_action_selects_every_node() {
        let mut app = test_app();
        use euclid::default::Point2D;
        let a = app.add_node_and_sync("a".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("b".into(), Point2D::new(10.0, 0.0));
        let c = app.add_node_and_sync("c".into(), Point2D::new(20.0, 0.0));

        let intents = intents_from_actions(&KeyboardActions {
            select_all: true,
            ..Default::default()
        });
        app.apply_intents(intents);

        assert!(app.selected_nodes.contains(&a));
        assert!(app.selected_nodes.contains(&b));
        assert!(app.selected_nodes.contains(&c));
        assert_eq!(app.selected_nodes.len(), 3);
    }

    #[test]
    fn test_select_all_maps_to_intent() {
        let intents = intents_from_actions(&KeyboardActions {
            select_all: true,
            ..Default::default()
        });
        assert!(intents.iter().any(|i| matches!(i, GraphIntent::SelectAll)));
    }
}
