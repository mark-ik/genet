/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Syntax-highlighted text fields (the `highlight` feature).
//!
//! This is the assembly point that joins a lexer ([`illume`]) to a palette
//! ([`tinct`]) over Genet's styled field, so any Genet host gets highlighted
//! djot/code/entity editing for free — the omnibar, a note editor, a chat line,
//! a script prompt. The two libraries stay independent of each other; Cambium
//! is the one place allowed to know both, mapping illume's fine-grained
//! [`SyntaxKind`] (what was lexed) onto tinct's canonical [`SyntaxRole`] (what
//! colour), then naming a `syntax-*` CSS class a host stylesheet themes.
//!
//! Two halves, kept apart so colours stay themeable the Genet/stylo way:
//! - [`note_styles`] / [`entity_styles`] produce the `(range, class)`
//!   [`StyleRange`]s the styled field paints;
//! - [`syntax_css`] derives the actual colours from a theme's seeds (perceptual,
//!   contrast-gated) as `.syntax-* { color }` rules for the host stylesheet.
//!
//! The host picks the mode per surface with [`highlighted_textarea`] (multi-line
//! notes: djot structure + code injection + entities) and
//! [`highlighted_text_field`] (single-line: entities only, for an omnibar).

use illume::{Span, SyntaxKind, default_pack, entities, highlight};
use tinct::{Seeds, SyntaxRole, derive_syntax_palette};

use crate::TextInput;
use crate::styled_field::{StyleRange, styled_text_field, styled_textarea};

/// Which passes to run over a surface's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    /// A full note: djot structure + polyglot injection (illume's default pack)
    /// plus inline prose entities (urls, mentions, tags).
    Note,
    /// Inline prose entities only (urls, mentions, tags, emails) — for a single-line
    /// surface like the omnibar, which is not a djot document.
    Entities,
}

/// Map an illume lexer kind onto a tinct highlight role. `None` for kinds that
/// carry no colour of their own: a code/raw block region (its inner tokens are
/// coloured instead), bare identifiers, and the few structural kinds without a role.
fn syntax_role(kind: SyntaxKind) -> Option<SyntaxRole> {
    use SyntaxKind as K;
    use SyntaxRole as R;
    Some(match kind {
        K::Heading => R::Heading,
        K::Emphasis => R::Emphasis,
        K::Strong => R::Strong,
        K::Verbatim => R::Verbatim,
        K::Link | K::Image => R::Link,
        K::Blockquote => R::Quote,
        K::Keyword => R::Keyword,
        K::StringLit => R::String,
        K::Number => R::Number,
        K::Comment => R::Comment,
        K::Function => R::Function,
        K::Type => R::Type,
        K::Punctuation => R::Punctuation,
        K::Url | K::Email => R::Url,
        K::Mention => R::Mention,
        K::Tag => R::Tag,
        K::Strikethrough
        | K::Mark
        | K::Math
        | K::CodeBlock
        | K::RawBlock
        | K::Div
        | K::Identifier => return None,
    })
}

/// The CSS class a host stylesheet themes for `role` (`SyntaxRole::Keyword` →
/// `"syntax-keyword"`). The colours come from [`syntax_css`].
pub fn role_class(role: SyntaxRole) -> &'static str {
    use SyntaxRole as R;
    match role {
        R::Heading => "syntax-heading",
        R::Emphasis => "syntax-emphasis",
        R::Strong => "syntax-strong",
        R::Link => "syntax-link",
        R::Quote => "syntax-quote",
        R::Verbatim => "syntax-verbatim",
        R::Keyword => "syntax-keyword",
        R::Type => "syntax-type",
        R::Function => "syntax-function",
        R::String => "syntax-string",
        R::Number => "syntax-number",
        R::Comment => "syntax-comment",
        R::Punctuation => "syntax-punctuation",
        R::Url => "syntax-url",
        R::Mention => "syntax-mention",
        R::Tag => "syntax-tag",
    }
}

/// Push a span's themed [`StyleRange`] onto `out`, skipping kinds with no role.
fn push_styled(out: &mut Vec<StyleRange>, span: Span) {
    if let Some(role) = syntax_role(span.kind) {
        out.push(StyleRange {
            range: span.range,
            class: role_class(role).to_string(),
        });
    }
}

/// The styled ranges for a full note: djot structure + polyglot injection
/// (illume's default pack) plus inline prose entities, each mapped to its themed
/// class. The styled field paints these; unmapped kinds are skipped.
pub fn note_styles(text: &str) -> Vec<StyleRange> {
    let registry = default_pack();
    let mut styles = Vec::new();
    for span in highlight(text, &registry) {
        push_styled(&mut styles, span);
    }
    for span in entities(text) {
        push_styled(&mut styles, span);
    }
    styles
}

/// The styled ranges for a single-line surface (an omnibar): only the inline
/// entities (urls, mentions, tags, emails). Unlike [`note_styles`] it runs no djot
/// structure pass (the omnibar is not a note).
pub fn entity_styles(text: &str) -> Vec<StyleRange> {
    let mut styles = Vec::new();
    for span in entities(text) {
        push_styled(&mut styles, span);
    }
    styles
}

/// The styled ranges for `text` under `mode`.
pub fn styles_for(text: &str, mode: Highlight) -> Vec<StyleRange> {
    match mode {
        Highlight::Note => note_styles(text),
        Highlight::Entities => entity_styles(text),
    }
}

/// A multi-line text field that highlights its buffer under `mode` — the styled
/// [`textarea`](crate::textarea) with the lexer wired in. The host recomputes the
/// styles from the buffer at view-build; pair with [`syntax_css`] in the host
/// stylesheet to colour the classes.
pub fn highlighted_textarea(input: &TextInput, mode: Highlight) -> crate::TextField {
    styled_textarea(input, &styles_for(input.text(), mode))
}

/// A single-line text field that highlights its buffer under `mode` — the styled
/// [`text_field`](crate::text_field) sibling of [`highlighted_textarea`], for an
/// omnibar or other one-line input.
pub fn highlighted_text_field(input: &TextInput, mode: Highlight) -> crate::TextField {
    styled_text_field(input, &styles_for(input.text(), mode))
}

/// `.syntax-* { color }` rules colouring the highlight classes from tinct's derived
/// syntax palette, one per role, for the host stylesheet. Derived from the active
/// theme's `seeds` (perceptual, contrast-gated against the surface), so the syntax
/// colours track a theme switch; the styled field's spans carry the classes these
/// rules theme.
pub fn syntax_css(seeds: &Seeds) -> Vec<String> {
    let palette = derive_syntax_palette(seeds);
    SyntaxRole::ALL
        .iter()
        .map(|&role| {
            let c = palette.role(role);
            format!(
                ".{} {{ color: rgb({}, {}, {}); }}",
                role_class(role),
                c.r,
                c.g,
                c.b
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_styles_highlight_structure_and_entities() {
        let styles = note_styles("# Title\n\nsee @ada and https://ex.com\n");
        let classes: Vec<&str> = styles.iter().map(|s| s.class.as_str()).collect();
        assert!(classes.contains(&"syntax-heading"), "{classes:?}");
        assert!(classes.contains(&"syntax-mention"), "{classes:?}");
        assert!(classes.contains(&"syntax-url"), "{classes:?}");
    }

    #[test]
    fn entity_styles_skip_djot_structure() {
        // A leading `#` is a tag here (entities pass), not a heading (no djot pass).
        let styles = entity_styles("visit https://ex.com or @ada #web");
        let classes: Vec<&str> = styles.iter().map(|s| s.class.as_str()).collect();
        assert!(classes.contains(&"syntax-url"), "{classes:?}");
        assert!(classes.contains(&"syntax-mention"), "{classes:?}");
        assert!(classes.contains(&"syntax-tag"), "{classes:?}");
        assert!(!classes.contains(&"syntax-heading"), "{classes:?}");
    }

    #[test]
    fn every_role_has_a_distinct_class() {
        let mut seen = std::collections::HashSet::new();
        for role in SyntaxRole::ALL {
            assert!(
                seen.insert(role_class(role)),
                "duplicate class for {role:?}"
            );
        }
    }
}
