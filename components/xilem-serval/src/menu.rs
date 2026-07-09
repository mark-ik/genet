/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A small reusable overlay menu: a positioned vertical list of labelled rows with one
//! marked active and each clicking a host handler. It is the shape every serval host
//! reaches for — a completion popup, a slash / command menu, a context menu, a picker —
//! composed from the existing [`overlay_at`](crate::overlay_at) primitive so it is pure
//! Tier-1 CSS/native views (no leaf). The host owns the state (which items, the query,
//! the selected index) and the keyboard; this only renders.
//!
//! Rows carry the classes `menu-row` and `menu-row-active`, and the container `menu`, so a
//! host stylesheet themes them.

use crate::pod::ServalElement;
use crate::{PointerClick, ServalCtx, View, el, on_click, overlay_at};

/// The container class on the menu overlay.
pub const MENU_CLASS: &str = "menu";
/// The class on an ordinary menu row.
pub const MENU_ROW_CLASS: &str = "menu-row";
/// The class on the active (selected) menu row.
pub const MENU_ROW_ACTIVE_CLASS: &str = "menu-row-active";

/// A positioned overlay menu at window point `(x, y)`: one row per `items` label, the row
/// at `selected` marked active, each row clicking `on_pick(state, index)`. Reusable across
/// hosts for completion / slash / command / context menus.
///
/// `on_pick` must be `Clone` (each row captures a copy). The host drives keyboard navigation
/// itself (moving `selected`, running `on_pick` on Enter) — this renders and handles clicks.
pub fn menu<State, F>(
    x: f32,
    y: f32,
    items: impl IntoIterator<Item = String>,
    selected: usize,
    on_pick: F,
) -> impl View<State, (), ServalCtx, Element = ServalElement>
where
    State: 'static,
    F: Fn(&mut State, usize) + Clone + 'static,
{
    let rows: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, label)| {
            let f = on_pick.clone();
            let class = if i == selected {
                MENU_ROW_ACTIVE_CLASS
            } else {
                MENU_ROW_CLASS
            };
            on_click(
                el::<_, State, ()>("div", label).attr("class", class),
                move |s: &mut State, _: PointerClick| f(s, i),
            )
        })
        .collect();
    overlay_at::<_, State, ()>(x, y, rows).attr("class", MENU_CLASS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_constructs_over_a_state() {
        // Building the view eagerly maps the items into rows (running the closure), so this
        // exercises the generics + construction. Row rendering / click routing is covered by
        // host integration tests, which run it through a real runner + layout.
        let _view = menu(
            10.0,
            20.0,
            ["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            1,
            |s: &mut usize, i| *s = i,
        );
    }
}
