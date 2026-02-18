/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use servo::WebViewId;

use crate::app::GraphBrowserApp;
use crate::graph::NodeKey;

/// Resolve which node the toolbar/omnibar should target.
///
/// Preference order:
/// 1) focused tile webview mapping (if available),
/// 2) active tile node fallback.
pub(crate) fn focused_toolbar_node(
    graph_app: &GraphBrowserApp,
    active_webview_node: Option<NodeKey>,
    focused_webview_id: Option<WebViewId>,
    selected_node: Option<NodeKey>,
) -> Option<NodeKey> {
    focused_webview_id
        .and_then(|webview_id| graph_app.get_node_for_webview(webview_id))
        .or(active_webview_node)
        .or(selected_node)
}

/// Resolve the explicit target webview for navigation commands.
pub(crate) fn nav_target_webview_id(
    graph_app: &GraphBrowserApp,
    focused_toolbar_node: Option<NodeKey>,
) -> Option<WebViewId> {
    focused_toolbar_node.and_then(|node_key| graph_app.get_webview_for_node(node_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use euclid::default::Point2D;

    /// Create a unique WebViewId for testing.
    fn test_webview_id() -> servo::WebViewId {
        thread_local! {
            static NS_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        NS_INSTALLED.with(|cell| {
            if !cell.get() {
                base::id::PipelineNamespace::install(base::id::PipelineNamespaceId(45));
                cell.set(true);
            }
        });
        servo::WebViewId::new(base::id::PainterId::next())
    }

    #[test]
    fn test_focused_toolbar_node_prefers_focused_webview_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let b = app.add_node_and_sync("https://b.example".into(), Point2D::new(10.0, 0.0));
        let a_id = test_webview_id();
        let b_id = test_webview_id();
        app.map_webview_to_node(a_id, a);
        app.map_webview_to_node(b_id, b);

        let chosen = focused_toolbar_node(&app, Some(a), Some(b_id), Some(a));
        assert_eq!(chosen, Some(b));
    }

    #[test]
    fn test_focused_toolbar_node_falls_back_to_active_node() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let a_id = test_webview_id();
        app.map_webview_to_node(a_id, a);

        let chosen = focused_toolbar_node(&app, Some(a), None, None);
        assert_eq!(chosen, Some(a));
    }

    #[test]
    fn test_focused_toolbar_node_falls_back_to_selected_node_when_no_live_focus() {
        let mut app = GraphBrowserApp::new_for_testing();
        let selected =
            app.add_node_and_sync("https://selected.example".into(), Point2D::new(0.0, 0.0));
        let other = app.add_node_and_sync("https://other.example".into(), Point2D::new(10.0, 0.0));
        let other_wv = test_webview_id();
        app.map_webview_to_node(other_wv, other);

        let chosen = focused_toolbar_node(&app, None, None, Some(selected));
        assert_eq!(chosen, Some(selected));
    }

    #[test]
    fn test_nav_target_webview_id_resolves_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let a = app.add_node_and_sync("https://a.example".into(), Point2D::new(0.0, 0.0));
        let a_id = test_webview_id();
        app.map_webview_to_node(a_id, a);

        assert_eq!(nav_target_webview_id(&app, Some(a)), Some(a_id));
    }

    #[test]
    fn test_nav_target_webview_id_none_without_mapping() {
        let mut app = GraphBrowserApp::new_for_testing();
        let node = app.add_node_and_sync("https://unmapped.example".into(), Point2D::new(0.0, 0.0));
        assert_eq!(nav_target_webview_id(&app, Some(node)), None);
        assert_eq!(nav_target_webview_id(&app, None), None);
    }
}
