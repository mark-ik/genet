/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The knot readout: derives the editor's views of a knot from its source text.
//!
//! Holds only the reusable machinery, the injection registry (this crate's full
//! pack, so polyglot blocks colour) and the render engine, built once and reused.
//! The source text is the host's (its text widget's buffer), passed to each call, so
//! there is one buffer, not a second copy here. It turns that text into the things
//! the editor surface draws: the highlight / structure (via the portable [`illume`]
//! pipe), the heading outline + folds, and the rendered preview (via the same
//! `DjotKnotEngine` the rest of the app renders knots through).

use illume::{Fold, InjectionRegistry, OutlineItem, Span, folds, highlight, outline};
use inker::{Engine, EngineDocument, EngineError, EngineInput};
use nematic::DjotKnotEngine;

/// Derives the editor's views of a knot. Holds the injection registry it highlights
/// with and the engine it renders the preview through; the source text is passed to
/// each method (the host owns the one buffer). Build it once and reuse it across
/// edits and across notes; re-derive on edit (cheap at note size).
pub struct KnotReadout {
    registry: InjectionRegistry,
    engine: DjotKnotEngine,
}

impl KnotReadout {
    /// Build the readout: the injection registry (the full pack) plus the render
    /// engine, both reused across every derivation.
    pub fn new() -> Self {
        Self {
            registry: crate::full_pack(),
            engine: DjotKnotEngine::new(),
        }
    }

    /// Highlight spans over `text`: djot structure plus injected inner-language
    /// colouring for polyglot blocks. The `(range, kind)` channel the edit surface
    /// paints.
    pub fn highlights(&self, text: &str) -> Vec<Span> {
        highlight(text, &self.registry)
    }

    /// The heading outline (levels + text) of `text`, for the gloss outline lens.
    pub fn outline(&self, text: &str) -> Vec<OutlineItem> {
        outline(text)
    }

    /// Collapsible regions of `text`.
    pub fn folds(&self, text: &str) -> Vec<Fold> {
        folds(text)
    }

    /// The rendered preview of `text` addressed at `address` (a `mere://` / `knot://`
    /// node URL once that exists, or a placeholder for a scratch note): run through
    /// `DjotKnotEngine` into the portable block model the document-canvas pane draws.
    /// Errors only on engine failure (malformed input still yields a best-effort doc).
    pub fn rendered(&self, address: &str, text: &str) -> Result<EngineDocument, EngineError> {
        self.engine
            .render(&EngineInput::new(address.to_string(), text.to_string()))
    }
}

impl Default for KnotReadout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use illume::SyntaxKind;

    const SAMPLE: &str = "# Title\n\nSome _em_ and a code block:\n\n```json\n{\"a\": 1}\n```\n";

    #[test]
    fn highlights_cover_djot_and_injected_json() {
        let r = KnotReadout::new();
        let hl = r.highlights(SAMPLE);
        assert!(
            hl.iter().any(|s| s.kind == SyntaxKind::Heading),
            "djot heading: {hl:?}"
        );
        assert!(
            hl.iter().any(|s| s.kind == SyntaxKind::StringLit),
            "injected json string: {hl:?}"
        );
        assert!(hl.iter().any(|s| s.kind == SyntaxKind::CodeBlock));
    }

    #[test]
    fn renders_a_preview_document_through_the_engine() {
        let r = KnotReadout::new();
        let doc = r
            .rendered("mere://note/test", SAMPLE)
            .expect("engine renders");
        assert!(!doc.blocks.is_empty(), "preview should have blocks");
        assert_eq!(doc.content_type, "text/x-knot");
    }

    #[test]
    fn outline_finds_the_heading() {
        let r = KnotReadout::new();
        let items = r.outline(SAMPLE);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "Title");
    }

    #[test]
    fn one_readout_derives_any_text() {
        // The readout holds no source: the same instance derives different buffers,
        // so the host's text widget stays the single source of truth.
        let r = KnotReadout::new();
        assert_eq!(r.outline("# One\n").len(), 1);
        assert_eq!(r.outline("# One\n\n## Two\n").len(), 2);
    }
}
