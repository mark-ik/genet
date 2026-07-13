/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The script-fence evaluate pass — `<lang> eval` fences run and render
//! inline (knot evaluation + export plan, K2).
//!
//! Engines stay pure: an ` ```lua eval ` fence parses as an ordinary code
//! block (a plain ` ```lua ` fence stays a code *sample* — evaluation is the
//! ` eval ` opt-in). This pass is the host-driven other half, the
//! [`transclude`](super::transclude) sibling: walk the document, and for each
//! eval fence the policy allows, run it through the host's evaluator, then
//! render the output inline — plain text becomes a block directly, while a
//! script that declares it returned `gemtext` / `markdown` / `djot` is
//! nested-rendered through the engine registry (the org-babel move).
//!
//! **Policy is the caller's** (declarative data, no ambient authority): the
//! host maps a knot's standing — own SelfAsserted note vs received flora —
//! onto an [`EvaluationPolicy`]. The stricter default than transclusion:
//! received documents evaluate nothing. The evaluator and the renderer arrive
//! as closures, so this file is decoupled from any script engine (piccolo
//! today, JS later) and from the routing layer.
//!
//! v1 limits, stated: top-level fences only, and evaluation output is **not**
//! re-scanned for further eval fences (a script cannot fan out evaluation).

use super::block_provenance::{BlockProvenance, BlockProvenanceMap};
use super::{Block, DocumentProvenance, EngineDocument};
use crate::engine::EngineInput;

/// What an evaluator returns: a format tag and the text it produced. `plain`
/// (or empty) renders as a text block; `gemtext` / `markdown` / `djot` are
/// nested-rendered through the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvalOutput {
    pub format: String,
    pub text: String,
}

impl EvalOutput {
    /// Plain-text output (the default when a script returns a single value).
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            format: "plain".into(),
            text: text.into(),
        }
    }

    /// Classify a plain-text result by a cheap leading-marker heuristic: a
    /// gemtext heading (`#`/`##`) or link (`=>`) line tags it `gemtext` so it
    /// nested-renders; otherwise `plain`. A convenience for backends without a
    /// richer format-declaration convention (a script that wants to be
    /// explicit returns an `EvalOutput` directly).
    pub fn detect(text: impl Into<String>) -> Self {
        let text = text.into();
        let gemtext = text.starts_with("# ")
            || text.starts_with("## ")
            || text.starts_with("### ")
            || text.starts_with("=> ")
            || text.contains("\n=> ");
        Self {
            format: if gemtext { "gemtext" } else { "plain" }.into(),
            text,
        }
    }
}

/// A pluggable knot-block evaluator: one scripting language behind the eval
/// fence lane.
///
/// This is the **thin slice** the knot path needs — evaluate a source string
/// under a budget and hand back text — deliberately distinct from genet's
/// full DOM-shaped `ScriptEngine` seam (reflectors, host promises, native
/// callbacks), which mod and DOM scripting use. Lua, Rhai, and JS all satisfy
/// this cheaply; a backend whose language has a native operation cap (Rhai)
/// gets `eval_block`'s budget for free.
pub trait BlockEvaluator {
    /// The fence language tag this evaluator handles (e.g. `"rhai"`, `"lua"`).
    fn language(&self) -> &str;

    /// Evaluate `source`, bounded by `max_ops` coarse operations (the unit is
    /// backend-defined, the same "stop a runaway" contract as the script
    /// seam's `Budget`; `0` lets the backend pick its own cap). Returns the
    /// output to render, or an error (compile, runtime, or budget) the
    /// evaluate pass records and surfaces — never a hang.
    fn eval_block(&mut self, source: &str, max_ops: u64) -> Result<EvalOutput, String>;
}

/// The host's eval menu: the [`BlockEvaluator`]s it ships, keyed by language.
/// Routing a fence's tag to a backend is the polyglot seam — register the
/// languages you trust on this build, and an unknown tag simply isn't found
/// (the evaluate pass denies it).
#[derive(Default)]
pub struct BlockEvaluators {
    by_language: std::collections::BTreeMap<String, Box<dyn BlockEvaluator>>,
}

impl BlockEvaluators {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an evaluator, keyed by its [`language`](BlockEvaluator::language).
    pub fn register(&mut self, evaluator: Box<dyn BlockEvaluator>) -> &mut Self {
        self.by_language
            .insert(evaluator.language().to_string(), evaluator);
        self
    }

    /// The registered language tags, sorted.
    pub fn languages(&self) -> Vec<&str> {
        self.by_language.keys().map(String::as_str).collect()
    }

    /// Evaluate a fence by language tag, bounded by `max_ops`. `Err` when the
    /// language has no registered evaluator (the host didn't ship it) or the
    /// evaluation itself failed.
    pub fn evaluate(
        &mut self,
        language: &str,
        source: &str,
        max_ops: u64,
    ) -> Result<EvalOutput, String> {
        match self.by_language.get_mut(language) {
            Some(evaluator) => evaluator.eval_block(source, max_ops),
            None => Err(format!("no evaluator registered for `{language}`")),
        }
    }
}

/// What the evaluate pass may do, as plain data. The host builds one per
/// document from its standing and the user's settings; [`deny_all`] is the
/// floor and the right default for any received document.
///
/// [`deny_all`]: EvaluationPolicy::deny_all
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationPolicy {
    /// Master switch — `false` evaluates nothing (fences stay source).
    pub enabled: bool,
    /// Languages that may run (e.g. `lua`). Anything else is denied.
    pub allowed_languages: Vec<String>,
}

impl EvaluationPolicy {
    /// The safe floor: evaluate nothing. The right default for a received
    /// document until the user consents.
    pub fn deny_all() -> Self {
        Self {
            enabled: false,
            allowed_languages: Vec::new(),
        }
    }

    /// A policy for the user's **own** notes, per setting.
    pub fn for_own_notes(allowed_languages: Vec<String>) -> Self {
        Self {
            enabled: true,
            allowed_languages,
        }
    }
}

/// What happened: counts, reasons, and the provenance of every spliced block.
#[derive(Debug, Default)]
pub struct EvalOutcome {
    pub evaluated: usize,
    /// `(language, reason)` for fences the policy refused.
    pub denied: Vec<(String, String)>,
    /// `(language, error)` for fences that ran in policy but errored
    /// (compile error, runtime error, budget exhaustion). Their source fence
    /// stays in place.
    pub failed: Vec<(String, String)>,
    /// Generated-block provenance (top-level index → "evaluated:<lang>").
    pub provenance: BlockProvenanceMap,
}

/// Parse an eval fence's info string: `<lang> eval` (the ` eval ` opt-in).
/// Returns the language; `None` for a plain `<lang>` sample or anything else.
pub fn parse_eval(language: &str) -> Option<&str> {
    let mut tokens = language.split_whitespace();
    let lang = tokens.next()?;
    match tokens.next() {
        Some("eval") if tokens.next().is_none() => Some(lang),
        _ => None,
    }
}

/// Map an output format to the content type the renderer routes on. `plain`
/// is handled before this (rendered directly), so it never reaches here.
fn content_type_for(format: &str) -> &str {
    match format {
        "gemtext" | "gemini" => "text/gemini",
        "markdown" | "md" => "text/markdown",
        "djot" | "knot" => "text/x-knot",
        other => other,
    }
}

/// Turn a plain-text eval result into a block: a multi-line result keeps its
/// shape as `Preformatted`; a single line is a `Paragraph`.
fn plain_block(text: &str) -> Block {
    if text.contains('\n') {
        Block::Preformatted {
            text: text.to_string(),
        }
    } else {
        Block::Paragraph {
            spans: vec![super::InlineSpan::Text(text.to_string())],
        }
    }
}

/// Run the document's top-level `<lang> eval` fences in place. See the module
/// docs for the contract; the outcome reports everything that happened.
pub fn evaluate_blocks(
    document: &mut EngineDocument,
    evaluate: &mut dyn FnMut(&str, &str) -> Result<EvalOutput, String>,
    render: &mut dyn FnMut(&EngineInput) -> Result<EngineDocument, String>,
    policy: &EvaluationPolicy,
) -> EvalOutcome {
    let mut outcome = EvalOutcome::default();
    let mut blocks: Vec<Block> = Vec::with_capacity(document.blocks.len());

    for block in std::mem::take(&mut document.blocks) {
        let fence = match &block {
            Block::CodeBlock {
                language: Some(language),
                text,
            } => parse_eval(language).map(|lang| (lang.to_string(), text.clone())),
            _ => None,
        };

        let Some((lang, source)) = fence else {
            blocks.push(block);
            continue;
        };

        if !policy.enabled {
            outcome.denied.push((lang, "evaluation disabled".into()));
            blocks.push(block);
            continue;
        }
        if !policy.allowed_languages.iter().any(|l| l == &lang) {
            outcome
                .denied
                .push((lang, "language not in the allowlist".into()));
            blocks.push(block);
            continue;
        }

        let output = match evaluate(&lang, &source) {
            Ok(output) => output,
            Err(error) => {
                outcome.failed.push((lang, error));
                blocks.push(block);
                continue;
            }
        };

        // Provenance marks the splice as generated, not fetched.
        let source_mark = BlockProvenance::from_document(DocumentProvenance {
            source_label: Some(format!("evaluated:{lang}")),
            ..DocumentProvenance::default()
        });

        if output.format.is_empty() || output.format == "plain" || output.format == "text" {
            outcome.provenance.insert(blocks.len(), source_mark);
            blocks.push(plain_block(&output.text));
            outcome.evaluated += 1;
            continue;
        }

        let mut input = EngineInput::new(format!("eval:{lang}"), output.text);
        input.content_type = Some(content_type_for(&output.format).to_string());
        match render(&input) {
            Ok(rendered) => {
                for child in rendered.blocks {
                    outcome.provenance.insert(blocks.len(), source_mark.clone());
                    blocks.push(child);
                }
                outcome.evaluated += 1;
            }
            Err(error) => {
                outcome
                    .failed
                    .push((lang, format!("render output: {error}")));
                blocks.push(block);
            }
        }
    }

    document.blocks = blocks;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{DocumentTrustState, InlineSpan};

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

    fn eval_fence(lang_tag: &str, source: &str) -> Block {
        Block::CodeBlock {
            language: Some(lang_tag.to_string()),
            text: source.to_string(),
        }
    }

    /// A render stub: one paragraph carrying the body, tagged by content type.
    fn stub_render(input: &EngineInput) -> Result<EngineDocument, String> {
        Ok(doc_with(vec![Block::Paragraph {
            spans: vec![InlineSpan::Text(format!(
                "[{}] {}",
                input.content_type.as_deref().unwrap_or("?"),
                input.body
            ))],
        }]))
    }

    fn allow_lua() -> EvaluationPolicy {
        EvaluationPolicy::for_own_notes(vec!["lua".into()])
    }

    #[test]
    fn parse_eval_requires_the_opt_in() {
        assert_eq!(parse_eval("lua eval"), Some("lua"));
        assert_eq!(parse_eval("lua"), None, "a plain fence is a sample");
        assert_eq!(parse_eval("rust"), None);
        assert_eq!(parse_eval("lua eval extra"), None);
        assert_eq!(parse_eval(""), None);
    }

    #[test]
    fn a_plain_result_renders_as_a_block() {
        let mut document = doc_with(vec![eval_fence("lua eval", "return 1 + 1")]);
        let mut evaluate = |_lang: &str, _src: &str| Ok(EvalOutput::plain("2"));
        let outcome = evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());

        assert_eq!(outcome.evaluated, 1);
        assert!(matches!(
            &document.blocks[0],
            Block::Paragraph { spans } if spans == &vec![InlineSpan::Text("2".into())]
        ));
        assert!(
            outcome.provenance.get(0).is_some(),
            "spliced block is marked generated"
        );
    }

    #[test]
    fn a_multiline_plain_result_is_preformatted() {
        let mut document = doc_with(vec![eval_fence("lua eval", "x")]);
        let mut evaluate = |_: &str, _: &str| Ok(EvalOutput::plain("line1\nline2"));
        evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());
        assert!(matches!(&document.blocks[0], Block::Preformatted { .. }));
    }

    #[test]
    fn a_gemtext_result_is_nested_rendered() {
        let mut document = doc_with(vec![eval_fence("lua eval", "x")]);
        let mut evaluate = |_: &str, _: &str| {
            Ok(EvalOutput {
                format: "gemtext".into(),
                text: "## generated".into(),
            })
        };
        evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());
        // The stub render tagged it with the routed content type.
        assert!(matches!(
            &document.blocks[0],
            Block::Paragraph { spans }
                if spans == &vec![InlineSpan::Text("[text/gemini] ## generated".into())]
        ));
    }

    #[test]
    fn disabled_and_disallowed_keep_the_source_fence() {
        // Disabled entirely (received document).
        let mut document = doc_with(vec![eval_fence("lua eval", "return 1")]);
        let mut evaluate =
            |_: &str, _: &str| -> Result<EvalOutput, String> { panic!("must not run") };
        let outcome = evaluate_blocks(
            &mut document,
            &mut evaluate,
            &mut stub_render,
            &EvaluationPolicy::deny_all(),
        );
        assert_eq!(outcome.evaluated, 0);
        assert_eq!(outcome.denied.len(), 1);
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));

        // Enabled, but the language is not allowed.
        let mut document = doc_with(vec![eval_fence("python eval", "print(1)")]);
        let outcome = evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());
        assert_eq!(outcome.denied.len(), 1);
        assert!(outcome.denied[0].1.contains("allowlist"));
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));
    }

    #[test]
    fn a_failing_script_keeps_the_fence_and_reports() {
        let mut document = doc_with(vec![eval_fence("lua eval", "error('boom')")]);
        let mut evaluate =
            |_: &str, _: &str| -> Result<EvalOutput, String> { Err("runtime error: boom".into()) };
        let outcome = evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());
        assert_eq!(outcome.failed.len(), 1);
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));
    }

    #[test]
    fn a_plain_lua_sample_fence_is_left_untouched() {
        let mut document = doc_with(vec![eval_fence("lua", "this is a code sample")]);
        let mut evaluate =
            |_: &str, _: &str| -> Result<EvalOutput, String> { panic!("samples don't run") };
        let outcome = evaluate_blocks(&mut document, &mut evaluate, &mut stub_render, &allow_lua());
        assert_eq!(outcome.evaluated, 0);
        assert!(outcome.denied.is_empty());
        assert!(matches!(&document.blocks[0], Block::CodeBlock { .. }));
    }
}
