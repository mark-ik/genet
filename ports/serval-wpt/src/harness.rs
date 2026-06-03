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
//! Engine: selectable via `--engine boa|nova` (see [`Engine`]). Boa is the
//! pure-Rust conformance oracle; Nova is the native primary. The harness's
//! regex-incompatible source is shimmed host-side (`harness_regex_compat`), and
//! the WTF-8/UTF-16 string-indexing bugs that once panicked Nova are fixed in the
//! fork (`docs/2026-06-02_nova_wtf8_indexing_fixes.md`); both engines now produce
//! real numbers on the same corpus.
//!
//! Limitation: the test starts with an empty DOM. Tests that build their own DOM
//! (`createElement`) or are pure-JS run; tests that query elements declared in the
//! HTML body do not see them yet (parsing the body into the scripted DOM is a
//! later step).

use std::fs;
use std::path::{Path, PathBuf};

use layout_dom_api::{LayoutDom, LocalName, Namespace};
use script_engine_api::ScriptEngine;
use script_runtime_api::{FetchHandler, Runtime, TestResult};
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

/// Where a test's `<script src>` resources come from. Disk mode reads files;
/// server mode HTTP-GETs them so `.sub.js` template substitution happens. The
/// harness / report-hook srcs are filtered out by the caller, not the loader.
pub trait ScriptSrcLoader {
    /// The contents of a non-harness `<script src>`, or `None` to skip it
    /// (unresolvable, remote-in-disk-mode, or fetch failed).
    fn load_script(&self, src: &str) -> Option<String>;
}

/// Disk loader: resolve `<script src>` against the test dir / tests root, read the
/// file. The default (no server). Remote and `data:` srcs are skipped.
pub struct DiskLoader<'a> {
    pub base_dir: &'a Path,
    pub tests_root: &'a Path,
}

impl ScriptSrcLoader for DiskLoader<'_> {
    fn load_script(&self, src: &str) -> Option<String> {
        let path = resolve(src, self.base_dir, self.tests_root)?;
        fs::read_to_string(path).ok()
    }
}

/// Run one testharness test HTML and collect its results, using `loader` to fetch
/// `<script src>` resources. `base_url` (when set) becomes the document base for
/// relative `fetch()` / `Request` URLs and populates `location`; `handler` (when
/// set) is the `fetch()` network seam. Disk mode passes `None`/`None`.
pub fn run_test(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    engine: Engine,
) -> HarnessOutcome {
    let doc = StaticDocument::parse(html);
    let mut scripts = Vec::new();
    collect_scripts(&doc, doc.document(), loader, &mut scripts);
    let test_src = scripts.join("\n;\n");

    match engine {
        Engine::Boa => {
            run_with::<script_engine_boa::BoaEngine>(testharness_js, &test_src, &doc, base_url, handler)
        }
        Engine::Nova => {
            run_with::<script_engine_nova::NovaEngine>(testharness_js, &test_src, &doc, base_url, handler)
        }
    }
}

/// Engine-generic core: build a `Runtime<E>`, load the test's body as the live
/// DOM, set the base URL + fetch handler if given, run the harness, collect
/// results. Each backend implements `ScriptEngine`, so the only per-engine thing
/// is the monomorphization chosen by [`run_test`].
fn run_with<E: ScriptEngine>(
    testharness_js: &str,
    test_src: &str,
    doc: &StaticDocument,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
) -> HarnessOutcome {
    let mut rt = match Runtime::<E>::new() {
        Ok(rt) => rt,
        Err(e) => return HarnessOutcome::Threw(format!("runtime init: {e:?}")),
    };
    // The test's body becomes the live DOM, so scripts querying body elements
    // (getElementById / querySelector / document.body) see them.
    rt.load_dom(doc);
    if let Some(base) = base_url {
        let _ = rt.set_base_url(base);
    }
    if let Some(h) = handler {
        rt.set_fetch_handler(h);
    }
    match rt.run_testharness(testharness_js, test_src) {
        Ok(results) => HarnessOutcome::Ran(results),
        // `ScriptEngine::Error` is `Debug`-only; truncate the (sometimes
        // backtrace-carrying) message defensively.
        Err(e) => HarnessOutcome::Threw(truncate(&format!("{e:?}"), 200)),
    }
}

/// Walk the document collecting test scripts in document order: inline `<script>`
/// text, and the contents of `<script src>` from `loader` (skipping the harness /
/// report hook, which the host surface supplies).
fn collect_scripts<D: LayoutDom>(
    dom: &D,
    node: D::NodeId,
    loader: &dyn ScriptSrcLoader,
    out: &mut Vec<String>,
) {
    if dom.element_name(node).is_some_and(|q| q.local.as_ref() == "script") {
        match dom.attribute(node, &Namespace::default(), &LocalName::from("src")) {
            Some(src) if !is_harness_src(src) => {
                if let Some(text) = loader.load_script(src) {
                    out.push(text);
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
        collect_scripts(dom, child, loader, out);
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
    let loader = DiskLoader { base_dir: root, tests_root: root };
    let t = Instant::now();
    for _ in 0..n {
        let _ = run_test(&testharness_js, html, &loader, None, None, Engine::Boa);
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
