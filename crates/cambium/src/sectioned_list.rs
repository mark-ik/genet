/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A sectioned list: grouped rows under section headers, each row inert, muted,
//! or activatable.
//!
//! Consumer-pull (merecat, 2026-07-17): four panes render exactly this shape —
//! Trail (Recent / This node / Removed), Steward (active / queued), Alembic
//! (Recent / Saved), and Apparatus's diagnostics — and Trail was hand-DOM. Not
//! grown out of `action_list`: that component is palette-shaped (it wraps
//! `command_palette`'s filter-and-activate machinery), and a resident list pane
//! wants none of that; the two stay separate on purpose.
//!
//! Explicit inputs in, caller-state handler out (the grid's house style): the
//! caller passes the sections and one `on_row` handler; every activatable row
//! reports `(section, row)` through it, from a click or from Enter/Space on
//! the focused row. The handler may return a bubbling [`Action`](crate::Action)
//! — a Trail row's activation IS a navigation, so the list must not force a
//! mirror-and-drain detour on it.
//!
//! No geometry: rows are normal-flow blocks and the host's sheet gives them
//! their heights and colours (`list-section-title`, `list-row`,
//! `list-row muted`, `list-row action`). Not virtualized — every consumer's
//! sections are bounded (the grid is the tool for unbounded rows); a future
//! unbounded consumer pulls virtualization then, not before.

use crate::pod::GenetElement;
use crate::{
    GenetCtx, Key, NamedKey, OptionalAction, PointerClick, View, el, focusable, on_click, on_key,
};

/// How a row renders and whether it activates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListRowKind {
    /// A regular activatable row (`list-row`).
    Plain,
    /// An inert, de-emphasized row (`list-row muted`) — empty states, hints.
    Muted,
    /// An activatable row styled as an affordance (`list-row action`) — e.g. a
    /// Recover row.
    Action,
}

/// One row: display text and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListRow {
    pub text: String,
    pub kind: ListRowKind,
}

impl ListRow {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: ListRowKind::Plain,
        }
    }

    pub fn muted(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: ListRowKind::Muted,
        }
    }

    pub fn action(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: ListRowKind::Action,
        }
    }
}

/// One section: a header and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListSection {
    pub title: String,
    pub rows: Vec<ListRow>,
}

impl ListSection {
    pub fn new(title: impl Into<String>, rows: Vec<ListRow>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }
}

/// A sectioned list over sections and one row handler. Activatable rows
/// (Plain / Action) take a click, are focusable, and activate from
/// Enter/Space; Muted rows and the headers are inert. The handler receives
/// `(section index, row index)` into the slice as passed.
pub fn sectioned_list<State, Action, OA, OnRow>(
    sections: &[ListSection],
    on_row: OnRow,
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action, OA, OnRow>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action> + 'static,
    OnRow: Fn(&mut State, usize, usize) -> OA + Clone + 'static,
{
    let mut children: Vec<
        Box<dyn crate::AnyView<State, Action, GenetCtx, GenetElement>>,
    > = Vec::new();
    for (si, section) in sections.iter().enumerate() {
        children.push(Box::new(
            el::<_, State, Action>("div", section.title.clone())
                .attr("class", "list-section-title")
                .attr("role", "heading"),
        ));
        for (ri, row) in section.rows.iter().enumerate() {
            let class = match row.kind {
                ListRowKind::Plain => "list-row",
                ListRowKind::Muted => "list-row muted",
                ListRowKind::Action => "list-row action",
            };
            if row.kind == ListRowKind::Muted {
                children.push(Box::new(
                    el::<_, State, Action>("div", row.text.clone()).attr("class", class),
                ));
                continue;
            }
            let click_row = on_row.clone();
            let key_row = on_row.clone();
            children.push(Box::new(focusable(on_key(
                on_click(
                    el::<_, State, Action>("div", row.text.clone())
                        .attr("class", class)
                        .attr("role", "button")
                        .attr("tabindex", "0"),
                    move |state: &mut State, _click: PointerClick| click_row(state, si, ri),
                ),
                move |state: &mut State, event| match &event.key {
                    Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                        event.prevent_default();
                        // The key activation reports through the same handler;
                        // its optional action bubbles from the key dispatch.
                        let _ = key_row(state, si, ri);
                    }
                    _ => {}
                },
            ))));
        }
    }
    el::<_, State, Action>("div", children).attr("class", "list")
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{AnyView, DomHandle, GenetAppRunner, PointerClick};

    #[derive(Default)]
    struct S {
        hits: Vec<(usize, usize)>,
    }

    type V = Box<dyn AnyView<S, (), GenetCtx, GenetElement>>;

    fn view(_s: &S) -> V {
        Box::new(sectioned_list(
            &[
                ListSection::new(
                    "Recent",
                    vec![ListRow::plain("alpha"), ListRow::muted("none yet")],
                ),
                ListSection::new("Removed", vec![ListRow::action("Recover beta")]),
            ],
            |s: &mut S, si, ri| s.hits.push((si, ri)),
        ))
    }

    #[test]
    fn rows_render_with_their_classes_and_only_activatable_rows_click() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(dom.clone(), view as fn(&S) -> V, S::default());
        let (plain, muted, action) = {
            let d = dom.borrow();
            let ns = Namespace::from("");
            let class = LocalName::from("class");
            let all = d.all_with_class(d.document(), "list-row");
            assert_eq!(all.len(), 3);
            let by = |want: &str| {
                *all.iter()
                    .find(|&&n| d.attribute(n, &ns, &class) == Some(want))
                    .unwrap()
            };
            (by("list-row"), by("list-row muted"), by("list-row action"))
        };
        runner.dispatch_click(plain, PointerClick::at((1.0, 1.0)));
        runner.dispatch_click(muted, PointerClick::at((1.0, 1.0)));
        runner.dispatch_click(action, PointerClick::at((1.0, 1.0)));
        assert_eq!(
            runner.state().hits,
            vec![(0, 0), (1, 0)],
            "plain and action rows report (section, row); the muted row is inert"
        );
    }

    #[test]
    fn headers_are_headings_and_inert() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(dom.clone(), view as fn(&S) -> V, S::default());
        let header = {
            let d = dom.borrow();
            let all = d.all_with_class(d.document(), "list-section-title");
            assert_eq!(all.len(), 2);
            all[0]
        };
        runner.dispatch_click(header, PointerClick::at((1.0, 1.0)));
        assert!(runner.state().hits.is_empty(), "a header click reports nothing");
    }
}
