/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A detail panel: key/value rows grouped under section headers, all inert.
//!
//! Consumer-pull (merecat, 2026-07-17): the Inspector pane renders exactly
//! this shape — sections of labelled facts (node identity, content lifecycle,
//! document structure) — and the surfaces-in-cambium mapping names it the
//! pane's expression. Steward's status rows and Apparatus's diagnostics
//! sections are the predicted next consumers. Not grown out of
//! [`sectioned_list`](crate::sectioned_list): that component's rows are
//! single-text and activatable; a detail row is a two-part label/value pair
//! and never activates. The two stay separate on purpose.
//!
//! No geometry: rows are normal-flow blocks and the host's sheet gives them
//! their type and colours (`detail-section-title`, `detail-row`,
//! `detail-key`, `detail-value`). Purely informational — nothing here takes
//! a click or focus; a pane that needs an affordance beside a fact composes
//! one alongside.

use crate::pod::GenetElement;
use crate::{AnyView, GenetCtx, View, el};

/// One row: a fact's label and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailRow {
    pub key: String,
    pub value: String,
}

impl DetailRow {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

/// One section: a header and its rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailSection {
    pub title: String,
    pub rows: Vec<DetailRow>,
}

impl DetailSection {
    pub fn new(title: impl Into<String>, rows: Vec<DetailRow>) -> Self {
        Self {
            title: title.into(),
            rows,
        }
    }
}

/// A detail panel over sections of key/value rows. Everything is inert; the
/// headers carry a heading role for the a11y projection.
pub fn detail_panel<State, Action>(
    sections: &[DetailSection],
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action>
where
    State: 'static,
    Action: 'static,
{
    let mut children: Vec<Box<dyn AnyView<State, Action, GenetCtx, GenetElement>>> = Vec::new();
    for section in sections {
        children.push(Box::new(
            el::<_, State, Action>("div", section.title.clone())
                .attr("class", "detail-section-title")
                .attr("role", "heading"),
        ));
        for row in &section.rows {
            let key: Box<dyn AnyView<State, Action, GenetCtx, GenetElement>> = Box::new(
                el::<_, State, Action>("span", row.key.clone()).attr("class", "detail-key"),
            );
            let value: Box<dyn AnyView<State, Action, GenetCtx, GenetElement>> = Box::new(
                el::<_, State, Action>("span", row.value.clone()).attr("class", "detail-value"),
            );
            children.push(Box::new(
                el::<_, State, Action>("div", vec![key, value]).attr("class", "detail-row"),
            ));
        }
    }
    el::<_, State, Action>("div", children).attr("class", "detail")
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::LayoutDom;

    use super::*;
    use crate::{DomHandle, GenetAppRunner, PointerClick};

    #[derive(Default)]
    struct S {
        hits: usize,
    }

    type V = Box<dyn AnyView<S, (), GenetCtx, GenetElement>>;

    fn view(_s: &S) -> V {
        Box::new(detail_panel(&[
            DetailSection::new(
                "Node",
                vec![
                    DetailRow::new("Title", "The Page"),
                    DetailRow::new("URL", "https://example.test/"),
                ],
            ),
            DetailSection::new("Content", vec![DetailRow::new("Fetch state", "live")]),
        ]))
    }

    fn texts(dom: &ScriptedDom, class: &str) -> Vec<String> {
        dom.all_with_class(dom.document(), class)
            .into_iter()
            .map(|n| {
                dom.dom_children(n)
                    .filter_map(|c| dom.text(c).map(str::to_string))
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn sections_and_rows_render_with_their_classes() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let _runner = GenetAppRunner::new(dom.clone(), view as fn(&S) -> V, S::default());
        let d = dom.borrow();
        assert_eq!(texts(&d, "detail-section-title"), vec!["Node", "Content"]);
        assert_eq!(d.all_with_class(d.document(), "detail-row").len(), 3);
        assert_eq!(
            texts(&d, "detail-key"),
            vec!["Title", "URL", "Fetch state"]
        );
        assert_eq!(
            texts(&d, "detail-value"),
            vec!["The Page", "https://example.test/", "live"]
        );
    }

    /// Every part of the panel is inert: a click anywhere reports nothing.
    #[test]
    fn the_panel_is_inert() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let mut runner = GenetAppRunner::new(dom.clone(), view as fn(&S) -> V, S::default());
        let targets: Vec<_> = {
            let d = dom.borrow();
            ["detail-section-title", "detail-row", "detail-key", "detail-value"]
                .iter()
                .flat_map(|c| d.all_with_class(d.document(), c))
                .collect()
        };
        for node in targets {
            runner.dispatch_click(node, PointerClick::at((1.0, 1.0)));
        }
        assert_eq!(runner.state().hits, 0, "nothing in a detail panel activates");
    }
}
