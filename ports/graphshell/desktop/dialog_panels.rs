/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashMap;
use std::rc::Rc;

use egui_tiles::Tree;
use servo::{OffscreenRenderingContext, WebViewId};

use crate::app::{GraphBrowserApp, GraphIntent};
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
    pub(crate) location_dirty: &'a mut bool,
    pub(crate) location_submitted: &'a mut bool,
    pub(crate) show_clear_data_confirm: &'a mut bool,
    pub(crate) toasts: &'a mut egui_notify::Toasts,
}

pub(crate) fn render_dialog_panels(args: DialogPanelsArgs<'_>) {
    if *args.show_clear_data_confirm {
        let confirm_deadline_id = egui::Id::new("clear_data_confirm_deadline_secs");
        let now = args.ctx.input(|i| i.time);
        let armed_deadline = args
            .ctx
            .data_mut(|d| d.get_temp::<f64>(confirm_deadline_id));
        if armed_deadline.is_some_and(|deadline| deadline >= now) {
            args.frame_intents
                .extend(webview_controller::close_all_webviews(
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
            *args.location_submitted = false;
            args.ctx.data_mut(|d| d.remove::<f64>(confirm_deadline_id));
            args.toasts.success("Cleared graph and saved data");
        } else {
            args.ctx
                .data_mut(|d| d.insert_temp(confirm_deadline_id, now + 3.0));
            args.toasts
                .warning("Press Clr again within 3 seconds to clear graph and saved data");
        }
        *args.show_clear_data_confirm = false;
    }
}
