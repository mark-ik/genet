/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Statement extraction from knot documents (knot design doc §10.5 Phase 4;
//! linked-data plan Phase 0) — the pure half.
//!
//! A knot inline link can carry an open *predicate* in its `rel`
//! (`[Topic](mere://node/topic){rel=cites}`). [`link_statements`] is a pure
//! walk of a document's blocks, collecting every predicate-bearing inline
//! link as a [`LinkStatement`] — no graph, no host mutation. (Plain
//! navigation links, with no `rel`, are not statements and are excluded.)
//!
//! Resolving each `rel` against the host's relation vocabulary and asserting
//! graph edges is the host's job, deliberately outside this crate: mere's
//! `linked-data` crate carries `apply_link_statements` (statements-over-schema
//! ingest), consuming this walk's output. That keeps inker kernel-free and the
//! statement *vocabulary* portable to any host.

use crate::{Block, EngineDocument, InlineSpan};

/// A predicate-bearing inline link extracted from a knot document: the link's
/// target URL plus the verbatim `rel` value (a bare slug like `cites`, a full
/// vocabulary IRI, or a raw CURIE). The `rel` is resolved against the host's
/// relation vocabulary at apply time, not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStatement {
    pub target_url: String,
    pub rel: String,
}

/// Walk a document's blocks and collect every predicate-bearing inline link, in
/// document order. Pure: no graph, no host mutation.
pub fn link_statements(doc: &EngineDocument) -> Vec<LinkStatement> {
    let mut out = Vec::new();
    for block in &doc.blocks {
        collect_block(block, &mut out);
    }
    out
}

fn collect_block(block: &Block, out: &mut Vec<LinkStatement>) {
    match block {
        Block::Heading { spans, .. } | Block::Paragraph { spans } => {
            for span in spans {
                collect_span(span, out);
            }
        },
        Block::Quote { blocks } => {
            for inner in blocks {
                collect_block(inner, out);
            }
        },
        Block::List { items, .. } => {
            for item in items {
                for inner in item {
                    collect_block(inner, out);
                }
            }
        },
        Block::Table { header, rows, .. } => {
            for cell in header.iter().chain(rows.iter().flatten()) {
                for span in cell {
                    collect_span(span, out);
                }
            }
        },
        // Feed blocks carry navigation URLs (article / source), not `rel`
        // statements; the remaining variants hold no inline links.
        Block::FeedHeader { .. }
        | Block::FeedEntry { .. }
        | Block::CodeBlock { .. }
        | Block::Image { .. }
        | Block::Preformatted { .. }
        | Block::Rule
        | Block::MetadataRow { .. }
        | Block::Badge { .. } => {},
    }
}

fn collect_span(span: &InlineSpan, out: &mut Vec<LinkStatement>) {
    match span {
        InlineSpan::Link {
            url,
            predicate,
            spans,
            ..
        } => {
            if let Some(rel) = predicate {
                out.push(LinkStatement {
                    target_url: url.clone(),
                    rel: rel.clone(),
                });
            }
            for inner in spans {
                collect_span(inner, out);
            }
        },
        InlineSpan::Emphasis(inner) | InlineSpan::Strong(inner) => {
            for s in inner {
                collect_span(s, out);
            }
        },
        InlineSpan::Text(_)
        | InlineSpan::Code(_)
        | InlineSpan::LineBreak
        | InlineSpan::SoftBreak => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentProvenance, DocumentTrustState};

    fn doc(blocks: Vec<Block>) -> EngineDocument {
        EngineDocument {
            address: "knot:test".to_string(),
            title: None,
            content_type: "text/x-knot".to_string(),
            lang: None,
            provenance: DocumentProvenance::default(),
            trust: DocumentTrustState::default(),
            diagnostics: Vec::new(),
            blocks,
        }
    }

    #[test]
    fn link_statements_extracts_predicate_links_only() {
        let document = doc(vec![Block::Paragraph {
            spans: vec![
                InlineSpan::Link {
                    url: "https://plain.test/".to_string(),
                    title: None,
                    spans: vec![InlineSpan::Text("plain".to_string())],
                    predicate: None,
                },
                InlineSpan::Link {
                    url: "mere://node/topic".to_string(),
                    title: None,
                    spans: vec![InlineSpan::Text("Topic".to_string())],
                    predicate: Some("cites".to_string()),
                },
            ],
        }]);
        assert_eq!(
            link_statements(&document),
            vec![LinkStatement {
                target_url: "mere://node/topic".to_string(),
                rel: "cites".to_string(),
            }]
        );
    }
}
