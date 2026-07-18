/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The transclusion resolve pass — `include` fences fetched and spliced
//! (knot evaluation + export plan, K1).
//!
//! Engines stay pure: an ` ```include <url> ` fence parses as an ordinary
//! code block (its body is the authored **fallback**, shown wherever
//! resolution doesn't happen — which makes inert rendering the default
//! everywhere, for free). This pass is the host-driven other half: walk the
//! document, and for each include fence the policy allows, fetch the
//! target, render the response through whatever engine fits, splice the
//! produced blocks in place, and record where every spliced block came
//! from.
//!
//! **Policy is the caller's** (declarative data in, no ambient authority):
//! the host maps a knot's standing — own SelfAsserted note vs received
//! flora — onto a [`TranscludePolicy`]. The pass never fetches more than
//! the policy names, never follows more depth than it allows, and never
//! revisits a URL (cycle guard).
//!
//! Fetch and render arrive as closures so this file stays decoupled from
//! both the network stack and the routing layer; a host typically wraps
//! netfetcher (and, later, the smolweb clients) and its `EngineRegistry`.
//! The closures are synchronous in this first cut — an async host adapts
//! with its own executor at the closure boundary.
//!
//! v1 limit, stated: only top-level fences resolve (an include inside a
//! quote or list stays inert). Per-block provenance indexing is defined on
//! top-level blocks; lifting the limit comes with anchor-based provenance.

use super::block_provenance::{BlockProvenance, BlockProvenanceMap};
use super::{Block, EngineDocument};
use crate::engine::EngineInput;
use std::collections::HashSet;

/// What a fetch closure returns: the body plus the content type the
/// transport reported (`None` lets the renderer sniff).
#[derive(Clone, Debug)]
pub struct Fetched {
    pub content_type: Option<String>,
    pub body: String,
}

/// What the resolve pass may do, as plain data. The host builds one per
/// document from the document's standing (own note vs received) and the
/// user's settings; [`TransclusionPolicy::deny_all`] is the safe floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransclusionPolicy {
    /// Master switch — `false` renders every fence inert (the fallback).
    pub enabled: bool,
    /// URL schemes that may resolve (e.g. `https`, `gemini`). Anything
    /// else is denied with a reason.
    pub allowed_schemes: Vec<String>,
    /// How many include-within-included-content generations may resolve.
    /// `1` resolves only fences authored in the document itself.
    pub max_depth: u8,
}

impl TransclusionPolicy {
    /// The safe floor: nothing resolves. The right default for any
    /// received document until the user consents.
    pub fn deny_all() -> Self {
        Self {
            enabled: false,
            allowed_schemes: Vec::new(),
            max_depth: 0,
        }
    }

    /// A permissive policy for the user's **own** notes, per setting.
    pub fn for_own_notes(allowed_schemes: Vec<String>, max_depth: u8) -> Self {
        Self {
            enabled: true,
            allowed_schemes,
            max_depth,
        }
    }
}

/// What happened, faithfully: counts and reasons, plus the provenance of
/// every spliced block (top-level index → source), for the host to carry
/// the way clips carry theirs.
#[derive(Debug, Default)]
pub struct TranscludeOutcome {
    pub resolved: usize,
    /// `(url, reason)` for fences the policy refused.
    pub denied: Vec<(String, String)>,
    /// `(url, error)` for fences that resolved in policy but failed in
    /// fetch or render. Their fallback stays in place.
    pub failed: Vec<(String, String)>,
    pub provenance: BlockProvenanceMap,
}

/// Parse an include fence's info string: `include <url>` (anything after
/// the URL is reserved). Returns the URL.
pub fn parse_include(language: &str) -> Option<&str> {
    let mut tokens = language.split_whitespace();
    if tokens.next()? != "include" {
        return None;
    }
    tokens.next()
}

fn scheme_of(url: &str) -> Option<&str> {
    url.split_once("://").map(|(scheme, _)| scheme)
}

/// Resolve the document's top-level `include` fences in place. See the
/// module docs for the contract; the outcome reports everything that
/// happened (and didn't).
pub fn resolve_transclusions(
    document: &mut EngineDocument,
    fetch: &mut dyn FnMut(&str) -> Result<Fetched, String>,
    render: &mut dyn FnMut(&EngineInput) -> Result<EngineDocument, String>,
    policy: &TransclusionPolicy,
) -> TranscludeOutcome {
    let mut outcome = TranscludeOutcome::default();
    let mut visited: HashSet<String> = HashSet::new();

    // Each pass resolves the fences currently present; content spliced by
    // pass N is scanned in pass N+1, up to the policy's depth. A disabled
    // policy still gets one scan so every fence's denial is reported.
    let passes = policy.max_depth.max(1);
    for _pass in 0..passes {
        let mut any_resolved = false;
        let old_blocks = std::mem::take(&mut document.blocks);
        let old_provenance = std::mem::replace(&mut outcome.provenance, BlockProvenanceMap::new());
        let mut blocks: Vec<Block> = Vec::with_capacity(old_blocks.len());

        for (old_index, block) in old_blocks.into_iter().enumerate() {
            let new_index = blocks.len();
            // Provenance recorded for this block by an earlier pass moves
            // with it (indices shift as splices land before it).
            let carried = old_provenance.get(old_index).cloned();

            let include_url = match &block {
                Block::CodeBlock {
                    language: Some(language),
                    ..
                } => parse_include(language).map(str::to_string),
                _ => None,
            };

            // Every path that keeps the original block also keeps its
            // carried provenance.
            let keep = |block: Block, blocks: &mut Vec<Block>, outcome: &mut TranscludeOutcome| {
                if let Some(p) = carried.clone() {
                    outcome.provenance.insert(new_index, p);
                }
                blocks.push(block);
            };

            let Some(url) = include_url else {
                keep(block, &mut blocks, &mut outcome);
                continue;
            };

            if !policy.enabled {
                outcome.denied.push((url, "transclusion disabled".into()));
                keep(block, &mut blocks, &mut outcome);
                continue;
            }
            let scheme_allowed = scheme_of(&url)
                .map(|s| policy.allowed_schemes.iter().any(|a| a == s))
                .unwrap_or(false);
            if !scheme_allowed {
                outcome
                    .denied
                    .push((url, "scheme not in the allowlist".into()));
                keep(block, &mut blocks, &mut outcome);
                continue;
            }
            if !visited.insert(url.clone()) {
                outcome
                    .denied
                    .push((url, "already resolved (cycle guard)".into()));
                keep(block, &mut blocks, &mut outcome);
                continue;
            }

            let fetched = match fetch(&url) {
                Ok(fetched) => fetched,
                Err(error) => {
                    outcome.failed.push((url, error));
                    keep(block, &mut blocks, &mut outcome);
                    continue;
                },
            };
            let mut input = EngineInput::new(url.clone(), fetched.body);
            input.content_type = fetched.content_type;
            let child = match render(&input) {
                Ok(child) => child,
                Err(error) => {
                    outcome.failed.push((url, error));
                    keep(block, &mut blocks, &mut outcome);
                    continue;
                },
            };

            let source = BlockProvenance::from_document(child.provenance.clone());
            for (offset, child_block) in child.blocks.into_iter().enumerate() {
                outcome
                    .provenance
                    .insert(new_index + offset, source.clone());
                blocks.push(child_block);
            }
            outcome.resolved += 1;
            any_resolved = true;
        }

        document.blocks = blocks;
        if !any_resolved {
            break;
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentProvenance, DocumentTrustState, InlineSpan};

    fn doc_with(blocks: Vec<Block>) -> EngineDocument {
        EngineDocument {
            address: "test.knot".into(),
            title: None,
            content_type: "text/x-knot".into(),
            lang: None,
            provenance: DocumentProvenance::default(),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        }
    }

    fn include_fence(url: &str, fallback: &str) -> Block {
        Block::CodeBlock {
            language: Some(format!("include {url}")),
            text: fallback.to_string(),
        }
    }

    /// A render stub: every fetched body becomes one paragraph, and the
    /// child document's provenance carries the input address.
    fn stub_render(input: &EngineInput) -> Result<EngineDocument, String> {
        let mut child = doc_with(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text(input.body.clone())],
        }]);
        child.provenance.canonical_uri = Some(input.address.clone());
        Ok(child)
    }

    fn policy(schemes: &[&str], depth: u8) -> TransclusionPolicy {
        TransclusionPolicy::for_own_notes(schemes.iter().map(|s| s.to_string()).collect(), depth)
    }

    #[test]
    fn an_allowed_include_splices_with_provenance() {
        let mut document = doc_with(vec![
            Block::Paragraph {
                spans: vec![InlineSpan::Text("before".into())],
            },
            include_fence("gemini://x.test/page.gmi", "fallback"),
        ]);
        let mut fetch = |url: &str| {
            assert_eq!(url, "gemini://x.test/page.gmi");
            Ok(Fetched {
                content_type: Some("text/gemini".into()),
                body: "from the capsule".into(),
            })
        };
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut stub_render,
            &policy(&["gemini"], 1),
        );

        assert_eq!(outcome.resolved, 1);
        assert!(outcome.denied.is_empty() && outcome.failed.is_empty());
        assert_eq!(document.blocks.len(), 2);
        assert!(matches!(
            &document.blocks[1],
            Block::Paragraph { spans } if spans == &vec![InlineSpan::Text("from the capsule".into())]
        ));
        let provenance = outcome
            .provenance
            .get(1)
            .expect("spliced block has provenance");
        assert_eq!(
            provenance.provenance.canonical_uri.as_deref(),
            Some("gemini://x.test/page.gmi")
        );
    }

    #[test]
    fn policy_denies_keep_the_fallback_visible() {
        let mut document = doc_with(vec![include_fence("https://x.test/", "fallback")]);
        let mut fetch = |_: &str| -> Result<Fetched, String> {
            panic!("fetch must not run for a denied fence")
        };

        // Disabled entirely.
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut stub_render,
            &TransclusionPolicy::deny_all(),
        );
        assert_eq!(outcome.resolved, 0);
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));

        // Enabled but scheme not allowed.
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut stub_render,
            &policy(&["gemini"], 1),
        );
        assert_eq!(outcome.denied.len(), 1);
        assert!(outcome.denied[0].1.contains("allowlist"));
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));
    }

    #[test]
    fn depth_and_cycles_are_capped() {
        // The fetched content contains another include fence — rendered by
        // a stub that produces a fence pointing back at the SAME url.
        let mut document = doc_with(vec![include_fence("gemini://x.test/a", "")]);
        let mut fetch = |_: &str| {
            Ok(Fetched {
                content_type: None,
                body: "irrelevant".into(),
            })
        };
        let mut render = |input: &EngineInput| -> Result<EngineDocument, String> {
            Ok(doc_with(vec![include_fence("gemini://x.test/a", "")])).map(|mut d| {
                d.provenance.canonical_uri = Some(input.address.clone());
                d
            })
        };
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut render,
            &policy(&["gemini"], 3),
        );
        assert_eq!(outcome.resolved, 1, "the cycle resolves once");
        assert!(
            outcome.denied.iter().any(|(_, r)| r.contains("cycle")),
            "the second visit is refused: {:?}",
            outcome.denied
        );

        // Depth 1 never scans spliced content for more fences.
        let mut document = doc_with(vec![include_fence("gemini://x.test/b", "")]);
        let mut render_chain = |input: &EngineInput| -> Result<EngineDocument, String> {
            let mut d = doc_with(vec![include_fence("gemini://x.test/c", "")]);
            d.provenance.canonical_uri = Some(input.address.clone());
            Ok(d)
        };
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut render_chain,
            &policy(&["gemini"], 1),
        );
        assert_eq!(outcome.resolved, 1);
        assert!(
            matches!(&document.blocks[0], Block::CodeBlock { .. }),
            "the nested fence stays inert at depth 1"
        );
    }

    #[test]
    fn fetch_failures_keep_the_fallback_and_report() {
        let mut document = doc_with(vec![include_fence("gemini://down.test/", "fallback")]);
        let mut fetch = |_: &str| -> Result<Fetched, String> { Err("connection refused".into()) };
        let outcome = resolve_transclusions(
            &mut document,
            &mut fetch,
            &mut stub_render,
            &policy(&["gemini"], 2),
        );
        assert_eq!(outcome.failed.len(), 1);
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));
    }

    #[test]
    fn parse_include_is_strict() {
        assert_eq!(
            parse_include("include gemini://x.test/"),
            Some("gemini://x.test/")
        );
        assert_eq!(parse_include("include"), None);
        assert_eq!(parse_include("rust"), None);
        assert_eq!(parse_include("included gemini://x.test/"), None);
    }
}
