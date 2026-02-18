/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::rc::Rc;

use egui_tiles::Tree;
use servo::{OffscreenRenderingContext, WebViewId};

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::persistence_ops;
use crate::desktop::tile_kind::TileKind;
use crate::desktop::tile_runtime;
use crate::desktop::webview_controller;
use crate::graph::NodeKey;
use crate::window::ServoShellWindow;

pub(crate) struct DialogPanelsArgs<'a> {
    pub(crate) ctx: &'a egui::Context,
    pub(crate) graph_app: &'a mut GraphBrowserApp,
    pub(crate) window: &'a ServoShellWindow,
    pub(crate) tiles_tree: &'a mut Tree<TileKind>,
    pub(crate) tile_rendering_contexts: &'a mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    pub(crate) tile_favicon_textures: &'a mut HashMap<NodeKey, (u64, egui::TextureHandle)>,
    pub(crate) favicon_textures:
        &'a mut HashMap<WebViewId, (egui::TextureHandle, egui::load::SizedTexture)>,
    pub(crate) frame_intents: &'a mut Vec<GraphIntent>,
    pub(crate) location: &'a mut String,
    pub(crate) location_dirty: &'a mut bool,
    pub(crate) location_submitted: &'a mut bool,
    pub(crate) show_clear_data_confirm: &'a mut bool,
    pub(crate) show_data_dir_dialog: &'a mut bool,
    pub(crate) data_dir_input: &'a mut String,
    pub(crate) data_dir_status: &'a mut Option<String>,
    pub(crate) show_persistence_settings_dialog: &'a mut bool,
    pub(crate) snapshot_interval_input: &'a mut String,
    pub(crate) persistence_settings_status: &'a mut Option<String>,
}

pub(crate) fn render_dialog_panels(args: DialogPanelsArgs<'_>) {
    if *args.show_clear_data_confirm {
        egui::Window::new("Clear Saved Graph Data?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(args.ctx, |ui| {
                ui.label("This clears all graph nodes and saved graph data.");
                ui.label("This action cannot be undone.");
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        *args.show_clear_data_confirm = false;
                    }
                    if ui.button("Clear Data").clicked() {
                        args.frame_intents.extend(webview_controller::close_all_webviews(
                            args.graph_app,
                            args.window,
                        ));
                        tile_runtime::reset_runtime_webview_state(
                            args.tiles_tree,
                            args.tile_rendering_contexts,
                            args.tile_favicon_textures,
                            args.favicon_textures,
                        );
                        args.graph_app.clear_graph_and_persistence();
                        *args.location_dirty = false;
                        *args.show_clear_data_confirm = false;
                    }
                });
            });
    }

    // The toolbar height is where the Context's available rect starts.
    // For reasons that are unclear, the TopBottomPanel's ui cursor exceeds this by one egui
    // point, but the Context is correct and the TopBottomPanel is wrong.
    if *args.show_data_dir_dialog {
        egui::Window::new("Switch Graph Data Directory")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(args.ctx, |ui| {
                ui.label("Enter a directory path to load/save graph data.");
                ui.add(
                    egui::TextEdit::singleline(args.data_dir_input)
                        .desired_width(480.0)
                        .hint_text("C:\\path\\to\\graph_data"),
                );
                if let Some(message) = args.data_dir_status.as_deref() {
                    ui.label(message);
                }
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        *args.show_data_dir_dialog = false;
                        *args.data_dir_status = None;
                    }
                    if ui.button("Switch").clicked() {
                        let Some(target_dir) = persistence_ops::parse_data_dir_input(args.data_dir_input)
                        else {
                            *args.data_dir_status =
                                Some("Enter a non-empty directory path.".to_string());
                            return;
                        };
                        match persistence_ops::switch_persistence_store(
                            args.graph_app,
                            args.window,
                            args.tiles_tree,
                            args.tile_rendering_contexts,
                            args.tile_favicon_textures,
                            args.favicon_textures,
                            args.frame_intents,
                            target_dir.clone(),
                        ) {
                            Ok(()) => {
                                *args.location = args
                                    .graph_app
                                    .graph
                                    .nodes()
                                    .next()
                                    .map(|(_, node)| node.url.clone())
                                    .unwrap_or_default();
                                *args.snapshot_interval_input = args
                                    .graph_app
                                    .snapshot_interval_secs()
                                    .unwrap_or(crate::persistence::DEFAULT_SNAPSHOT_INTERVAL_SECS)
                                    .to_string();
                                *args.location_dirty = false;
                                *args.location_submitted = false;
                                *args.show_data_dir_dialog = false;
                                *args.data_dir_input = target_dir.display().to_string();
                                *args.data_dir_status = None;
                            },
                            Err(e) => {
                                *args.data_dir_status =
                                    Some(format!("Failed to switch data directory: {e}"));
                            },
                        }
                    }
                });
            });
    }

    if *args.show_persistence_settings_dialog {
        egui::Window::new("Persistence Settings")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(args.ctx, |ui| {
                ui.label("Snapshot interval (seconds):");
                ui.add(
                    egui::TextEdit::singleline(args.snapshot_interval_input)
                        .desired_width(180.0)
                        .hint_text("300"),
                );
                if let Some(message) = args.persistence_settings_status.as_deref() {
                    ui.label(message);
                }
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        *args.show_persistence_settings_dialog = false;
                        *args.persistence_settings_status = None;
                    }
                    if ui.button("Apply").clicked() {
                        let raw = args.snapshot_interval_input.trim();
                        let parsed_secs = raw.parse::<u64>();
                        match parsed_secs {
                            Ok(secs) => match args.graph_app.set_snapshot_interval_secs(secs) {
                                Ok(()) => {
                                    *args.snapshot_interval_input = secs.to_string();
                                    *args.persistence_settings_status =
                                        Some("Snapshot interval updated.".to_string());
                                },
                                Err(e) => {
                                    *args.persistence_settings_status =
                                        Some(format!("Failed to update interval: {e}"));
                                },
                            },
                            Err(_) => {
                                *args.persistence_settings_status =
                                    Some("Enter a valid positive integer.".to_string());
                            },
                        }
                    }
                });
            });
    }
}
