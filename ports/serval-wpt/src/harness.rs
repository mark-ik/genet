/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Phase 3: run a `testharness.js` test and collect its per-subtest results.
//!
//! Extracts the test's own scripts (inline `<script>` + local `<script src>`,
//! skipping `testharness.js` / the report hook, which the host surface supplies),
//! then runs them against `testharness.js` on a fresh [`Runtime`] and reads the
//! results through the bridge ([`Runtime::run_testharness`]).
//!
//! Engine: **Boa** (pure Rust, the conformance oracle). Nova loads the harness but
//! its regex engine rejects the surrogate ranges in the harness's completion
//! sanitizer (see `docs/2026-05-26_pluggable_engines_testharness_plan.md`).
//!
//! Limitation: the test starts with an empty DOM. Tests that build their own DOM
//! (`createElement`) or are pure-JS run; tests that query elements declared in the
//! HTML body do not see them yet (parsing the body into the scripted DOM is a
//! later step).

use std::fs;
use std::path::{Path, PathBuf};

use layout_dom_api::{LayoutDom, LocalName, Namespace};
use script_engine_boa::BoaEngine;
use script_runtime_api::{Runtime, TestResult};
use serval_static_dom::StaticDocument;

/// Outcome of running one testharness test.
pub enum HarnessOutcome {
    /// The harness ran to completion; the per-subtest results (may be empty if the
    /// test reported none, e.g. an async test that never completed).
    Ran(Vec<TestResult>),
    /// The harness or the test threw before reporting — usually an unimplemented
    /// DOM/JS feature. Carries a concise message.
    Threw(String),
}

/// Run one testharness test HTML and collect its results. `base_dir` is the test's
/// own directory (for resolving local `<script src>`), `tests_root` the corpus root
/// (for `/`-absolute srcs).
pub fn run_test(
    testharness_js: &str,
    html: &str,
    base_dir: &Path,
    tests_root: &Path,
) -> HarnessOutcome {
    let doc = StaticDocument::parse(html);
    let mut scripts = Vec::new();
    collect_scripts(&doc, doc.document(), base_dir, tests_root, &mut scripts);
    let test_src = scripts.join("\n;\n");

    let mut rt = match Runtime::<BoaEngine>::new() {
        Ok(rt) => rt,
        Err(e) => return HarnessOutcome::Threw(format!("runtime init: {e}")),
    };
    // The test's body becomes the live DOM, so scripts querying body elements
    // (getElementById / querySelector / document.body) see them.
    rt.load_dom(&doc);
    match rt.run_testharness(testharness_js, &test_src) {
        Ok(results) => HarnessOutcome::Ran(results),
        // Boa's `JsError` Display is concise (the Debug form carries a full
        // backtrace); truncate defensively all the same.
        Err(e) => HarnessOutcome::Threw(truncate(&format!("{e}"), 200)),
    }
}

/// Walk the document collecting test scripts in document order: inline `<script>`
/// text, and the contents of local `<script src>` files (skipping the harness and
/// remote sources).
fn collect_scripts<D: LayoutDom>(
    dom: &D,
    node: D::NodeId,
    base_dir: &Path,
    tests_root: &Path,
    out: &mut Vec<String>,
) {
    if dom.element_name(node).is_some_and(|q| q.local.as_ref() == "script") {
        match dom.attribute(node, &Namespace::default(), &LocalName::from("src")) {
            Some(src) if !is_harness_src(src) => {
                if let Some(path) = resolve(src, base_dir, tests_root) {
                    if let Ok(text) = fs::read_to_string(path) {
                        out.push(text);
                    }
                }
            }
            Some(_) => {} // the harness / report hook: the host surface supplies these
            None => {
                let mut text = String::new();
                for child in dom.dom_children(node) {
                    if let Some(t) = dom.text(child) {
                        text.push_str(t);
                    }
                }
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
        }
    }
    for child in dom.dom_children(node) {
        collect_scripts(dom, child, base_dir, tests_root, out);
    }
}

/// `testharness.js` and its report hook are supplied by the host surface (the
/// results bridge replaces the report), so the test's own copies are skipped.
fn is_harness_src(src: &str) -> bool {
    let s = src.split(['#', '?']).next().unwrap_or(src);
    s.ends_with("testharness.js")
        || s.ends_with("testharnessreport.js")
        || s.ends_with("testharnesscss.css")
}

/// Resolve a local `<script src>` to a path (`/`-absolute against the tests root,
/// else relative to the test dir). Remote / `data:` srcs return `None`.
fn resolve(src: &str, base_dir: &Path, tests_root: &Path) -> Option<PathBuf> {
    let src = src.split(['#', '?']).next().unwrap_or(src).trim();
    if src.is_empty()
        || src.starts_with("http://")
        || src.starts_with("https://")
        || src.starts_with("//")
        || src.starts_with("data:")
    {
        return None;
    }
    Some(match src.strip_prefix('/') {
        Some(rest) => tests_root.join(rest),
        None => base_dir.join(src),
    })
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
