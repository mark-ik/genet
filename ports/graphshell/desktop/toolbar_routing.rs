/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::desktop::nav_targeting;
use crate::desktop::webview_controller;
use crate::graph::NodeKey;
use crate::window::ServoShellWindow;

pub(crate) enum ToolbarNavAction {
    Back,
    Forward,
    Reload,
}

pub(crate) enum ToolbarOpenMode {
    Tab,
    SplitHorizontal,
}

pub(crate) struct ToolbarSubmitResult {
    pub(crate) intents: Vec<GraphIntent>,
    pub(crate) mark_clean: bool,
    pub(crate) open_mode: Option<ToolbarOpenMode>,
}

pub(crate) fn run_nav_action(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    focused_toolbar_node: Option<NodeKey>,
    action: ToolbarNavAction,
) -> bool {
    let Some(webview_id) = nav_targeting::nav_target_webview_id(graph_app, focused_toolbar_node)
    else {
        return false;
    };
    let Some(webview) = window.webview_by_id(webview_id) else {
        return false;
    };
    match action {
        ToolbarNavAction::Back => {
            let _ = webview.go_back(1);
        },
        ToolbarNavAction::Forward => {
            let _ = webview.go_forward(1);
        },
        ToolbarNavAction::Reload => webview.reload(),
    }
    window.set_needs_update();
    true
}

pub(crate) fn submit_address_bar_intents(
    graph_app: &GraphBrowserApp,
    location: &str,
    is_graph_view: bool,
    focused_toolbar_node: Option<NodeKey>,
    split_open_requested: bool,
    window: &ServoShellWindow,
    searchpage: &str,
) -> ToolbarSubmitResult {
    let focused_webview_id =
        focused_toolbar_node.and_then(|key| graph_app.get_webview_for_node(key));
    let submit_result = webview_controller::handle_address_bar_submit_intents(
        graph_app,
        location,
        is_graph_view,
        focused_toolbar_node,
        focused_webview_id,
        window,
        searchpage,
    );
    ToolbarSubmitResult {
        intents: submit_result.intents,
        mark_clean: submit_result.outcome.mark_clean,
        open_mode: requested_open_mode(
            submit_result.outcome.open_selected_tile,
            split_open_requested,
        ),
    }
}

fn requested_open_mode(
    open_selected_tile: bool,
    split_open_requested: bool,
) -> Option<ToolbarOpenMode> {
    if !open_selected_tile {
        return None;
    }
    Some(if split_open_requested {
        ToolbarOpenMode::SplitHorizontal
    } else {
        ToolbarOpenMode::Tab
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_requested_open_mode_none_when_not_requested() {
        assert!(requested_open_mode(false, false).is_none());
        assert!(requested_open_mode(false, true).is_none());
    }

    #[test]
    fn test_requested_open_mode_tab_when_split_not_requested() {
        assert!(matches!(
            requested_open_mode(true, false),
            Some(ToolbarOpenMode::Tab)
        ));
    }

    #[test]
    fn test_requested_open_mode_split_when_requested() {
        assert!(matches!(
            requested_open_mode(true, true),
            Some(ToolbarOpenMode::SplitHorizontal)
        ));
    }
}
