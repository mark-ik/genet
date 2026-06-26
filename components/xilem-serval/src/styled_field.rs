/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A text field rendered as per-range styled `<span>` runs — the style channel a
//! host paints syntax highlighting through.
//!
//! [`controls`](crate::controls) renders a field's text as plain `before` / `after`
//! strings split at the caret (plus the preedit + ghost spans). This module is the
//! styled sibling: given [`StyleRange`]s (a byte range plus a CSS string), it renders
//! the same text as a sequence of styled `<span>` runs. The styles are opaque CSS;
//! this module knows nothing of djot or any language, so the host computes the ranges
//! (for the knot editor, from jotdown highlight spans) and passes them in. The runs
//! concatenate to the same text the plain field shows, so the host's `caret_rect`
//! over them lines up exactly as over the plain field (the field is already laid out
//! as several inline nodes: before-text, preedit span, after-text, ghost span).
//!
//! Style ranges are byte ranges over the same buffer the host highlighted, so their
//! bounds fall on char boundaries; runs are sliced on those bounds.

use std::ops::Range;

use crate::controls::edit_multiline;
use crate::pod::ServalElement;
use crate::{el, on_key, AnyView, KeyEvent, ServalCtx, TextInput};

/// A styled run over the field text: a byte `range` painted with a CSS `style`
/// string (`"color: #c4a7e7; font-weight: 600;"`). Ranges may overlap and nest (a
/// heading containing emphasis, a code block containing a keyword);
/// [`styled_textarea`] flattens them innermost-wins into non-overlapping runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleRange {
    pub range: Range<usize>,
    pub style: String,
}

/// The boxed view a styled field returns. Its children are a dynamic `Vec` of span
/// runs, unlike [`TextField`](crate::TextField)'s fixed tuple, so the type is erased.
pub type StyledField = Box<dyn AnyView<TextInput, (), ServalCtx, ServalElement>>;

type Kid = Box<dyn AnyView<TextInput, (), ServalCtx, ServalElement>>;

/// Flatten possibly-overlapping `styles` over `len` bytes into non-overlapping runs,
/// the innermost (smallest) range winning on overlap. Runs cover `0..len` with no
/// gaps; a `None` style is unstyled text.
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
            paint[b] = Some(s.style.as_str());
        }
    }
    // Coalesce adjacent bytes carrying the same style into one run.
    let mut runs = Vec::new();
    let mut i = 0;
    while i < len {
        let style = paint[i];
        let start = i;
        while i < len && paint[i] == style {
            i += 1;
        }
        runs.push((start..i, style.map(str::to_string)));
    }
    runs
}

/// Emit the `runs` clipped to `[lo, hi)` as `<span>` children over `text`. A styled
/// run carries its CSS as a `style` attribute; an unstyled run is a bare `<span>`.
fn emit(kids: &mut Vec<Kid>, text: &str, runs: &[(Range<usize>, Option<String>)], lo: usize, hi: usize) {
    for (r, style) in runs {
        let start = r.start.max(lo);
        let end = r.end.min(hi);
        if start >= end {
            continue;
        }
        let span = el::<_, TextInput, ()>("span", text[start..end].to_string());
        let span = match style {
            Some(css) => span.attr("style", css.clone()),
            None => span,
        };
        kids.push(Box::new(span));
    }
}

/// Build a styled field body: the committed text as styled `<span>` runs, split at
/// the caret to splice the IME preedit (an underlined span), then the ghost suffix.
/// Mirrors `controls`'s `field_body`, but with per-range styling and dynamic
/// children. `tag` is `"textarea"` for the multi-line field.
fn styled_body(tag: &str, input: &TextInput, styles: &[StyleRange]) -> StyledField {
    let text = input.text();
    let (before, preedit, _after) = input.render_parts();
    let at = before.len();
    let runs = flatten(text.len(), styles);

    let mut kids: Vec<Kid> = Vec::new();
    emit(&mut kids, text, &runs, 0, at);
    if !preedit.is_empty() {
        kids.push(Box::new(
            el::<_, TextInput, ()>("span", preedit).attr("style", "text-decoration: underline;"),
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

    let handler: fn(&mut TextInput, KeyEvent) = edit_multiline;
    Box::new(on_key(el::<_, TextInput, ()>(tag, kids), handler))
}

/// A multi-line text field whose text is rendered as per-range styled `<span>` runs
/// from `styles` — the [`textarea`](crate::textarea) sibling that paints a host's
/// syntax highlighting. The `edit_multiline` handler and the caret / IME behaviour
/// are identical; only the rendering carries colour. The host recomputes `styles`
/// from the buffer (for example at view build) and passes them in.
pub fn styled_textarea(input: &TextInput, styles: &[StyleRange]) -> StyledField {
    styled_body("textarea", input, styles)
}
