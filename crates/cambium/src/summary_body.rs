/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A context-neutral titled record body for rows, cards, panels, and popovers.

use crate::{GenetCtx, GenetElement, View, el};

/// Content carried by a [`summary_body`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryBody {
    pub id: String,
    pub title: String,
    pub eyebrow: Option<String>,
    pub description: Option<String>,
    pub facts: Vec<(String, String)>,
}

impl SummaryBody {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            eyebrow: None,
            description: None,
            facts: Vec::new(),
        }
    }

    pub fn with_eyebrow(mut self, eyebrow: impl Into<String>) -> Self {
        self.eyebrow = Some(eyebrow.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_fact(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.facts.push((label.into(), value.into()));
        self
    }
}

/// Render semantic summary content without choosing its surrounding surface.
///
/// The root is a labelled group instead of an `article` or heading so callers
/// can place it inside a row, card, panel, or popover without invalid nesting.
pub fn summary_body<State, Action>(
    summary: &SummaryBody,
) -> impl View<State, Action, GenetCtx, Element = GenetElement> + use<State, Action>
where
    State: 'static,
    Action: 'static,
{
    let title_id = format!("{}-title", summary.id);
    let eyebrow = summary.eyebrow.as_ref().map(|eyebrow| {
        el::<_, State, Action>("span", eyebrow.clone()).attr("class", "summary-eyebrow")
    });
    let description = summary.description.as_ref().map(|description| {
        el::<_, State, Action>("p", description.clone()).attr("class", "summary-description")
    });
    let facts: Vec<_> = summary
        .facts
        .iter()
        .map(|(label, value)| {
            el::<_, State, Action>(
                "div",
                (
                    el::<_, State, Action>("dt", label.clone()),
                    el::<_, State, Action>("dd", value.clone()),
                ),
            )
            .attr("class", "summary-fact")
        })
        .collect();
    let facts = (!facts.is_empty())
        .then(|| el::<_, State, Action>("dl", facts).attr("class", "summary-facts"));

    el::<_, State, Action>(
        "div",
        (
            eyebrow,
            el::<_, State, Action>("span", summary.title.clone())
                .attr("id", title_id.clone())
                .attr("class", "summary-title"),
            description,
            facts,
        ),
    )
    .attr("id", summary.id.clone())
    .attr("class", "summary-body")
    .attr("role", "group")
    .attr("aria-labelledby", title_id)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::{LayoutDom, LocalName, Namespace};

    use super::*;
    use crate::{DomHandle, GenetAppRunner};

    #[test]
    fn summary_is_a_context_neutral_labelled_group() {
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let summary = SummaryBody::new("node-summary", "Field notes")
            .with_eyebrow("Document")
            .with_description("Observations from the north trail")
            .with_fact("Links", "12");
        let runner = GenetAppRunner::<_, _, _, ()>::new(
            dom.clone(),
            move |_: &()| summary_body::<(), ()>(&summary),
            (),
        );
        let dom = dom.borrow();
        assert_eq!(
            dom.attribute(
                runner.root(),
                &Namespace::from(""),
                &LocalName::from("role")
            ),
            Some("group")
        );
        assert_eq!(
            dom.attribute(
                runner.root(),
                &Namespace::from(""),
                &LocalName::from("aria-labelledby")
            ),
            Some("node-summary-title")
        );
    }
}
