/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Webview lifecycle management for the graph browser.
//!
//! Extracts webview create/destroy/sync logic from gui.rs into focused,
//! testable functions. All Servo WebView operations (create, close,
//! sync to graph nodes) live here.

use std::collections::HashSet;

use servo::WebViewId;

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::NodeKey;
use crate::parser::location_bar_input_to_url;
use crate::search::fuzzy_match_node_keys;
use crate::window::ServoShellWindow;
use euclid::default::Point2D;

fn reconcile_mappings_and_selection(
    app: &mut GraphBrowserApp,
    seen_webviews: &HashSet<WebViewId>,
    active_webview: Option<WebViewId>,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    // Highlight the active tab's node (reuse reducer intent for consistency).
    if let Some(active_wv_id) = active_webview
        && let Some(active_node_key) = app.get_node_for_webview(active_wv_id)
    {
        intents.push(GraphIntent::SelectNode {
            key: active_node_key,
            multi_select: false,
        });
    }

    // Clean up mappings for webviews that no longer exist.
    let old_webviews: Vec<WebViewId> = app
        .webview_node_mappings()
        .filter(|(wv_id, _)| !seen_webviews.contains(wv_id))
        .map(|(wv_id, _)| wv_id)
        .collect();

    for wv_id in old_webviews {
        intents.push(GraphIntent::UnmapWebview { webview_id: wv_id });
    }
    intents
}

fn intents_for_graph_view_address_submit(
    app: &GraphBrowserApp,
    input: &str,
) -> (bool, Vec<GraphIntent>) {
    let input = input.trim();
    if input.is_empty() {
        return (false, Vec::new());
    }

    if let Some(selected_node) = app.get_single_selected_node() {
        (
            true,
            vec![GraphIntent::SetNodeUrl {
                key: selected_node,
                new_url: input.to_string(),
            }],
        )
    } else {
        let position = new_node_position_for_context(app, app.selected_nodes.primary());
        (
            true,
            vec![GraphIntent::CreateNodeAtUrl {
                url: input.to_string(),
                position,
            }],
        )
    }
}

fn graph_centroid_or_default(app: &GraphBrowserApp) -> Point2D<f32> {
    if app.graph.node_count() == 0 {
        return Point2D::new(400.0, 300.0);
    }
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut count = 0.0f32;
    for (_, node) in app.graph.nodes() {
        sum_x += node.position.x;
        sum_y += node.position.y;
        count += 1.0;
    }
    Point2D::new(sum_x / count, sum_y / count)
}

fn new_node_position_for_context(app: &GraphBrowserApp, anchor: Option<NodeKey>) -> Point2D<f32> {
    let base = anchor
        .and_then(|key| app.graph.get_node(key).map(|node| node.position))
        .unwrap_or_else(|| graph_centroid_or_default(app));
    let n = app.graph.node_count() as f32;
    let angle = n * 0.7853982; // ~pi/4 steps for simple deterministic spread.
    let radius = 90.0;
    Point2D::new(base.x + radius * angle.cos(), base.y + radius * angle.sin())
}

fn intents_for_omnibox_node_search(app: &GraphBrowserApp, query: &str) -> (bool, Vec<GraphIntent>) {
    let query = query.trim();
    if query.is_empty() {
        return (false, Vec::new());
    }
    if let Some(key) = fuzzy_match_node_keys(&app.graph, query).first().copied() {
        return (
            true,
            vec![GraphIntent::SelectNode {
                key,
                multi_select: false,
            }],
        );
    }
    (false, Vec::new())
}

/// Sync existing webviews to graph mappings.
///
/// This is now structural-reconciliation only (cleanup + active highlight).
/// Structural graph creation and navigation semantics are handled by Servo
/// delegate events routed through GraphIntent reducer paths.
pub(crate) fn sync_to_graph_intents(
    app: &mut GraphBrowserApp,
    window: &ServoShellWindow,
) -> Vec<GraphIntent> {
    // Track which webviews we've seen (to remove stale mappings later).
    let mut seen_webviews = HashSet::new();
    for (wv_id, _) in window.webviews().into_iter() {
        seen_webviews.insert(wv_id);
    }
    let active = window.platform_window().preferred_input_webview_id(window);
    reconcile_mappings_and_selection(app, &seen_webviews, active)
}

pub(crate) struct AddressBarSubmitOutcome {
    pub mark_clean: bool,
    pub open_selected_tile: bool,
}

pub(crate) struct AddressBarIntentOutcome {
    pub outcome: AddressBarSubmitOutcome,
    pub intents: Vec<GraphIntent>,
}

pub(crate) fn handle_address_bar_submit_intents(
    app: &GraphBrowserApp,
    url: &str,
    is_graph_view: bool,
    focused_node: Option<NodeKey>,
    focused_webview: Option<WebViewId>,
    window: &ServoShellWindow,
    searchpage: &str,
) -> AddressBarIntentOutcome {
    let input = url.trim();
    if let Some(query) = input.strip_prefix('@') {
        let (_handled, intents) = intents_for_omnibox_node_search(app, query);
        return AddressBarIntentOutcome {
            outcome: AddressBarSubmitOutcome {
                mark_clean: true,
                open_selected_tile: false,
            },
            intents,
        };
    }

    if is_graph_view {
        let (open_selected_tile, intents) = intents_for_graph_view_address_submit(app, input);
        AddressBarIntentOutcome {
            outcome: AddressBarSubmitOutcome {
                mark_clean: true,
                open_selected_tile,
            },
            intents,
        }
    } else {
        // Parse URL first before attempting to navigate.
        let Some(parsed_url) = location_bar_input_to_url(input, searchpage) else {
            log::warn!("Failed to parse location: {}", input);
            return AddressBarIntentOutcome {
                outcome: AddressBarSubmitOutcome {
                    mark_clean: false,
                    open_selected_tile: false,
                },
                intents: Vec::new(),
            };
        };

        if let Some(webview_id) = focused_webview
            && let Some(webview) = window.webview_by_id(webview_id)
        {
            window.activate_webview(webview_id);
            webview.load(parsed_url.into_url());
            window.set_needs_update();
            return AddressBarIntentOutcome {
                outcome: AddressBarSubmitOutcome {
                    mark_clean: false,
                    open_selected_tile: false,
                },
                intents: Vec::new(),
            };
        }

        // No focused live webview in detail mode:
        // if we still have a focused node/pane target, update/reactivate it;
        // otherwise create a new node as a fallback.
        if let Some(node_key) = focused_node {
            return AddressBarIntentOutcome {
                outcome: AddressBarSubmitOutcome {
                    mark_clean: true,
                    open_selected_tile: false,
                },
                intents: vec![
                    GraphIntent::SetNodeUrl {
                        key: node_key,
                        new_url: parsed_url.as_str().to_string(),
                    },
                    GraphIntent::PromoteNodeToActive { key: node_key },
                ],
            };
        }

        AddressBarIntentOutcome {
            outcome: AddressBarSubmitOutcome {
                mark_clean: true,
                open_selected_tile: true,
            },
            intents: vec![GraphIntent::CreateNodeAtUrl {
                url: parsed_url.into_url().to_string(),
                position: new_node_position_for_context(app, app.selected_nodes.primary()),
            }],
        }
    }
}

/// Close webviews associated with the given nodes.
///
/// Call before removing nodes from the graph to ensure the actual
/// Servo webviews are properly closed.
pub(crate) fn close_webviews_for_nodes(
    app: &GraphBrowserApp,
    nodes: &[NodeKey],
    window: &ServoShellWindow,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    for &node_key in nodes {
        if let Some(wv_id) = app.get_webview_for_node(node_key) {
            window.close_webview(wv_id);
            intents.push(GraphIntent::UnmapWebview { webview_id: wv_id });
        }
        intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
    }
    intents
}

/// Close all current webviews and clear their app mappings.
pub(crate) fn close_all_webviews(
    app: &GraphBrowserApp,
    window: &ServoShellWindow,
) -> Vec<GraphIntent> {
    let mut intents = Vec::new();
    let webviews_to_close: Vec<WebViewId> =
        window.webviews().into_iter().map(|(id, _)| id).collect();
    for wv_id in webviews_to_close {
        window.close_webview(wv_id);
        intents.push(GraphIntent::UnmapWebview { webview_id: wv_id });
        if let Some(node_key) = app.get_node_for_webview(wv_id) {
            intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
        }
    }
    intents
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
                base::id::PipelineNamespace::install(base::id::PipelineNamespaceId(43));
                cell.set(true);
            }
        });
        servo::WebViewId::new(base::id::PainterId::next())
    }

    #[test]
    fn test_new_node_position_for_context_defaults_to_center_when_empty() {
        let app = GraphBrowserApp::new_for_testing();
        let p = new_node_position_for_context(&app, None);
        assert!(p.x.is_finite() && p.y.is_finite());
    }

    #[test]
    fn test_new_node_position_for_context_biases_near_anchor() {
        let mut app = GraphBrowserApp::new_for_testing();
        let anchor = app
            .graph
            .add_node("https://anchor.com".into(), Point2D::new(100.0, 200.0));
        let p = new_node_position_for_context(&app, Some(anchor));
        let dx = p.x - 100.0;
        let dy = p.y - 200.0;
        assert!(dx.hypot(dy) > 20.0);
        assert!(dx.hypot(dy) < 140.0);
    }

    #[test]
    fn test_reconcile_mappings_removes_stale_webviews() {
        let mut app = GraphBrowserApp::new_for_testing();
        let n1 = app
            .graph
            .add_node("https://a.com".into(), Point2D::new(0.0, 0.0));
        let n2 = app
            .graph
            .add_node("https://b.com".into(), Point2D::new(1.0, 1.0));
        let w1 = test_webview_id();
        let w2 = test_webview_id();
        app.map_webview_to_node(w1, n1);
        app.map_webview_to_node(w2, n2);

        let mut seen = HashSet::new();
        seen.insert(w1);
        let intents = reconcile_mappings_and_selection(&mut app, &seen, Some(w1));
        app.apply_intents(intents);

        assert_eq!(app.get_node_for_webview(w1), Some(n1));
        assert_eq!(app.get_node_for_webview(w2), None);
        assert_eq!(app.get_single_selected_node(), Some(n1));
    }

    #[test]
    fn test_apply_graph_view_submit_updates_selected_node_url() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://old.com".into(), Point2D::new(0.0, 0.0));
        app.select_node(key, false);

        let (open_selected_tile, intents) =
            intents_for_graph_view_address_submit(&app, "https://new.com");
        app.apply_intents(intents);

        let node = app.graph.get_node(key).unwrap();
        assert_eq!(node.url, "https://new.com");
        assert!(open_selected_tile);
    }

    #[test]
    fn test_apply_graph_view_submit_creates_node_when_none_selected() {
        let mut app = GraphBrowserApp::new_for_testing();
        let before = app.graph.node_count();

        let (open_selected_tile, intents) =
            intents_for_graph_view_address_submit(&app, "https://created.com");
        app.apply_intents(intents);

        assert_eq!(app.graph.node_count(), before + 1);
        let selected = app.get_single_selected_node().unwrap();
        assert_eq!(
            app.graph.get_node(selected).unwrap().url,
            "https://created.com"
        );
        assert!(open_selected_tile);
    }

    #[test]
    fn test_apply_graph_view_submit_handle_search_selects_without_navigation() {
        let mut app = GraphBrowserApp::new_for_testing();
        let key = app
            .graph
            .add_node("https://example.com".into(), Point2D::new(0.0, 0.0));
        if let Some(node) = app.graph.get_node_mut(key) {
            node.title = "Example Handle".into();
        }
        let original_url = app.graph.get_node(key).unwrap().url.clone();

        let (open_selected_tile, intents) = intents_for_omnibox_node_search(&app, "example handle");
        app.apply_intents(intents);

        assert_eq!(app.get_single_selected_node(), Some(key));
        assert_eq!(app.graph.get_node(key).unwrap().url, original_url);
        assert!(open_selected_tile);
    }
}
