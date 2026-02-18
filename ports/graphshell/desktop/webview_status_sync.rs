/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use servo::{LoadStatus, WebViewId};

use crate::window::ServoShellWindow;

pub(crate) fn update_location_in_toolbar(
    location_dirty: bool,
    location: &mut String,
    has_webview_tiles: bool,
    selected_node_url: Option<String>,
    focused_webview_id: Option<WebViewId>,
    window: &ServoShellWindow,
) -> bool {
    if location_dirty {
        return false;
    }

    if !has_webview_tiles {
        if let Some(url) = selected_node_url
            && url != *location
        {
            *location = url;
            return true;
        }
        return false;
    }

    let current_url_string = focused_webview_id
        .and_then(|id| window.webview_by_id(id))
        .and_then(|webview| Some(webview.url()?.to_string()));
    match current_url_string {
        Some(new_location) if new_location != *location => {
            *location = new_location;
            true
        },
        _ => false,
    }
}

pub(crate) fn update_load_status(
    load_status: &mut LoadStatus,
    location_dirty: &mut bool,
    focused_webview_id: Option<WebViewId>,
    window: &ServoShellWindow,
) -> bool {
    let state_status = focused_webview_id
        .and_then(|id| window.webview_by_id(id))
        .map(|webview| webview.load_status())
        .unwrap_or(LoadStatus::Complete);
    let old_status = std::mem::replace(load_status, state_status);
    let status_changed = old_status != *load_status;

    if status_changed {
        *location_dirty = false;
    }

    status_changed
}

pub(crate) fn update_status_text(
    status_text: &mut Option<String>,
    focused_webview_id: Option<WebViewId>,
    window: &ServoShellWindow,
) -> bool {
    let state_status = focused_webview_id
        .and_then(|id| window.webview_by_id(id))
        .and_then(|webview| webview.status_text());
    let old_status = std::mem::replace(status_text, state_status);
    old_status != *status_text
}

pub(crate) fn update_can_go_back_and_forward(
    can_go_back: &mut bool,
    can_go_forward: &mut bool,
    focused_webview_id: Option<WebViewId>,
    window: &ServoShellWindow,
) -> bool {
    let (state_can_go_back, state_can_go_forward) = focused_webview_id
        .and_then(|id| window.webview_by_id(id))
        .map(|webview| (webview.can_go_back(), webview.can_go_forward()))
        .unwrap_or((false, false));

    let can_go_back_changed = *can_go_back != state_can_go_back;
    let can_go_forward_changed = *can_go_forward != state_can_go_forward;
    *can_go_back = state_can_go_back;
    *can_go_forward = state_can_go_forward;
    can_go_back_changed || can_go_forward_changed
}
