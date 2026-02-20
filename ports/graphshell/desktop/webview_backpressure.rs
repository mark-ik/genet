/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use log::warn;
use servo::{OffscreenRenderingContext, RenderingContext, WebViewId, WindowRenderingContext};
use url::Url;

use crate::app::{GraphBrowserApp, GraphIntent};
use crate::graph::{NodeKey, NodeLifecycle};
use crate::running_app_state::RunningAppState;
use crate::window::ServoShellWindow;

// Pragmatic Phase A backpressure:
// Servo webview creation is not fallible in the embedder API, so we infer failure
// from "no semantic signal + no stable live webview" within a timeout window.
const WEBVIEW_CREATION_CONFIRMATION_WINDOW: Duration = Duration::from_secs(2);
const WEBVIEW_CREATION_TIMEOUT: Duration = Duration::from_secs(8);
const WEBVIEW_CREATION_MAX_RETRIES: u8 = 3;

#[derive(Clone, Copy, Debug)]
struct WebviewCreationProbe {
    webview_id: WebViewId,
    started_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WebviewCreationProbeOutcome {
    Confirmed,
    Pending,
    TimedOut,
}

fn cold_restore_url_for_node(node: &crate::graph::Node) -> String {
    if !node.history_entries.is_empty() {
        let idx = node
            .history_index
            .min(node.history_entries.len().saturating_sub(1));
        if let Some(url) = node.history_entries.get(idx)
            && !url.is_empty()
        {
            return url.clone();
        }
    }
    node.url.clone()
}

#[derive(Default, Debug)]
pub(crate) struct WebviewCreationBackpressureState {
    retry_count: u8,
    pending: Option<WebviewCreationProbe>,
}

fn classify_webview_creation_probe(
    elapsed: Duration,
    contains_webview: bool,
    has_responsive_signal: bool,
) -> WebviewCreationProbeOutcome {
    if has_responsive_signal
        || (contains_webview && elapsed >= WEBVIEW_CREATION_CONFIRMATION_WINDOW)
    {
        WebviewCreationProbeOutcome::Confirmed
    } else if elapsed >= WEBVIEW_CREATION_TIMEOUT {
        WebviewCreationProbeOutcome::TimedOut
    } else {
        WebviewCreationProbeOutcome::Pending
    }
}

pub(crate) fn ensure_webview_for_node(
    graph_app: &mut GraphBrowserApp,
    window: &ServoShellWindow,
    app_state: &Option<Rc<RunningAppState>>,
    base_rendering_context: &Rc<OffscreenRenderingContext>,
    window_rendering_context: &Rc<WindowRenderingContext>,
    tile_rendering_contexts: &mut HashMap<NodeKey, Rc<OffscreenRenderingContext>>,
    node_key: NodeKey,
    responsive_webviews: &HashSet<WebViewId>,
    webview_creation_backpressure: &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    lifecycle_intents: &mut Vec<GraphIntent>,
) {
    let (Some(node), Some(running_state)) =
        (graph_app.graph.get_node(node_key), app_state.as_ref())
    else {
        webview_creation_backpressure.remove(&node_key);
        return;
    };
    if node.lifecycle != NodeLifecycle::Active {
        webview_creation_backpressure.remove(&node_key);
        return;
    }
    let node_url = cold_restore_url_for_node(node);

    if let Some(existing_webview_id) = graph_app.get_webview_for_node(node_key) {
        if window.contains_webview(existing_webview_id) {
            if responsive_webviews.contains(&existing_webview_id)
                && let Some(state) = webview_creation_backpressure.get_mut(&node_key)
            {
                state.pending = None;
                state.retry_count = 0;
            }
            return;
        }
        lifecycle_intents.push(GraphIntent::UnmapWebview {
            webview_id: existing_webview_id,
        });
    }

    let state = webview_creation_backpressure.entry(node_key).or_default();
    if state.pending.is_some() {
        return;
    }
    if state.retry_count >= WEBVIEW_CREATION_MAX_RETRIES {
        lifecycle_intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
        return;
    }

    let render_context = tile_rendering_contexts
        .entry(node_key)
        .or_insert_with(|| {
            Rc::new(window_rendering_context.offscreen_context(base_rendering_context.size()))
        })
        .clone();
    let url = Url::parse(&node_url).unwrap_or_else(|_| Url::parse("about:blank").unwrap());
    let webview =
        window.create_toplevel_webview_with_context(running_state.clone(), url, render_context);
    state.retry_count = state.retry_count.saturating_add(1);
    state.pending = Some(WebviewCreationProbe {
        webview_id: webview.id(),
        started_at: Instant::now(),
    });
    lifecycle_intents.extend([
        GraphIntent::MapWebviewToNode {
            webview_id: webview.id(),
            key: node_key,
        },
        GraphIntent::PromoteNodeToActive { key: node_key },
    ]);
}

pub(crate) fn reconcile_webview_creation_backpressure(
    graph_app: &GraphBrowserApp,
    window: &ServoShellWindow,
    responsive_webviews: &HashSet<WebViewId>,
    webview_creation_backpressure: &mut HashMap<NodeKey, WebviewCreationBackpressureState>,
    lifecycle_intents: &mut Vec<GraphIntent>,
) {
    let tracked_nodes: Vec<NodeKey> = webview_creation_backpressure.keys().copied().collect();
    for node_key in tracked_nodes {
        let Some(node) = graph_app.graph.get_node(node_key) else {
            webview_creation_backpressure.remove(&node_key);
            continue;
        };
        if node.lifecycle != NodeLifecycle::Active {
            webview_creation_backpressure.remove(&node_key);
            continue;
        }

        let mut remove_state = false;
        if let Some(state) = webview_creation_backpressure.get_mut(&node_key)
            && let Some(probe) = state.pending
        {
            let contains_webview = window.contains_webview(probe.webview_id);
            let has_responsive_signal = responsive_webviews.contains(&probe.webview_id);
            match classify_webview_creation_probe(
                probe.started_at.elapsed(),
                contains_webview,
                has_responsive_signal,
            ) {
                WebviewCreationProbeOutcome::Confirmed => {
                    state.pending = None;
                    state.retry_count = 0;
                },
                WebviewCreationProbeOutcome::Pending => {},
                WebviewCreationProbeOutcome::TimedOut => {
                    if contains_webview {
                        window.close_webview(probe.webview_id);
                    }
                    lifecycle_intents.push(GraphIntent::UnmapWebview {
                        webview_id: probe.webview_id,
                    });
                    state.pending = None;
                    if state.retry_count >= WEBVIEW_CREATION_MAX_RETRIES {
                        warn!(
                            "Demoting node {:?} after {} webview creation retries without confirmation",
                            node_key, state.retry_count
                        );
                        lifecycle_intents.push(GraphIntent::DemoteNodeToCold { key: node_key });
                        remove_state = true;
                    }
                },
            }
        }
        if remove_state {
            webview_creation_backpressure.remove(&node_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Node, NodeLifecycle};
    use euclid::default::{Point2D, Vector2D};
    use uuid::Uuid;

    #[test]
    fn test_classify_webview_creation_probe_confirms_on_responsive_signal() {
        let outcome = classify_webview_creation_probe(Duration::from_millis(10), false, true);
        assert_eq!(outcome, WebviewCreationProbeOutcome::Confirmed);
    }

    #[test]
    fn test_classify_webview_creation_probe_confirms_on_stable_live_webview() {
        let outcome = classify_webview_creation_probe(
            WEBVIEW_CREATION_CONFIRMATION_WINDOW + Duration::from_millis(1),
            true,
            false,
        );
        assert_eq!(outcome, WebviewCreationProbeOutcome::Confirmed);
    }

    #[test]
    fn test_classify_webview_creation_probe_times_out_without_confirmation() {
        let outcome = classify_webview_creation_probe(
            WEBVIEW_CREATION_TIMEOUT + Duration::from_millis(1),
            false,
            false,
        );
        assert_eq!(outcome, WebviewCreationProbeOutcome::TimedOut);
    }

    #[test]
    fn test_classify_webview_creation_probe_pending_before_timeout() {
        let outcome = classify_webview_creation_probe(Duration::from_millis(500), false, false);
        assert_eq!(outcome, WebviewCreationProbeOutcome::Pending);
    }

    fn test_node(url: &str) -> Node {
        Node {
            id: Uuid::new_v4(),
            url: url.to_string(),
            title: url.to_string(),
            position: Point2D::new(0.0, 0.0),
            velocity: Vector2D::new(0.0, 0.0),
            is_pinned: false,
            last_visited: std::time::SystemTime::now(),
            history_entries: Vec::new(),
            history_index: 0,
            thumbnail_png: None,
            thumbnail_width: 0,
            thumbnail_height: 0,
            favicon_rgba: None,
            favicon_width: 0,
            favicon_height: 0,
            session_scroll: None,
            session_form_draft: None,
            lifecycle: NodeLifecycle::Cold,
        }
    }

    #[test]
    fn test_cold_restore_url_for_node_prefers_history_index_entry() {
        let mut node = test_node("https://fallback.example");
        node.history_entries = vec![
            "https://example.com/one".to_string(),
            "https://example.com/two".to_string(),
        ];
        node.history_index = 1;
        assert_eq!(
            cold_restore_url_for_node(&node),
            "https://example.com/two".to_string()
        );
    }

    #[test]
    fn test_cold_restore_url_for_node_falls_back_to_node_url_without_history() {
        let node = test_node("https://fallback.example");
        assert_eq!(
            cold_restore_url_for_node(&node),
            "https://fallback.example".to_string()
        );
    }
}
