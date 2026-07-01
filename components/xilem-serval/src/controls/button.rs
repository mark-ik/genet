/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`button`]: a `<button>` view with a click handler.

use crate::{El, OnClick, OptionalAction, PointerClick, el, on_click};

/// A `<button>` view: `label` text plus an `on_click` handler — the ergonomic
/// form of `on_click(el("button", label), handler)`. The handler may return an
/// action (it is an [`OptionalAction`]) exactly as [`on_click`](crate::on_click).
///
/// Add a `class` (or any attribute) with the fluent [`OnClick::attr`], e.g.
/// `button("Save", on_save).attr("class", "primary")`.
pub fn button<State, Action, OA, F>(
    label: impl Into<String>,
    handler: F,
) -> OnClick<El<String, State, Action>, State, Action, F>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
    on_click(el::<_, State, Action>("button", label.into()), handler)
}
