/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The field-rendering layer: one style-aware field body shared by the plain
//! [`text_field`](crate::text_field) / [`textarea`](crate::textarea) and the
//! highlighting [`styled_textarea`].
//!
//! It renders a [`TextInput`]'s text as the children of the field element:
//! unstyled runs as text nodes, styled runs as `<span class="…">` runs a host's
//! stylesheet themes, with the IME preedit and ghost-completion spans spliced at
//! the caret. The plain field passes no styles (the empty case), so there is one
//! body, not a styled fork of the plain one.
//!
//! Styling carries a *class*, not inline CSS, so the host themes the highlight
//! through one stylesheet (the colours derive from tinct's syntax palette). The
//! runs concatenate to the same text, so the host's `caret_rect` lines up exactly
//! as over the plain field (which is already several inline nodes: text, the
//! preedit span, text, the ghost span). Style ranges are byte ranges over the same
//! buffer the host highlighted, so their bounds fall on char boundaries.

use std::ops::Range;

use crate::controls::{TextInput, edit, edit_multiline};
use crate::pod::GenetElement;
use crate::{AnyView, GenetCtx, KeyEvent, el, on_key};

/// A styled run over the field text: a byte `range` painted with a CSS `class`
/// (`"syntax-keyword"`) the host's stylesheet themes. Ranges may overlap and nest
/// (a heading containing emphasis, a code block containing a keyword);
/// The field builder flattens them innermost-wins into non-overlapping runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleRange {
    pub range: Range<usize>,
    pub class: String,
}

/// One child of a field element: a text node or a styled `<span>`, type-erased so
/// the field's children are a uniform `Vec`. Public so the [`crate::TextField`] alias can
/// name it.
pub type FieldChild = Box<dyn AnyView<TextInput, (), GenetCtx, GenetElement>>;

/// Flatten possibly-overlapping `styles` over `len` bytes into non-overlapping
/// runs, the innermost (smallest) range winning on overlap. Runs cover `0..len`
/// with no gaps; a `None` class is unstyled text. Empty `styles` yields a single
/// `None` run (the plain field).
fn flatten(len: usize, styles: &[StyleRange]) -> Vec<(Range<usize>, Option<String>)> {
    // Paint per byte, largest range first so a smaller (inner) range overwrites it.
    let mut ordered: Vec<&StyleRange> = styles
        .iter()
        .filter(|s| s.range.start < s.range.end && s.range.end <= len)
        .collect();
    ordered.sort_by_key(|s| std::cmp::Reverse(s.range.end - s.range.start));
    let mut paint: Vec<Option<&str>> = vec![None; len];
    for s in ordered {
        for b in s.range.clone() {
            paint[b] = Some(s.class.as_str());
        }
    }
    // Coalesce adjacent bytes carrying the same class into one run.
    let mut runs = Vec::new();
    let mut i = 0;
    while i < len {
        let class = paint[i];
        let start = i;
        while i < len && paint[i] == class {
            i += 1;
        }
        runs.push((start..i, class.map(str::to_string)));
    }
    runs
}

/// Emit the `runs` clipped to `[lo, hi)` over `text`: a styled run as a
/// `<span class="…">`, an unstyled run as a bare text node.
fn emit(
    kids: &mut Vec<FieldChild>,
    text: &str,
    runs: &[(Range<usize>, Option<String>)],
    lo: usize,
    hi: usize,
) {
    for (r, class) in runs {
        let start = r.start.max(lo);
        let end = r.end.min(hi);
        if start >= end {
            continue;
        }
        let slice = text[start..end].to_string();
        match class {
            Some(c) => kids.push(Box::new(
                el::<_, TextInput, ()>("span", slice).attr("class", c.clone()),
            )),
            None => kids.push(Box::new(slice)),
        }
    }
}

/// The children of a field element: the committed text as (styled) runs split at
/// the caret to splice the IME preedit (an underlined span), then the ghost suffix.
/// Empty `styles` renders the plain field (unstyled text nodes); non-empty paints
/// the highlight classes. This is the one body behind the plain and styled fields.
pub(crate) fn field_children(input: &TextInput, styles: &[StyleRange]) -> Vec<FieldChild> {
    field_children_impl(input, styles, false)
}

/// The class on the in-flow caret span a [`caret_text_field`] renders, for the
/// host's stylesheet to theme (colour, weight — e.g. `.field-caret { color: … }`).
pub const FIELD_CARET_CLASS: &str = "field-caret";

/// The one field body, with or without a rendered caret. `show_caret` splices a
/// `<span class="field-caret">▍</span>` at the caret split, *after* the preedit
/// (composition text lands before the caret, matching platform IME fields).
fn field_children_impl(
    input: &TextInput,
    styles: &[StyleRange],
    show_caret: bool,
) -> Vec<FieldChild> {
    let text = input.text();
    let (before, preedit, _after) = input.render_parts();
    let at = before.len();
    let runs = flatten(text.len(), styles);

    let mut kids: Vec<FieldChild> = Vec::new();
    emit(&mut kids, text, &runs, 0, at);
    if !preedit.is_empty() {
        kids.push(Box::new(
            el::<_, TextInput, ()>("span", preedit)
                .attr("style", "text-decoration-line: underline;"),
        ));
    }
    if show_caret {
        kids.push(Box::new(
            el::<_, TextInput, ()>("span", "▍").attr("class", FIELD_CARET_CLASS),
        ));
    }
    emit(&mut kids, text, &runs, at, text.len());
    let ghost = input.ghost();
    if !ghost.is_empty() {
        kids.push(Box::new(
            el::<_, TextInput, ()>("span", ghost.to_string())
                .attr("style", "color: #8b91a0; font-style: italic;"),
        ));
    }
    kids
}

/// A multi-line text field rendered with per-range syntax highlighting from
/// `styles` (the [`textarea`](crate::textarea) sibling that paints a host's
/// classes). Same `edit_multiline` handler and caret / IME behaviour as the plain
/// field; only the rendering carries the classes. The host recomputes `styles`
/// from the buffer (for example at view build) and passes them in.
pub fn styled_textarea(input: &TextInput, styles: &[StyleRange]) -> crate::TextField {
    on_key(
        el::<_, TextInput, ()>("textarea", field_children(input, styles)),
        edit_multiline as fn(&mut TextInput, KeyEvent),
    )
}

/// A single-line text field with per-range highlighting from `styles` — the
/// [`text_field`](crate::text_field) sibling (the `edit` handler and an `<input>`
/// tag). Same caret / IME behaviour as the plain field; only the rendering carries
/// the classes. Lets a host highlight the omnibar (urls, command tokens) the way the
/// editor highlights a note.
pub fn styled_text_field(input: &TextInput, styles: &[StyleRange]) -> crate::TextField {
    on_key(
        el::<_, TextInput, ()>("input", field_children(input, styles)),
        edit as fn(&mut TextInput, KeyEvent),
    )
}

/// A single-line field that renders its own caret: the `▍` glyph as an in-flow
/// `<span class="field-caret">` at the caret split (after any preedit), themed by
/// the host through [`FIELD_CARET_CLASS`]. For hosts that paint the whole scene
/// from the DOM (no overlay layer) — merecat's omnibar is the first consumer.
///
/// Trade-off vs [`text_field`](crate::text_field): the glyph is in flow, so the
/// runs no longer concatenate to exactly the committed text and a `caret_rect`
/// overlay would land a glyph-width off past the split. A host draws the caret
/// one way or the other — pick this constructor *instead of* an overlay, not
/// alongside one. Focus-conditional display is the host's call too: build with
/// this constructor when focused, [`styled_text_field`] when not.
pub fn caret_text_field(input: &TextInput, styles: &[StyleRange]) -> crate::TextField {
    on_key(
        el::<_, TextInput, ()>("input", field_children_impl(input, styles, true)),
        edit as fn(&mut TextInput, KeyEvent),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_empty_styles_is_one_unstyled_run() {
        assert_eq!(flatten(5, &[]), vec![(0..5, None)]);
    }

    #[test]
    fn flatten_innermost_wins() {
        // Outer 0..10 "a" with inner 2..5 "b": the inner range overrides.
        let styles = vec![
            StyleRange {
                range: 0..10,
                class: "a".into(),
            },
            StyleRange {
                range: 2..5,
                class: "b".into(),
            },
        ];
        assert_eq!(
            flatten(10, &styles),
            vec![
                (0..2, Some("a".into())),
                (2..5, Some("b".into())),
                (5..10, Some("a".into())),
            ]
        );
    }

    #[test]
    fn flatten_drops_out_of_range_styles() {
        let styles = vec![StyleRange {
            range: 3..99,
            class: "x".into(),
        }];
        // end past len is filtered, leaving a plain run.
        assert_eq!(flatten(4, &styles), vec![(0..4, None)]);
    }

    /// The caret field mounts with the `▍` span *at the split*: text-before,
    /// `<span class="field-caret">`, text-after — the in-flow caret a
    /// scene-painting host renders instead of an overlay.
    #[test]
    fn caret_field_renders_the_caret_span_at_the_split() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use genet_scripted_dom::ScriptedDom;
        use layout_dom_api::{LayoutDom, LocalName, Namespace, NodeKind};

        use crate::runner::GenetAppRunner;
        use crate::{DomHandle, TextField};

        let mut input = TextInput::default();
        input.insert_str("abcd");
        input.set_caret_byte(2, false);

        fn view(s: &TextInput) -> TextField {
            super::caret_text_field(s, &[])
        }
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let runner = GenetAppRunner::new(dom.clone(), view, input);

        let dom = runner.dom();
        let dom = dom.borrow();
        let kids: Vec<_> = dom.dom_children(runner.root()).collect();
        assert_eq!(kids.len(), 3, "text-before, caret span, text-after");
        assert_eq!(dom.text(kids[0]), Some("ab"));
        assert_eq!(dom.kind(kids[1]), NodeKind::Element);
        assert_eq!(
            dom.attribute(kids[1], &Namespace::from(""), &LocalName::from("class")),
            Some(super::FIELD_CARET_CLASS)
        );
        assert_eq!(dom.text(kids[2]), Some("cd"));
    }
}
