/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use egui::Key;

use crate::app::{GraphBrowserApp, GraphIntent, SearchDisplayMode};
use crate::graph::NodeKey;

pub(crate) struct GraphSearchFlowArgs<'a> {
    pub ctx: &'a egui::Context,
    pub graph_app: &'a mut GraphBrowserApp,
    pub graph_search_open: &'a mut bool,
    pub graph_search_query: &'a mut String,
    pub graph_search_filter_mode: &'a mut bool,
    pub graph_search_matches: &'a mut Vec<NodeKey>,
    pub graph_search_active_match_index: &'a mut Option<usize>,
    pub location: &'a mut String,
    pub location_dirty: &'a mut bool,
    pub frame_intents: &'a mut Vec<GraphIntent>,
    pub graph_search_available: bool,
}

pub(crate) struct GraphSearchFlowOutput {
    pub focus_graph_search_field: bool,
    pub focus_location_field_for_search: bool,
    pub suppress_toggle_view: bool,
}

pub(crate) fn handle_graph_search_flow<FRefresh, FStep, FActive>(
    args: GraphSearchFlowArgs<'_>,
    mut refresh_graph_search_matches: FRefresh,
    mut step_graph_search_active_match: FStep,
    active_graph_search_match: FActive,
) -> GraphSearchFlowOutput
where
    FRefresh: FnMut(&mut GraphBrowserApp, &str, &mut Vec<NodeKey>, &mut Option<usize>),
    FStep: FnMut(&[NodeKey], &mut Option<usize>, isize),
    FActive: Fn(&[NodeKey], Option<usize>) -> Option<NodeKey>,
{
    let GraphSearchFlowArgs {
        ctx,
        graph_app,
        graph_search_open,
        graph_search_query,
        graph_search_filter_mode,
        graph_search_matches,
        graph_search_active_match_index,
        location,
        location_dirty,
        frame_intents,
        graph_search_available,
    } = args;

    if !graph_search_available && *graph_search_open {
        *graph_search_open = false;
        graph_search_query.clear();
        graph_search_matches.clear();
        *graph_search_active_match_index = None;
        *graph_search_filter_mode = false;
        graph_app.search_display_mode = SearchDisplayMode::Highlight;
        graph_app.egui_state_dirty = true;
    }

    let search_shortcut_pressed = ctx.input(|i| {
        if cfg!(target_os = "macos") {
            i.modifiers.command && i.key_pressed(Key::F)
        } else {
            i.modifiers.ctrl && i.key_pressed(Key::F)
        }
    });

    let focus_graph_search_field = false;
    let mut focus_location_field_for_search = false;
    if graph_search_available && search_shortcut_pressed {
        // Omnibox-first graph search: Ctrl+F focuses the location bar
        // with an `@` query prefix instead of opening a separate dialog.
        *graph_search_open = false;
        if !location.starts_with('@') {
            *location = "@".to_string();
        }
        *location_dirty = true;
        focus_location_field_for_search = true;
    }

    let mut suppress_toggle_view = false;
    if *graph_search_open {
        refresh_graph_search_matches(
            graph_app,
            graph_search_query,
            graph_search_matches,
            graph_search_active_match_index,
        );

        if ctx.input(|i| i.key_pressed(Key::ArrowDown)) {
            step_graph_search_active_match(
                graph_search_matches,
                graph_search_active_match_index,
                1,
            );
        }
        if ctx.input(|i| i.key_pressed(Key::ArrowUp)) {
            step_graph_search_active_match(
                graph_search_matches,
                graph_search_active_match_index,
                -1,
            );
        }
        if ctx.input(|i| i.key_pressed(Key::Enter))
            && let Some(node_key) =
                active_graph_search_match(graph_search_matches, *graph_search_active_match_index)
        {
            frame_intents.push(GraphIntent::SelectNode {
                key: node_key,
                multi_select: false,
            });
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            suppress_toggle_view = true;
            if graph_search_query.trim().is_empty() {
                *graph_search_open = false;
                *graph_search_filter_mode = false;
                graph_app.search_display_mode = SearchDisplayMode::Highlight;
            } else {
                graph_search_query.clear();
            }
            refresh_graph_search_matches(
                graph_app,
                graph_search_query,
                graph_search_matches,
                graph_search_active_match_index,
            );
            graph_app.egui_state_dirty = true;
        }
    }

    GraphSearchFlowOutput {
        focus_graph_search_field,
        focus_location_field_for_search,
        suppress_toggle_view,
    }
}
