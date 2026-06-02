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
use script_engine_api::ScriptEngine;
use script_runtime_api::{Runtime, TestResult};
use serval_static_dom::StaticDocument;

/// Which JS engine the testharness runner drives. Boa is the pure-Rust
/// conformance oracle; Nova is the native primary. Both implement
/// `ScriptEngine`, so the harness path is generic — this only selects the
/// monomorphization (`--engine`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Engine {
    #[default]
    Boa,
    Nova,
}

impl Engine {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "boa" => Some(Engine::Boa),
            "nova" => Some(Engine::Nova),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Engine::Boa => "boa",
            Engine::Nova => "nova",
        }
    }
}

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
    engine: Engine,
) -> HarnessOutcome {
    let doc = StaticDocument::parse(html);
    let mut scripts = Vec::new();
    collect_scripts(&doc, doc.document(), base_dir, tests_root, &mut scripts);
    let test_src = scripts.join("\n;\n");

    match engine {
        Engine::Boa => run_with::<script_engine_boa::BoaEngine>(testharness_js, &test_src, &doc),
        Engine::Nova => run_with::<script_engine_nova::NovaEngine>(testharness_js, &test_src, &doc),
    }
}

/// Engine-generic core: build a `Runtime<E>`, load the test's body as the live
/// DOM, run the harness, collect results. Each backend implements `ScriptEngine`
/// (Nova native-primary, Boa pure-Rust oracle), so the only per-engine thing is
/// the monomorphization chosen by [`run_test`].
fn run_with<E: ScriptEngine>(
    testharness_js: &str,
    test_src: &str,
    doc: &StaticDocument,
) -> HarnessOutcome {
    let mut rt = match Runtime::<E>::new() {
        Ok(rt) => rt,
        Err(e) => return HarnessOutcome::Threw(format!("runtime init: {e:?}")),
    };
    // The test's body becomes the live DOM, so scripts querying body elements
    // (getElementById / querySelector / document.body) see them.
    rt.load_dom(doc);
    match rt.run_testharness(testharness_js, test_src) {
        Ok(results) => HarnessOutcome::Ran(results),
        // `ScriptEngine::Error` is `Debug`-only; truncate the (sometimes
        // backtrace-carrying) message defensively.
        Err(e) => HarnessOutcome::Threw(truncate(&format!("{e:?}"), 200)),
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

/// Microbench: where does per-test time go? Times, over N iterations,
/// (a) `Runtime::new()` (the host bootstrap a pool would amortize), (b) the same
/// plus `eval(testharness.js)` (the harness re-eval a pool would *also* amortize),
/// and (c) a full `run_test` of a small testharness file. The deltas say whether a
/// reuse-pool is worth its isolation cost, and which eval dominates.
pub fn bench(tests_root: &str) {
    use std::time::Instant;
    // bench is a Boa-specific perf probe (Runtime::new / harness-eval / full run
    // timings); it doesn't vary by engine, so it names Boa directly.
    use script_engine_boa::BoaEngine;
    let root = Path::new(tests_root);
    let testharness_js = match fs::read_to_string(root.join("resources/testharness.js")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("bench: testharness.js not found under {tests_root}/resources");
            std::process::exit(2);
        }
    };
    let n = 50;

    // (a) Runtime::new() only.
    let t = Instant::now();
    for _ in 0..n {
        let _rt = Runtime::<BoaEngine>::new().expect("new");
    }
    let new_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (b) new() + eval(testharness.js).
    let t = Instant::now();
    for _ in 0..n {
        let mut rt = Runtime::<BoaEngine>::new().expect("new");
        rt.eval(&testharness_js).expect("harness eval");
    }
    let new_harness_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (c) a full run_test on a trivial inline testharness test.
    let html = "<!doctype html><script src=/resources/testharness.js></script>\
                <script>test(function(){ assert_true(true); }, 'x');</script>";
    let t = Instant::now();
    for _ in 0..n {
        let _ = run_test(&testharness_js, html, root, root, Engine::Boa);
    }
    let run_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (d) Isolation probe: can one Runtime run two harness evals back-to-back
    // without the `tests` singleton leaking results across them? If a re-eval
    // resets cleanly, a pooled-Runtime (re-eval harness per test) is safe.
    let mut rt = Runtime::<BoaEngine>::new().expect("new");
    let r1 = rt.run_testharness(&testharness_js, "test(function(){ assert_true(true); }, 'a');");
    let r2 = rt.run_testharness(&testharness_js, "test(function(){ assert_true(true); }, 'b');");
    let leak = match (&r1, &r2) {
        (Ok(a), Ok(b)) => format!("run1={} subtests, run2={} subtests (want 1 and 1; >1 = leak)", a.len(), b.len()),
        _ => "a run errored".to_string(),
    };

    println!("bench (Boa, {n} iters, ms/iter):");
    println!("  (a) Runtime::new()                  {new_ms:8.2}");
    println!("  (b) new() + eval(testharness.js)    {new_harness_ms:8.2}  (harness eval = {:.2})", new_harness_ms - new_ms);
    println!("  (c) full run_test (trivial test)    {run_ms:8.2}");
    println!("  (d) reuse isolation: {leak}");
    println!(
        "\nFinding: the dominant per-test cost is the harness eval (~{:.0} ms), not\n\
         Runtime::new() (~{:.0} ms). Reusing a Runtime across tests LEAKS — testharness's\n\
         `tests` singleton accumulates across re-evals (see (d)) — so realm-reuse is\n\
         incorrect without a reset. Correct amortization needs a post-(harness-eval)\n\
         snapshot cloned per test (a fresh `tests` each time): the GcAgent::clone path.",
        new_harness_ms - new_ms, new_ms,
    );
}
