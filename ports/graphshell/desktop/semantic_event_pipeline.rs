/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::HashSet;

use servo::WebViewId;

use crate::app::GraphIntent;
use crate::window::GraphSemanticEvent;

pub(crate) fn graph_intents_from_semantic_events(
    events: Vec<GraphSemanticEvent>,
) -> Vec<GraphIntent> {
    let mut intents = Vec::with_capacity(events.len());
    for event in events {
        match event {
            GraphSemanticEvent::UrlChanged {
                webview_id,
                new_url,
            } => intents.push(GraphIntent::WebViewUrlChanged {
                webview_id,
                new_url,
            }),
            GraphSemanticEvent::HistoryChanged {
                webview_id,
                entries,
                current,
            } => intents.push(GraphIntent::WebViewHistoryChanged {
                webview_id,
                entries,
                current,
            }),
            GraphSemanticEvent::PageTitleChanged { webview_id, title } => {
                intents.push(GraphIntent::WebViewTitleChanged { webview_id, title });
            },
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id,
                child_webview_id,
                initial_url,
            } => intents.push(GraphIntent::WebViewCreated {
                parent_webview_id,
                child_webview_id,
                initial_url,
            }),
            GraphSemanticEvent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace,
            } => intents.push(GraphIntent::WebViewCrashed {
                webview_id,
                reason,
                has_backtrace,
            }),
        }
    }
    intents
}

pub(crate) fn graph_intents_and_responsive_from_events(
    events: Vec<GraphSemanticEvent>,
) -> (Vec<GraphIntent>, Vec<WebViewId>, HashSet<WebViewId>) {
    let mut create_events = Vec::new();
    let mut other_events = Vec::new();
    let mut created_child_webviews = Vec::new();
    let mut responsive_webviews = HashSet::new();

    for event in events {
        match &event {
            GraphSemanticEvent::CreateNewWebView {
                parent_webview_id,
                child_webview_id,
                ..
            } => {
                responsive_webviews.insert(*parent_webview_id);
                responsive_webviews.insert(*child_webview_id);
                created_child_webviews.push(*child_webview_id);
                create_events.push(event);
            },
            GraphSemanticEvent::UrlChanged { webview_id, .. }
            | GraphSemanticEvent::HistoryChanged { webview_id, .. }
            | GraphSemanticEvent::PageTitleChanged { webview_id, .. } => {
                responsive_webviews.insert(*webview_id);
                other_events.push(event);
            },
            GraphSemanticEvent::WebViewCrashed { .. } => {
                other_events.push(event);
            },
        }
    }

    let mut intents = graph_intents_from_semantic_events(create_events);
    intents.extend(graph_intents_from_semantic_events(other_events));
    (intents, created_child_webviews, responsive_webviews)
}
