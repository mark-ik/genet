/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
use egui::{Key, Modifiers, TopBottomPanel, Vec2, WidgetInfo, WidgetType};
use winit::window::Window;

use super::toolbar_routing::{self, ToolbarNavAction, ToolbarOpenMode};
use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::NodeKey;
use crate::running_app_state::{RunningAppState, UserInterfaceCommand};
use crate::window::ServoShellWindow;

pub(crate) struct ToolbarUiArgs<'a> {
    pub ctx: &'a egui::Context,
    pub winit_window: &'a Window,
    pub state: &'a RunningAppState,
    pub graph_app: &'a mut GraphBrowserApp,
    pub window: &'a ServoShellWindow,
    pub focused_toolbar_node: Option<NodeKey>,
    pub has_webview_tiles: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub location: &'a mut String,
    pub location_dirty: &'a mut bool,
    pub location_submitted: &'a mut bool,
    pub focus_location_field_for_search: bool,
    pub show_data_dir_dialog: &'a mut bool,
    pub show_persistence_settings_dialog: &'a mut bool,
    pub show_clear_data_confirm: &'a mut bool,
    pub frame_intents: &'a mut Vec<GraphIntent>,
}

pub(crate) struct ToolbarUiOutput {
    pub toggle_tile_view_requested: bool,
    pub open_selected_mode_after_submit: Option<ToolbarOpenMode>,
    pub toolbar_visible: bool,
}

fn toolbar_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(text)
        .frame(false)
        .min_size(Vec2 { x: 20.0, y: 20.0 })
}

pub(crate) fn render_toolbar_ui(args: ToolbarUiArgs<'_>) -> ToolbarUiOutput {
    let ToolbarUiArgs {
        ctx,
        winit_window,
        state,
        graph_app,
        window,
        focused_toolbar_node,
        has_webview_tiles,
        can_go_back,
        can_go_forward,
        location,
        location_dirty,
        location_submitted,
        focus_location_field_for_search,
        show_data_dir_dialog,
        show_persistence_settings_dialog,
        show_clear_data_confirm,
        frame_intents,
    } = args;

    if winit_window.fullscreen().is_some() {
        let fullscreen_url = focused_toolbar_node
            .and_then(|key| graph_app.graph.get_node(key).map(|node| node.url.clone()))
            .unwrap_or_else(|| "about:blank".to_string());
        let frame = egui::Frame::default()
            .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 25, 220))
            .inner_margin(4.0);
        TopBottomPanel::top("fullscreen_origin_strip")
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fullscreen");
                    ui.separator();
                    ui.label(fullscreen_url);
                    ui.separator();
                    ui.label("Press Esc to exit");
                });
            });
        return ToolbarUiOutput {
            toggle_tile_view_requested: false,
            open_selected_mode_after_submit: None,
            toolbar_visible: false,
        };
    }

    let mut toggle_tile_view_requested = false;
    let mut open_selected_mode_after_submit = None;
    let is_graph_view = !has_webview_tiles;

    let frame = egui::Frame::default()
        .fill(ctx.style().visuals.window_fill)
        .inner_margin(4.0);
    TopBottomPanel::top("toolbar").frame(frame).show(ctx, |ui| {
        ui.allocate_ui_with_layout(
            ui.available_size(),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let back_button = ui.add_enabled(can_go_back, toolbar_button("<"));
                back_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Back".into());
                    info
                });
                if back_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Back,
                    );
                }

                let forward_button = ui.add_enabled(can_go_forward, toolbar_button(">"));
                forward_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Forward".into());
                    info
                });
                if forward_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Forward,
                    );
                }

                let reload_button = ui.add(toolbar_button("R"));
                reload_button.widget_info(|| {
                    let mut info = WidgetInfo::new(WidgetType::Button);
                    info.label = Some("Reload".into());
                    info
                });
                if reload_button.clicked() {
                    *location_dirty = false;
                    let _ = toolbar_routing::run_nav_action(
                        graph_app,
                        window,
                        focused_toolbar_node,
                        ToolbarNavAction::Reload,
                    );
                }
                ui.add_space(2.0);

                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let mut experimental_preferences_enabled =
                            state.experimental_preferences_enabled();
                        let prefs_toggle = ui
                            .toggle_value(&mut experimental_preferences_enabled, "Exp")
                            .on_hover_text("Enable experimental prefs");
                        prefs_toggle.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Enable experimental preferences".into());
                            info.selected = Some(experimental_preferences_enabled);
                            info
                        });
                        if prefs_toggle.clicked() {
                            state.set_experimental_preferences_enabled(
                                experimental_preferences_enabled,
                            );
                            *location_dirty = false;
                            window.queue_user_interface_command(UserInterfaceCommand::ReloadAll);
                        }

                        let (view_icon, view_tooltip) = if has_webview_tiles {
                            ("Graph", "Switch to Graph View")
                        } else {
                            ("Detail", "Switch to Detail View")
                        };
                        let view_toggle_button =
                            ui.add(toolbar_button(view_icon)).on_hover_text(view_tooltip);
                        view_toggle_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Toggle View".into());
                            info
                        });
                        if view_toggle_button.clicked() {
                            toggle_tile_view_requested = true;
                        }

                        let data_dir_button = ui
                            .add(toolbar_button("Dir"))
                            .on_hover_text("Switch graph data directory");
                        data_dir_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Switch graph data directory".into());
                            info
                        });
                        if data_dir_button.clicked() {
                            *show_data_dir_dialog = true;
                        }

                        let persistence_settings_button = ui
                            .add(toolbar_button("Cfg"))
                            .on_hover_text("Persistence settings");
                        persistence_settings_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Persistence settings".into());
                            info
                        });
                        if persistence_settings_button.clicked() {
                            *show_persistence_settings_dialog = true;
                        }

                        let clear_data_button = ui
                            .add(toolbar_button("Clr"))
                            .on_hover_text("Clear graph and saved data");
                        clear_data_button.widget_info(|| {
                            let mut info = WidgetInfo::new(WidgetType::Button);
                            info.label = Some("Clear graph and saved data".into());
                            info
                        });
                        if clear_data_button.clicked() {
                            *show_clear_data_confirm = true;
                        }

                        let physics_button = ui
                            .add(toolbar_button("Phys"))
                            .on_hover_text("Show/hide physics settings panel");
                        if physics_button.clicked() {
                            frame_intents.push(GraphIntent::TogglePhysicsPanel);
                        }

                        let new_node_button = ui
                            .add(toolbar_button("Node+"))
                            .on_hover_text("Create a new graph node");
                        if new_node_button.clicked() {
                            frame_intents.push(GraphIntent::CreateNodeNearCenter);
                        }

                        let new_tab_button = ui
                            .add(toolbar_button("Tab+"))
                            .on_hover_text(
                                "Create a new node and open it as a tab in this graph window",
                            );
                        if new_tab_button.clicked() {
                            frame_intents.push(GraphIntent::CreateNodeNearCenter);
                            open_selected_mode_after_submit = Some(ToolbarOpenMode::Tab);
                        }

                        let split_button = ui
                            .add(toolbar_button("Split+"))
                            .on_hover_text("Create a new node and open it in a split pane");
                        if split_button.clicked() {
                            frame_intents.push(GraphIntent::CreateNodeNearCenter);
                            open_selected_mode_after_submit = Some(ToolbarOpenMode::SplitHorizontal);
                        }

                        let location_id = egui::Id::new("location_input");
                        let location_field = ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::singleline(location)
                                .id(location_id)
                                .hint_text("Search or enter address"),
                        );

                        if location_field.changed() {
                            *location_dirty = true;
                        }
                        if focus_location_field_for_search || ui.input(|i| {
                            if cfg!(target_os = "macos") {
                                i.clone().consume_key(Modifiers::COMMAND, Key::L)
                            } else {
                                i.clone().consume_key(Modifiers::COMMAND, Key::L)
                                    || i.clone().consume_key(Modifiers::ALT, Key::D)
                            }
                        }) {
                            location_field.request_focus();
                        }
                        if location_field.gained_focus()
                            && let Some(mut state) = TextEditState::load(ui.ctx(), location_id)
                        {
                            state.cursor.set_char_range(Some(CCursorRange::two(
                                CCursor::new(0),
                                CCursor::new(location.len()),
                            )));
                            state.store(ui.ctx(), location_id);
                        }

                        let enter_while_focused =
                            location_field.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if enter_while_focused {
                            *location_submitted = true;
                        }
                        let should_submit_now = enter_while_focused
                            || *location_submitted
                            || (location_field.lost_focus()
                                && ui.input(|i| i.key_pressed(Key::Enter)));
                        if should_submit_now {
                            *location_submitted = false;
                            let split_open_requested = ui.input(|i| i.modifiers.shift);
                            let submit_result = toolbar_routing::submit_address_bar_intents(
                                graph_app,
                                location,
                                is_graph_view,
                                focused_toolbar_node,
                                split_open_requested,
                                window,
                                &state.servoshell_preferences.searchpage,
                            );
                            frame_intents.extend(submit_result.intents);
                            if submit_result.mark_clean {
                                *location_dirty = false;
                                open_selected_mode_after_submit = submit_result.open_mode;
                            }
                        }
                    },
                );
            },
        );
    });

    ToolbarUiOutput {
        toggle_tile_view_requested,
        open_selected_mode_after_submit,
        toolbar_visible: true,
    }
}
