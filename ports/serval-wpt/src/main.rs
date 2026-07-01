/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! serval-native web-platform-tests runner.
//!
//! Runs a **selectable subset** of `tests/wpt` against serval, so a single
//! subsystem can be checked without the whole 1.2 GB suite.
//!
//! Phase 1 (this binary) is a **crash-smoke**: each runnable test is loaded
//! through `serval_static_dom::parse` + `serval_layout::render` (with inline
//! `<style>` extracted), wrapped in `catch_unwind`. A test "passes" if
//! loading does not panic. That finds layout panics across real pages, the
//! highest-leverage early signal, and needs no GPU and no JS. Reftest pixel
//! comparison and testharness.js are later phases.
//!
//! Cf. `docs/2026-05-26_wpt_runner_plan.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use layout_dom_api::{LayoutDom, LocalName};
use script_engine_api::ScriptEngine;
use serval_static_dom::StaticDocument;

mod harness;
mod manifest;
mod render;
mod test262;
#[cfg(test)]
mod webgl_conformance;

// The upstream WPT checkout lives under `tests/wpt/tests/`
// (`tests/wpt/mozilla/` holds servo-specific tests).
const DEFAULT_TESTS_ROOT: &str = "tests/wpt/tests";
const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: f32 = 600.0;
// Reftest render size (the WPT default viewport).
const REFTEST_W: u32 = 800;
const REFTEST_H: u32 = 600;

// GPU anti-aliasing jitter floor. Vello rasterization is not bit-exact
// run-to-run: two renders of identical input differ by up to ~1/255 on a
// sub-1% sliver of (anti-aliased edge) pixels. Exact-match scoring (0,0)
// therefore flips borderline tests between runs, making the pass count
// non-deterministic. This floor — at most `FUZZ_FLOOR_DIFF` per-channel
// delta on at most `FUZZ_FLOOR_PIXELS` pixels — absorbs exactly that
// jitter and nothing near a real paint bug (those differ by 255 over a
// localized region). Applied as a *lower bound* on every comparison: a
// test's own `<meta name=fuzzy>` still wins where it is looser.
const FUZZ_FLOOR_DIFF: u16 = 1;
// 0.5% of the 800x600 render = 2400 px.
const FUZZ_FLOOR_PIXELS: u64 = (REFTEST_W as u64 * REFTEST_H as u64) / 200;

/// WPT test classification (convention-based; see the plan doc).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reference,
    Manual,
    Reftest,
    Crashtest,
    Testharness,
    Load,
}

impl Kind {
    fn from_manifest(kind: manifest::TestKind) -> Kind {
        match kind {
            manifest::TestKind::Testharness => Kind::Testharness,
            manifest::TestKind::Reftest | manifest::TestKind::PrintReftest => Kind::Reftest,
            manifest::TestKind::Crashtest => Kind::Crashtest,
            manifest::TestKind::Manual
            | manifest::TestKind::Visual
            | manifest::TestKind::Wdspec => Kind::Manual,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Reference => "reference",
            Kind::Manual => "manual",
            Kind::Reftest => "reftest",
            Kind::Crashtest => "crashtest",
            Kind::Testharness => "testharness",
            Kind::Load => "load",
        }
    }

    /// Phase 1 runs the crash-smoke on everything except references and
    /// manual tests. (Reftest/testharness still only get the load-smoke
    /// here; their real verification is phases 2/3.)
    fn runs_in_phase1(self) -> bool {
        !matches!(self, Kind::Reference | Kind::Manual)
    }
}

fn is_html(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("html" | "htm" | "xht" | "xhtml")
    )
}

/// True for XHTML/XML documents (parse with xml5ever, not html5ever), keyed on the
/// file extension — the reliable signal. Content sniffing misroutes HTML files
/// that merely mention "xhtml" in a doctype or comment.
fn is_xml_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("xht" | "xhtml" | "xml")
    )
}

/// Classify a test by filename + path conventions and a cheap content scan.
fn classify(path: &Path, contents: &str) -> Kind {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let path_str = path.to_string_lossy().replace('\\', "/");

    // References are not tests themselves.
    if stem.ends_with("-ref")
        || stem.ends_with(".ref")
        || stem.starts_with("ref-")
        || path_str.contains("/reference/")
    {
        return Kind::Reference;
    }
    if stem.ends_with("-manual") {
        return Kind::Manual;
    }
    if path_str.contains("/crashtests/") || stem.ends_with("-crash") {
        return Kind::Crashtest;
    }
    if contents.contains("rel=\"match\"")
        || contents.contains("rel=match")
        || contents.contains("rel=\"mismatch\"")
        || contents.contains("rel=mismatch")
    {
        return Kind::Reftest;
    }
    if contents.contains("testharness.js") {
        return Kind::Testharness;
    }
    Kind::Load
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    Skipped,
    ReadError,
}

#[derive(Clone)]
struct TestCase {
    /// Backing file inside `tests_root`. Generated WPT variants such as
    /// `/foo.any.html` point back to `foo.any.js`.
    path: PathBuf,
    /// Runnable WPT URL, tests-root-relative and without the leading slash. This
    /// is the stable identity used for listings, expectations, and server mode.
    url: String,
    kind: Kind,
    refs: Vec<(String, manifest::RefMatch)>,
    fuzzy: Option<(u16, u64)>,
    long_timeout: bool,
    from_manifest: bool,
}

impl TestCase {
    fn from_walk(path: PathBuf, tests_root: &str) -> TestCase {
        let contents = fs::read(&path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let kind = classify(&path, &contents);
        TestCase {
            url: rel(&path, tests_root),
            path,
            kind,
            refs: Vec::new(),
            fuzzy: None,
            long_timeout: false,
            from_manifest: false,
        }
    }

    fn from_manifest(test: manifest::ManifestTest, tests_root: &Path) -> Option<TestCase> {
        if test.is_worker() {
            return None;
        }
        let source_path = strip_url_path(&test.source_path);
        if source_path.is_empty() {
            return None;
        }
        Some(TestCase {
            path: tests_root.join(source_path),
            url: normalize_test_url(&test.url),
            kind: Kind::from_manifest(test.kind),
            refs: test.refs,
            fuzzy: manifest_fuzzy_upper(test.fuzzy),
            long_timeout: test.long_timeout,
            from_manifest: true,
        })
    }

    fn name(&self) -> &str {
        &self.url
    }

    fn disk_doc_url(&self) -> String {
        format!("http://web-platform.test/{}", self.url)
    }
}

fn normalize_test_url(url: &str) -> String {
    url.trim_start_matches('/').replace('\\', "/")
}

fn strip_url_path(url_or_path: &str) -> &str {
    url_or_path
        .trim_start_matches('/')
        .split(['#', '?'])
        .next()
        .unwrap_or(url_or_path)
}

fn manifest_fuzzy_upper(fuzzy: Option<((u32, u32), (u32, u32))>) -> Option<(u16, u64)> {
    fuzzy.map(|((_, diff_hi), (_, total_hi))| {
        (diff_hi.min(u32::from(u16::MAX)) as u16, u64::from(total_hi))
    })
}

/// Crash-smoke one test: parse + cascade + layout, catching panics.
fn smoke_test(test: &TestCase) -> (Kind, Outcome) {
    let kind = test.kind;
    if !kind.runs_in_phase1() {
        return (kind, Outcome::Skipped);
    }
    let html = match load_test_document_disk(test) {
        TestHtml::Html(html) => html,
        TestHtml::Skip(_) => return (kind, Outcome::Skipped),
        TestHtml::ReadError => return (kind, Outcome::ReadError),
    };

    let is_xml = is_xml_path(&test.path);
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let document = if is_xml {
            StaticDocument::parse_xml(&html)
        } else {
            StaticDocument::parse(&html)
        };
        let sheets = serval_layout::inline_stylesheets_from_source(&html);
        let sheet_refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
        let _fragments = serval_layout::render(&document, &sheet_refs, VIEWPORT_W, VIEWPORT_H);
    }));

    (
        kind,
        if result.is_ok() {
            Outcome::Passed
        } else {
            Outcome::Failed
        },
    )
}

/// True for a WPT `.any.js` / `.window.js` / `.worker.js` test (a JS file the
/// harness wraps into a generated HTML page rather than a standalone document).
fn is_any_js(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with(".any.js") || name.ends_with(".window.js") || name.ends_with(".worker.js")
}

/// Collect HTML + `.any.js`-style test files under `root` (a dir or a single file).
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        if is_html(root) || is_any_js(root) {
            out.push(root.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            // WPT excludes `tools/` and `support/` directories from test
            // collection: they hold test-generation templates and helper
            // resources (images, fragments referenced by path), not tests. A
            // `tools/*-template.html` carries a `rel=match` to a ref that does
            // not exist, so collecting it produces a spurious `ref-missing`
            // error. Hidden dirs (`.git`, …) are skipped too.
            if matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("tools" | "support")
            ) || path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            collect(&path, out);
        } else if is_html(&path) || is_any_js(&path) {
            out.push(path);
        }
    }
}

/// Synthesize the testharness HTML wrapper for a `.any.js` / `.window.js` test:
/// load testharness.js + the test's `// META: script=...` helpers + the test
/// file itself, exactly as WPT's build step generates the `.any.html` variant.
/// `run_test` then resolves those `<script src>` the usual way. Returns `None`
/// for worker-only tests (`.worker.js`, or `.any.js` whose `global=` excludes
/// window), which this window-shaped runner can't host.
fn synthesize_any_js(path: &Path, variant_url: Option<&str>) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".worker.js") {
        return None;
    }
    let src = fs::read_to_string(path).ok()?;
    let mut scripts: Vec<String> = Vec::new();
    let mut window_ok = true;
    let mut in_block = false;
    // The `// META:` directives form the leading comment header; scan until the
    // first real statement (tracking /* */ so a license block doesn't end it).
    for line in src.lines() {
        let t = line.trim();
        if in_block {
            if t.contains("*/") {
                in_block = false;
            }
            continue;
        }
        if t.starts_with("/*") {
            if !t.contains("*/") {
                in_block = true;
            }
            continue;
        }
        if let Some(meta) = t.strip_prefix("// META:") {
            let meta = meta.trim();
            if let Some(s) = meta.strip_prefix("script=") {
                scripts.push(s.trim().to_owned());
            } else if let Some(g) = meta.strip_prefix("global=") {
                // window-shaped run: only `.any.js` whose globals include window
                // (or the dedicated-window aliases) is hostable here.
                window_ok = g.split(',').any(|tok| {
                    let tok = tok.trim();
                    tok == "window" || tok == "default" || tok.starts_with("window")
                });
            }
            continue;
        }
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        break; // first real statement: META header is over
    }
    // `.window.js` is inherently window-scoped; the global directive only gates
    // `.any.js`.
    if name.ends_with(".any.js") && !window_ok {
        return None;
    }
    // `self.GLOBAL` is injected by WPT's `.any.html` wrapper (tools/serve/serve.py)
    // before testharness.js; tests branch on it (`GLOBAL.isWorker()`), so synthesize
    // the window-shaped stub here too or those files throw at load.
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8>\n\
         <script>self.GLOBAL={isWindow:function(){return true;},isWorker:function(){return false;},isShadowRealm:function(){return false;}};</script>\n\
         <script src=\"/resources/testharness.js\"></script>\n\
         <script src=\"/resources/testharnessreport.js\"></script>\n",
    );
    for s in scripts {
        html.push_str(&format!("<script src=\"{s}\"></script>\n"));
    }
    let query = variant_url
        .and_then(|u| {
            u.split_once('?')
                .map(|(_, q)| q.split('#').next().unwrap_or(q))
        })
        .filter(|q| !q.is_empty());
    let test_src = match query {
        Some(q) => format!("{name}?{q}"),
        None => name.to_string(),
    };
    html.push_str(&format!("<script src=\"{test_src}\"></script>\n"));
    Some(html)
}

struct Args {
    command: String,
    subset: String,
    tests_root: String,
    verbose: bool,
    engine: harness::Engine,
    /// Connect to an already-running `wpt serve` at this origin (server mode).
    server_base: Option<String>,
    /// Spawn (and tear down) a `wpt serve` for the run (server mode).
    spawn_server: bool,
    /// Per-test wall-clock timeout (seconds) for `test262` worker subprocesses: a test
    /// running longer is killed and recorded as a timeout. Generous enough for slow
    /// (but finite) tests; bounds true infinite hangs.
    timeout_secs: u64,
    /// Write the full `test262` worklist (every Nova gap + every timeout, not just the
    /// printed sample) to this path. Essential for a full-corpus run, whose lists run to
    /// thousands.
    worklist_out: Option<String>,
    /// Use the legacy directory walk instead of MANIFEST.json. This is retained as a
    /// diagnostic fallback for custom partial trees; normal WPT commands are
    /// manifest-backed.
    walk_discovery: bool,
    /// Check current per-test statuses against a JSON expectations file.
    expectations: Option<String>,
    /// Write current per-test statuses to a JSON expectations file.
    write_expectations: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut command = None;
    let mut subset = None;
    let mut tests_root = DEFAULT_TESTS_ROOT.to_string();
    let mut verbose = false;
    let mut engine = harness::Engine::default();
    let mut server_base = None;
    let mut spawn_server = false;
    let mut timeout_secs = 30u64;
    let mut worklist_out = None;
    let mut walk_discovery = false;
    let mut expectations = None;
    let mut write_expectations = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tests-root" => {
                tests_root = it.next().ok_or("--tests-root needs a value")?;
            },
            "--engine" => {
                let v = it.next().ok_or("--engine needs a value (boa | nova)")?;
                engine = harness::Engine::parse(&v)
                    .ok_or_else(|| format!("unknown engine: {v} (expected boa | nova)"))?;
            },
            "--server-base" => {
                server_base = Some(it.next().ok_or("--server-base needs a URL")?);
            },
            "--spawn-server" => spawn_server = true,
            "--timeout" => {
                let v = it.next().ok_or("--timeout needs a value (seconds)")?;
                timeout_secs = v.parse().map_err(|_| format!("invalid --timeout: {v}"))?;
            },
            "--worklist-out" => {
                worklist_out = Some(it.next().ok_or("--worklist-out needs a path")?);
            },
            "--walk-discovery" => walk_discovery = true,
            "--expectations" => {
                expectations = Some(it.next().ok_or("--expectations needs a path")?);
            },
            "--write-expectations" => {
                write_expectations = Some(it.next().ok_or("--write-expectations needs a path")?);
            },
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => return Err(usage()),
            _ if arg.starts_with('-') => return Err(format!("unknown flag: {arg}\n{}", usage())),
            _ if command.is_none() => command = Some(arg),
            _ if subset.is_none() => subset = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}\n{}", usage())),
        }
    }
    Ok(Args {
        command: command.ok_or(usage())?,
        subset: subset.unwrap_or_default(),
        tests_root,
        verbose,
        engine,
        server_base,
        spawn_server,
        timeout_secs,
        worklist_out,
        walk_discovery,
        expectations,
        write_expectations,
    })
}

fn usage() -> String {
    "\
serval-wpt - serval-native web-platform-tests runner (phase 1: crash-smoke)

Usage:
    serval-wpt list        <subset>   enumerate + classify tests in a subset
    serval-wpt run         <subset>   crash-smoke a subset (parse + layout)
    serval-wpt reftest     <subset>   render + pixel-compare reftests (needs a GPU)
    serval-wpt testharness <subset>   run testharness.js tests + collect results (Boa)
    serval-wpt manifest    <subset>   enumerate from MANIFEST.json (authoritative; H1)
    serval-wpt compare     <subset>   run each testharness test on Boa + Nova, diff (H2b)
    serval-wpt test262     <subset>   run test262 on Boa + Nova, diff = Nova's worklist

Options:
    --tests-root <dir>   tests root (default: tests/wpt)
    --timeout <secs>     per-test worker timeout for `test262` (default: 30)
    --worklist-out <f>   write the full `test262` Nova-gap + timeout list to <f>
    --walk-discovery     use the legacy directory walk instead of MANIFEST.json
    --expectations <f>   fail if testharness results differ from JSON expectations
    --write-expectations <f>
                         write current testharness results as JSON expectations
    --engine <name>      testharness JS engine: boa (default) | nova
    --server-base <url>  run testharness against a live `wpt serve` at <url>
                         (server mode; needs --features netfetch)
    --spawn-server       spawn + tear down a `wpt serve` for the run
                         (server mode; needs --features netfetch)
    -v, --verbose        print every test, not just failures
    -h, --help

<subset> is a directory or file beneath the tests root, e.g.
    serval-wpt run css/CSS2/floats
    serval-wpt run dom/nodes/Element-classList.html"
        .to_string()
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        },
    };

    // bench needs only the tests root (for resources/testharness.js), not a subset
    // walk; handle it before the corpus collection below.
    if args.command == "bench" {
        harness::bench(&args.tests_root);
        return;
    }

    // `manifest` enumerates from MANIFEST.json (the authoritative WPT test list),
    // not the directory walk — handled before the corpus collection so it can be
    // diffed against `collect` (harness-exactness H1: spot-check the counts match).
    if args.command == "manifest" {
        manifest_list(&args);
        return;
    }

    // `test262` runs its own vendored corpus (third_party/test262), not the WPT walk.
    if args.command == "test262" {
        test262_cmd(&args);
        return;
    }
    // The per-test worker the parent `test262` run spawns for hang isolation.
    if args.command == "test262-one" {
        test262_one(&args);
        return;
    }

    if (args.expectations.is_some() || args.write_expectations.is_some())
        && args.command != "testharness"
    {
        eprintln!(
            "--expectations / --write-expectations are currently supported for `testharness` only"
        );
        std::process::exit(2);
    }

    let tests = discover_tests(&args);
    if tests.is_empty() {
        eprintln!(
            "no runnable tests found for '{}' under {}",
            if args.subset.is_empty() {
                "<all>"
            } else {
                &args.subset
            },
            args.tests_root
        );
        std::process::exit(1);
    }

    match args.command.as_str() {
        "list" => list(&tests, &args),
        "run" => run(&tests, &args),
        "reftest" => reftest(&tests, &args),
        "dump" => dump(&tests, &args),
        "testharness" => testharness(&tests, &args),
        "compare" => compare(&tests, &args),
        other => {
            eprintln!("unknown command: {other}\n{}", usage());
            std::process::exit(2);
        },
    }
}

fn manifest_path(tests_root: &str) -> PathBuf {
    Path::new(tests_root)
        .parent()
        .map(|p| p.join("meta/MANIFEST.json"))
        .unwrap_or_else(|| PathBuf::from("MANIFEST.json"))
}

fn discover_tests(args: &Args) -> Vec<TestCase> {
    let tests_root = Path::new(&args.tests_root);
    if args.walk_discovery {
        let root = tests_root.join(&args.subset);
        if !root.exists() {
            eprintln!("subset path does not exist: {}", root.display());
            std::process::exit(2);
        }
        let mut paths = Vec::new();
        collect(&root, &mut paths);
        return paths
            .into_iter()
            .map(|path| TestCase::from_walk(path, &args.tests_root))
            .collect();
    }

    let path = manifest_path(&args.tests_root);
    let manifest = match manifest::Manifest::load(&path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "manifest load failed ({}): {e}; pass --walk-discovery to use the legacy directory walk",
                path.display()
            );
            std::process::exit(1);
        },
    };
    manifest
        .tests_under(&args.subset)
        .into_iter()
        .filter_map(|test| TestCase::from_manifest(test, tests_root))
        .collect()
}

/// Enumerate tests under a subset from MANIFEST.json (harness-exactness H1), for
/// diffing the authoritative manifest enumeration against the directory walk. The
/// manifest sits at `<tests-root>/../meta/MANIFEST.json`. Worker variants are counted
/// but excluded from the runnable total (this window-shaped runner cannot host them).
fn manifest_list(args: &Args) {
    let manifest_path = manifest_path(&args.tests_root);
    let manifest = match manifest::Manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest load failed ({}): {e}", manifest_path.display());
            std::process::exit(1);
        },
    };
    let tests = manifest.tests_under(&args.subset);
    let mut total = 0usize;
    let mut workers = 0usize;
    let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for t in &tests {
        if t.is_worker() {
            workers += 1;
            continue;
        }
        total += 1;
        *counts.entry(t.kind.label()).or_default() += 1;
        if args.verbose {
            println!("{:<12} {}", t.kind.label(), t.url);
        }
    }
    let by_kind: Vec<String> = counts.iter().map(|(k, n)| format!("{k}={n}")).collect();
    println!(
        "manifest: {total} runnable test(s) under '{}' ({}); {workers} worker variant(s) skipped",
        if args.subset.is_empty() {
            "<all>"
        } else {
            &args.subset
        },
        by_kind.join(", "),
    );
}

fn rel(path: &Path, tests_root: &str) -> String {
    path.strip_prefix(tests_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn list(tests: &[TestCase], _args: &Args) {
    let mut counts = [0usize; 6];
    let mut manifest_backed = 0usize;
    for test in tests {
        let kind = test.kind;
        if test.from_manifest {
            manifest_backed += 1;
        }
        counts[kind as usize] += 1;
        let timeout = if test.long_timeout { " long" } else { "" };
        println!("{:<12} {}{}", kind.label(), test.name(), timeout);
    }
    println!(
        "\n{} test variant(s): {} reftest, {} testharness, {} crashtest, {} load, {} manual, {} reference{}",
        tests.len(),
        counts[Kind::Reftest as usize],
        counts[Kind::Testharness as usize],
        counts[Kind::Crashtest as usize],
        counts[Kind::Load as usize],
        counts[Kind::Manual as usize],
        counts[Kind::Reference as usize],
        if manifest_backed == tests.len() {
            " (manifest-backed)"
        } else {
            ""
        },
    );
}

fn run(tests: &[TestCase], args: &Args) {
    // Quiet the default panic hook so crash-smoke failures do not spam
    // backtraces; the runner reports them itself.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut passed, mut failed, mut skipped, mut errored) = (0, 0, 0, 0);
    for test in tests {
        let (kind, outcome) = smoke_test(test);
        match outcome {
            Outcome::Passed => {
                passed += 1;
                if args.verbose {
                    println!("PASS  {:<12} {}", kind.label(), test.name());
                }
            },
            Outcome::Failed => {
                failed += 1;
                println!("FAIL  {:<12} {}", kind.label(), test.name());
            },
            Outcome::ReadError => {
                errored += 1;
                println!("ERROR read    {}", test.name());
            },
            Outcome::Skipped => {
                skipped += 1;
                if args.verbose {
                    println!("SKIP  {:<12} {}", kind.label(), test.name());
                }
            },
        }
    }

    panic::set_hook(prev);

    println!(
        "\ncrash-smoke: {} passed, {} failed, {} errored, {} skipped (of {} files)",
        passed,
        failed,
        errored,
        skipped,
        tests.len()
    );
    if failed > 0 || errored > 0 {
        std::process::exit(1);
    }
}

/// Resolve the server-mode context from the args: spawn a `wpt serve`, connect to
/// one, or `None` (disk mode). `--spawn-server` wins over `--server-base`. A
/// requested-but-unreachable server is fatal (the run would silently fall back to
/// network errors otherwise).
#[cfg(feature = "netfetch")]
fn setup_server(args: &Args) -> Option<net::ServerCtx> {
    if !args.spawn_server && args.server_base.is_none() {
        return None;
    }
    // The WPT server's https / h2 origins use a self-signed CA; trust any
    // certificate so https:// and h2 fetches reach it. Must run before the first
    // request (the readiness probe below builds the shared client).
    netfetcher::accept_invalid_certs();
    let result = if args.spawn_server {
        eprintln!("spawning `wpt serve` under {} ...", args.tests_root);
        net::ServerCtx::spawn(Path::new(&args.tests_root))
    } else {
        net::ServerCtx::connect(args.server_base.clone().unwrap())
    };
    match result {
        Ok(s) => {
            eprintln!("server mode: driving fetch against {}", s.origin);
            net::set_page_origin(&s.origin);
            Some(s)
        },
        Err(e) => {
            eprintln!("server mode setup failed: {e}");
            std::process::exit(2);
        },
    }
}

/// Disk-mode testharness HTML for a test path: the file's contents (testharness
/// only; XHTML and non-testharness skipped) or a synthesized `.any.js` wrapper.
/// Mirrors the disk branch of [`testharness`], shared by [`compare`].
enum TestHtml {
    Html(String),
    Skip(&'static str),
    ReadError,
}

fn worker_family_reason(test: &TestCase) -> &'static str {
    let name = test.name();
    if name.contains(".sharedworker.") {
        "sharedworker-unsupported"
    } else if name.contains(".serviceworker.") {
        "serviceworker-unsupported"
    } else if name.contains(".shadowrealm-") {
        "shadowrealm-unsupported"
    } else if name.contains(".worker.") || name.contains(".worker?") {
        "dedicated-worker-unsupported"
    } else {
        "non-window-global"
    }
}

fn load_test_document_disk(test: &TestCase) -> TestHtml {
    if is_any_js(&test.path) {
        return match synthesize_any_js(&test.path, Some(test.name())) {
            Some(h) => TestHtml::Html(h),
            None => TestHtml::Skip(worker_family_reason(test)),
        };
    }
    let Ok(bytes) = fs::read(&test.path) else {
        return TestHtml::ReadError;
    };
    TestHtml::Html(String::from_utf8_lossy(&bytes).into_owned())
}

fn build_test_html_disk(test: &TestCase) -> TestHtml {
    if test.kind != Kind::Testharness {
        return TestHtml::Skip("non-testharness");
    }
    let ext = test.path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("xhtml") || ext.eq_ignore_ascii_case("xht") {
        return TestHtml::Skip("xhtml"); // XML parse mode serval's HTML parser doesn't handle
    }
    load_test_document_disk(test)
}

/// The cross-engine pass predicate: a caught run that did not panic or throw,
/// produced results, and every subtest passed.
fn outcome_passes(result: &Result<harness::HarnessOutcome, Box<dyn std::any::Any + Send>>) -> bool {
    matches!(
        result,
        Ok(harness::HarnessOutcome::Ran(results))
            if !results.is_empty() && results.iter().all(|r| r.passed())
    )
}

struct ActualRecord {
    test: String,
    status: &'static str,
    reason: Option<String>,
}

impl ActualRecord {
    fn new(test: &TestCase, status: &'static str) -> ActualRecord {
        ActualRecord {
            test: test.name().to_string(),
            status,
            reason: None,
        }
    }

    fn with_reason(test: &TestCase, status: &'static str, reason: impl Into<String>) -> ActualRecord {
        ActualRecord {
            test: test.name().to_string(),
            status,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExpectedRecord {
    status: String,
    reason: Option<String>,
}

impl ExpectedRecord {
    fn matches(&self, actual: &ActualRecord) -> bool {
        self.status == actual.status
            && self
                .reason
                .as_deref()
                .is_none_or(|reason| actual.reason.as_deref() == Some(reason))
    }

    fn describe(&self) -> String {
        match self.reason.as_deref() {
            Some(reason) => format!("{} ({reason})", self.status),
            None => self.status.clone(),
        }
    }
}

struct ActualRecordDisplay<'a>(&'a ActualRecord);

impl std::fmt::Display for ActualRecordDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.reason.as_deref() {
            Some(reason) => write!(f, "{} ({reason})", self.0.status),
            None => f.write_str(self.0.status),
        }
    }
}

fn finish_expectations(args: &Args, command: &str, actuals: &[ActualRecord]) {
    if let Some(out) = &args.write_expectations {
        if let Err(e) = write_expectations(out, command, args.engine.label(), actuals) {
            eprintln!("failed to write expectations to {out}: {e}");
            std::process::exit(1);
        }
        println!("expectations written to {out} ({} tests)", actuals.len());
    }
    if let Some(path) = &args.expectations {
        match check_expectations(path, actuals) {
            Ok(()) => println!("expectations: unexpected=0"),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            },
        }
    }
}

fn write_expectations(
    path: &str,
    command: &str,
    engine: &str,
    actuals: &[ActualRecord],
) -> Result<(), String> {
    let mut tests = BTreeMap::new();
    for actual in actuals {
        let value = match actual.reason.as_deref() {
            Some(reason) => serde_json::json!({
                "status": actual.status,
                "reason": reason,
            }),
            None => serde_json::Value::String(actual.status.to_string()),
        };
        tests.insert(actual.test.clone(), value);
    }
    let value = serde_json::json!({
        "version": 1,
        "command": command,
        "engine": engine,
        "tests": tests,
    });
    let out = Path::new(path);
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        out,
        serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn load_expectations(path: &str) -> Result<BTreeMap<String, ExpectedRecord>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("expectations read failed ({path}): {e}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("expectations parse failed ({path}): {e}"))?;
    let tests = value.get("tests").unwrap_or(&value);
    let obj = tests.as_object().ok_or_else(|| {
        format!("expectations file {path} must be an object or carry a `tests` object")
    })?;
    let mut out = BTreeMap::new();
    for (name, expected) in obj {
        let record = match expected {
            serde_json::Value::String(status) => ExpectedRecord {
                status: status.to_ascii_lowercase(),
                reason: None,
            },
            serde_json::Value::Object(fields) => {
                let Some(status) = fields.get("status").and_then(serde_json::Value::as_str) else {
                    return Err(format!(
                        "expectation for {name} must carry a string `status` field"
                    ));
                };
                let reason = match fields.get("reason") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(serde_json::Value::String(reason)) => Some(reason.clone()),
                    Some(_) => {
                        return Err(format!(
                            "expectation for {name} must carry a string `reason` field"
                        ));
                    },
                };
                ExpectedRecord {
                    status: status.to_ascii_lowercase(),
                    reason,
                }
            },
            _ => {
                return Err(format!(
                    "expectation for {name} must be a string or an object with `status`"
                ));
            },
        };
        out.insert(name.clone(), record);
    }
    Ok(out)
}

fn check_expectations(path: &str, actuals: &[ActualRecord]) -> Result<(), String> {
    let expected = load_expectations(path)?;
    let actual_names: BTreeSet<&str> = actuals.iter().map(|a| a.test.as_str()).collect();
    let mut unexpected = Vec::new();
    for actual in actuals {
        match expected.get(&actual.test) {
            Some(record) if record.matches(actual) => {},
            Some(record) => unexpected.push(format!(
                "{}: expected {}, got {}",
                actual.test,
                record.describe(),
                ActualRecordDisplay(actual)
            )),
            None => unexpected.push(format!(
                "{}: missing expectation, got {}",
                actual.test,
                ActualRecordDisplay(actual)
            )),
        }
    }
    for expected_name in expected.keys() {
        if !actual_names.contains(expected_name.as_str()) {
            let status = expected
                .get(expected_name)
                .map(ExpectedRecord::describe)
                .unwrap_or_else(|| "<missing>".to_string());
            unexpected.push(format!(
                "{expected_name}: expected {status}, but test was not run"
            ));
        }
    }
    if unexpected.is_empty() {
        return Ok(());
    }
    let mut msg = format!("expectations: unexpected={} ({path})", unexpected.len());
    for line in unexpected.iter().take(40) {
        msg.push_str("\n  ");
        msg.push_str(line);
    }
    if unexpected.len() > 40 {
        msg.push_str(&format!("\n  … and {} more", unexpected.len() - 40));
    }
    Err(msg)
}

/// Phase 3 / harness-exactness H2b: run each testharness test on **both** engines
/// (Boa + Nova) and diff. A test that passes on Boa but fails on Nova is a **Nova
/// JS-engine gap** (Nova's worklist, the fork-improvement signal); a test that
/// fails on both is a **serval-platform gap** (layout / DOM). Disk mode only.
fn compare(tests: &[TestCase], args: &Args) {
    let tests_root = Path::new(&args.tests_root);
    let testharness_js = match fs::read_to_string(tests_root.join("resources/testharness.js")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("testharness.js not found under {}", tests_root.display());
            std::process::exit(2);
        },
    };
    // Boa / Nova can panic on unimplemented paths; swallow the hooks like `testharness`.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut both_pass, mut both_fail, mut boa_only, mut nova_only, mut skipped) = (0, 0, 0, 0, 0);
    let mut nova_worklist: Vec<String> = Vec::new();

    for test in tests {
        let html = match build_test_html_disk(test) {
            TestHtml::Html(h) => h,
            TestHtml::Skip(_) => {
                skipped += 1;
                continue;
            },
            TestHtml::ReadError => {
                skipped += 1;
                continue;
            },
        };
        let base_dir = test.path.parent().unwrap_or(tests_root);
        let disk = harness::DiskLoader {
            base_dir,
            tests_root,
        };
        let doc_url = test.disk_doc_url();
        let run = |engine| {
            panic::catch_unwind(AssertUnwindSafe(|| {
                harness::run_test(
                    &testharness_js,
                    &html,
                    &disk,
                    Some(&doc_url),
                    None,
                    None,
                    engine,
                )
            }))
        };
        let boa = run(harness::Engine::Boa);
        let nova = run(harness::Engine::Nova);
        let name = test.name();
        match (outcome_passes(&boa), outcome_passes(&nova)) {
            (true, true) => both_pass += 1,
            (false, false) => both_fail += 1,
            (false, true) => nova_only += 1,
            (true, false) => {
                boa_only += 1;
                nova_worklist.push(name.to_string());
                if args.verbose {
                    println!("NOVA-GAP  {name}");
                }
            },
        }
    }
    panic::set_hook(prev);

    println!(
        "\ncompare [{}]: both-pass={both_pass} both-fail={both_fail} (serval-platform gap) \
         boa-only={boa_only} (Nova gap) nova-only={nova_only} skipped={skipped}",
        if args.subset.is_empty() {
            "<all>"
        } else {
            &args.subset
        },
    );
    if !nova_worklist.is_empty() {
        println!(
            "\nNova worklist (pass on Boa, fail on Nova) — {} test(s):",
            nova_worklist.len()
        );
        for name in nova_worklist.iter().take(40) {
            println!("  {name}");
        }
        if nova_worklist.len() > 40 {
            println!("  … and {} more", nova_worklist.len() - 40);
        }
    }
}

/// One test262 outcome.
enum T262 {
    Pass,
    Fail,
    Skip,
}

/// Run one test262 test on engine `E` and classify it.
///
/// **module** (`flags: [module]`) installs the harness preamble as a script, then
/// evaluates the test as a module (imports resolved against the test's directory).
/// Otherwise it assembles (harness + includes + test) for each strict variant and
/// evals. A positive test passes iff it does not throw; a negative test passes iff it
/// throws an error of the expected type (matched against the thrown value's toString).
/// `async` tests report completion through `$DONE`; `module` tests run as ES modules.
fn run_262<E: ScriptEngine>(
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
    path: &Path,
) -> T262 {
    if meta.flags.r#async {
        return run_262_async::<E>(hns, test_src, meta);
    }
    if meta.flags.module {
        return run_262_module::<E>(hns, test_src, meta, path);
    }
    let negative = meta.negative.as_ref();
    for &strict in &test262::strict_variants(&meta.flags) {
        let Ok(script) = hns.assemble(test_src, meta, strict) else {
            return T262::Skip; // a missing include file
        };
        // Ok(()) = ran without throwing; Err(desc) = threw, with the error's toString.
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<(), String> {
            let mut rt = script_runtime_api::Runtime::<E>::new().map_err(|_| String::new())?;
            match rt.eval(&script) {
                Ok(_) => Ok(()),
                Err(e) => Err(rt.describe_error(&e)),
            }
        }));
        let ran = match outcome {
            Ok(r) => r,
            Err(_) => return T262::Fail, // the engine panicked on this source
        };
        let ok = match (negative, ran) {
            (None, Ok(())) => true,     // positive: must not throw
            (None, Err(_)) => false,    // positive: threw
            (Some(_), Ok(())) => false, // negative: must throw
            (Some(neg), Err(desc)) => negative_matches(&desc, neg), // negative: right type
        };
        if !ok {
            return T262::Fail;
        }
    }
    T262::Pass
}

/// Whether a thrown error's description satisfies a `negative:` expectation. Both
/// engines name the JS constructor (e.g. "TypeError") in the thrown value's `toString`;
/// Nova additionally reports a parse failure as the literal "parse error", so a
/// parse-phase negative also matches that.
fn negative_matches(desc: &str, neg: &test262::Negative) -> bool {
    desc.contains(&neg.error_type)
        || (matches!(neg.phase.as_str(), "parse" | "early") && desc.contains("parse error"))
}

/// Module test: evaluate the harness preamble as a sloppy script (so its globals
/// land on `globalThis`), then run the test as a module. Imports resolve against the
/// importing file's directory (the entry module's referrer is its own path).
fn run_262_module<E: ScriptEngine>(
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
    path: &Path,
) -> T262 {
    let Ok(preamble) = hns.preamble(meta) else {
        return T262::Skip; // a missing include file
    };
    let negative = meta.negative.is_some();
    let base = path.to_string_lossy().into_owned();
    let test_src = test_src.to_string();
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || {
        let Ok(mut rt) = script_runtime_api::Runtime::<E>::new() else {
            return true;
        };
        if rt.eval(&preamble).is_err() {
            return true; // the harness itself failed to load
        }
        let mut resolve = |specifier: &str, referrer: &str| -> Option<(String, String)> {
            let target = Path::new(referrer).parent()?.join(specifier);
            let src = std::fs::read_to_string(&target).ok()?;
            Some((target.to_string_lossy().into_owned(), src))
        };
        rt.eval_module(&test_src, &base, &mut resolve).is_err()
    }));
    let threw = match outcome {
        Ok(t) => t,
        Err(_) => return T262::Fail,
    };
    if threw != negative {
        T262::Fail
    } else {
        T262::Pass
    }
}

/// Async test: the test signals completion through `$DONE`, which the harness's
/// `doneprintHandle.js` reports via `print`. We shim `print` into a JS buffer, run the
/// test, drive the event loop to settle promise/timer jobs, then read the buffer back
/// and scan for the `Test262:AsyncTestComplete` sentinel (absent or `…Failure` = fail).
///
/// Re-enabled once per-test worker-subprocess isolation existed: each async test runs
/// in its own reaped process bounded by `--timeout`, so a non-settling test is a clean
/// timeout, not the cross-test memory blow-up that forced the earlier in-process revert.
fn run_262_async<E: ScriptEngine>(
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
) -> T262 {
    let Ok(preamble) = hns.preamble(meta) else {
        return T262::Skip; // a missing include file
    };
    let negative = meta.negative.is_some();
    // `print` is defined before `$DONE` is invoked; `doneprintHandle.js` (in the
    // preamble) calls it on completion. The host captures `console`, but the test262
    // async harness uses `print`, so route it into a buffer we can read back.
    let script = format!(
        "globalThis.__262log='';globalThis.print=function(s){{__262log+=String(s)+'\\n';}};\n{preamble}{test_src}"
    );
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || -> bool {
        let Ok(mut rt) = script_runtime_api::Runtime::<E>::new() else {
            return true;
        };
        if rt.eval(&script).is_err() {
            return true; // threw synchronously before completing
        }
        let _ = rt.run_event_loop(1024); // settle promise/timer jobs (breaks when idle)
        let log = rt
            .eval("__262log")
            .ok()
            .and_then(|v| rt.value_to_string(&v).ok())
            .unwrap_or_default();
        let passed = log.contains("Test262:AsyncTestComplete")
            && !log.contains("Test262:AsyncTestFailure");
        !passed // threw-style: true = did not pass
    }));
    let threw = match outcome {
        Ok(t) => t,
        Err(_) => return T262::Fail,
    };
    if threw != negative {
        T262::Fail
    } else {
        T262::Pass
    }
}

/// Dispatch [`run_262`] to the concrete engine, mirroring `harness::run_test`.
fn run_262_on(
    engine: harness::Engine,
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
    path: &Path,
) -> T262 {
    match engine {
        harness::Engine::Boa => run_262::<script_engine_boa::BoaEngine>(hns, test_src, meta, path),
        harness::Engine::Nova => {
            run_262::<script_engine_nova::NovaEngine>(hns, test_src, meta, path)
        },
    }
}

/// Worker mode: run ONE test262 test (both engines) and print per-engine results,
/// each line flushed, so the parent ([`test262_cmd`]) can attribute a hang to the
/// engine that never reported. The parent spawns this as a subprocess per test, so a
/// hanging test (the engines cannot be step-metered) kills only this process.
fn test262_one(args: &Args) {
    use std::io::Write;
    // A panicking test is caught by run_262's catch_unwind (→ Fail); silence the hook.
    panic::set_hook(Box::new(|_| {}));

    let t262_root = Path::new(&args.tests_root).join("third_party/test262");
    let hns = match test262::Harness::load(&t262_root.join("harness")) {
        Ok(h) => h,
        Err(_) => std::process::exit(2), // parent sees no output → counts as skip
    };
    let path = t262_root.join("test").join(&args.subset);
    let Ok(src) = fs::read_to_string(&path) else {
        std::process::exit(2);
    };
    let meta = test262::parse_meta(&src);

    let mut so = std::io::stdout();
    let boa = run_262_on(harness::Engine::Boa, &hns, &src, &meta, &path);
    let _ = writeln!(so, "boa {}", t262_word(&boa));
    let _ = so.flush();
    let nova = run_262_on(harness::Engine::Nova, &hns, &src, &meta, &path);
    let _ = writeln!(so, "nova {}", t262_word(&nova));
    let _ = so.flush();
}

/// The wire word for one engine's outcome (the `test262-one` worker protocol).
fn t262_word(t: &T262) -> &'static str {
    match t {
        T262::Pass => "pass",
        T262::Fail => "fail",
        T262::Skip => "skip",
    }
}

/// Parse a `<engine> <pass|fail|skip>` line from a worker's output. `None` if the
/// engine never reported (it hung, or the worker died before reaching it).
fn parse_engine_result(out: &str, engine: &str) -> Option<T262> {
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix(engine) {
            return Some(match rest.trim() {
                "pass" => T262::Pass,
                "fail" => T262::Fail,
                _ => T262::Skip,
            });
        }
    }
    None
}

fn is_262_test(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "js")
        && !p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_FIXTURE.js"))
}

fn collect_262(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        if is_262_test(dir) {
            out.push(dir.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect_262(&p, out);
        } else if is_262_test(&p) {
            out.push(p);
        }
    }
}

/// `test262 <subset>`: run each test262 test (under `third_party/test262/test/<subset>`)
/// on **both** engines and diff. Boa-pass / Nova-fail is a **Nova JS-engine gap** —
/// the actual Nova worklist, since WPT showed Boa/Nova at parity. Disk only; run in
/// **release** (debug frames overflow on bounded-deep recursion).
fn test262_cmd(args: &Args) {
    let t262_root = Path::new(&args.tests_root).join("third_party/test262");
    // Preflight: fail fast with a clear message if the harness is missing. The actual
    // runs happen in `test262-one` worker subprocesses, which load it themselves.
    if let Err(e) = test262::Harness::load(&t262_root.join("harness")) {
        eprintln!("test262 harness load failed ({}): {e}", t262_root.display());
        std::process::exit(2);
    }
    let subset_dir = t262_root.join("test").join(&args.subset);
    if !subset_dir.exists() {
        eprintln!(
            "test262 subset path does not exist: {}",
            subset_dir.display()
        );
        std::process::exit(2);
    }
    let mut files = Vec::new();
    collect_262(&subset_dir, &mut files);
    let test_root = t262_root.join("test");

    // Boa and Nova cannot be step-metered (eval_bounded is unbounded for both), so a
    // pathological test (e.g. a Promise.race iterator-close infinite loop) would hang
    // the whole run. We isolate each test in a worker subprocess (`test262-one`) with a
    // wall-clock timeout: a hang kills only that process, is recorded as a timeout
    // (attributed to whichever engine never reported), and the run continues. A shared
    // work index keeps the worker pool balanced across the sorted corpus; jemalloc is
    // already linked, so per-test cost is engine-bound, not allocator-bound. Process
    // startup (~0.1s) is modest against per-test engine work, the price of hang-safety.
    let test_timeout = std::time::Duration::from_secs(args.timeout_secs);

    #[derive(Default)]
    struct Tally {
        both_pass: u64,
        both_fail: u64,
        boa_only: u64,
        nova_only: u64,
        skipped: u64,
        timeout: u64,
        worklist: Vec<String>,
        timeouts: Vec<String>,
    }

    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let verbose = args.verbose;
    let test_root = test_root.as_path();
    let files = &files;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let next = &next;
    let tests_root = args.tests_root.as_str();
    let exe = std::env::current_exe().ok();
    let exe = exe.as_deref();
    let subset_label = if args.subset.is_empty() {
        "<all>"
    } else {
        &args.subset
    };
    println!(
        "test262 [{subset_label}]: {} tests x 2 engines on {jobs} worker procs (timeout {}s)…",
        files.len(),
        test_timeout.as_secs(),
    );

    let tally = std::thread::scope(|scope| {
        // A shared work index: workers pull the next test as they finish, so the
        // heterogeneous corpus stays balanced (contiguous chunks imbalance when the
        // slow both-pass tests cluster, as they do in the sorted corpus).
        let handles: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(move || {
                    let mut t = Tally::default();
                    let Some(exe) = exe else {
                        return t; // cannot locate our own binary to spawn workers
                    };
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= files.len() {
                            break;
                        }
                        let path = &files[i];
                        let rel = path.strip_prefix(test_root).unwrap_or(path);
                        let name = rel.to_string_lossy().replace('\\', "/");

                        let spawned = std::process::Command::new(exe)
                            .arg("test262-one")
                            .arg(rel.as_os_str())
                            .arg("--tests-root")
                            .arg(tests_root)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::null())
                            .spawn();
                        let Ok(mut child) = spawned else {
                            t.skipped += 1;
                            continue;
                        };

                        let start = std::time::Instant::now();
                        let timed_out = loop {
                            match child.try_wait() {
                                Ok(Some(_)) => break false,
                                Ok(None) => {},
                                Err(_) => break false,
                            }
                            if start.elapsed() >= test_timeout {
                                let _ = child.kill();
                                let _ = child.wait();
                                break true;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        };
                        let mut out = String::new();
                        if let Some(mut so) = child.stdout.take() {
                            use std::io::Read;
                            let _ = so.read_to_string(&mut out);
                        }
                        let boa = parse_engine_result(&out, "boa");
                        let nova = parse_engine_result(&out, "nova");

                        if timed_out {
                            // Whichever engine never reported is the one still spinning.
                            let eng = if boa.is_none() { "boa" } else { "nova" };
                            if verbose {
                                println!("TIMEOUT[{eng}]  {name}");
                            }
                            t.timeout += 1;
                            t.timeouts.push(format!("{name} ({eng})"));
                            continue;
                        }
                        let (b, n) = match (boa, nova) {
                            (Some(b), Some(n)) => (b, n),
                            (Some(b), None) => (b, T262::Fail), // nova crashed mid-test
                            (None, Some(n)) => (T262::Fail, n), // boa crashed mid-test
                            (None, None) => {
                                t.skipped += 1; // worker produced nothing (load/early crash)
                                continue;
                            },
                        };
                        match (b, n) {
                            (T262::Skip, _) | (_, T262::Skip) => t.skipped += 1,
                            (T262::Pass, T262::Pass) => t.both_pass += 1,
                            (T262::Fail, T262::Fail) => t.both_fail += 1,
                            (T262::Pass, T262::Fail) => {
                                if verbose {
                                    println!("NOVA-GAP  {name}");
                                }
                                t.boa_only += 1;
                                t.worklist.push(name);
                            },
                            (T262::Fail, T262::Pass) => t.nova_only += 1,
                        }
                    }
                    t
                })
            })
            .collect();
        let mut total = Tally::default();
        for h in handles {
            let t = h.join().unwrap_or_default();
            total.both_pass += t.both_pass;
            total.both_fail += t.both_fail;
            total.boa_only += t.boa_only;
            total.nova_only += t.nova_only;
            total.skipped += t.skipped;
            total.timeout += t.timeout;
            total.worklist.extend(t.worklist);
            total.timeouts.extend(t.timeouts);
        }
        total
    });

    let mut nova_worklist = tally.worklist;
    nova_worklist.sort();
    let mut timeouts = tally.timeouts;
    timeouts.sort();
    println!(
        "\ntest262 compare [{subset_label}]: both-pass={} both-fail={} boa-only={} (Nova gap) \
         nova-only={} timeout={} skipped={} (module/async/missing)",
        tally.both_pass,
        tally.both_fail,
        tally.boa_only,
        tally.nova_only,
        tally.timeout,
        tally.skipped,
    );
    if !timeouts.is_empty() {
        println!(
            "\nExceeded {}s — infinite hang or pathological slowness (the engine that \
             never reported) — {} test(s):",
            test_timeout.as_secs(),
            timeouts.len()
        );
        for name in timeouts.iter().take(40) {
            println!("  {name}");
        }
        if timeouts.len() > 40 {
            println!("  … and {} more", timeouts.len() - 40);
        }
    }
    if !nova_worklist.is_empty() {
        println!(
            "\nNova worklist (pass on Boa, fail on Nova) — {} test(s):",
            nova_worklist.len()
        );
        for name in nova_worklist.iter().take(40) {
            println!("  {name}");
        }
        if nova_worklist.len() > 40 {
            println!("  … and {} more", nova_worklist.len() - 40);
        }
    }

    if let Some(out_path) = &args.worklist_out {
        use std::io::Write;
        let mut buf = format!(
            "# test262 worklist [{subset_label}]\n\
             # both-pass={} both-fail={} boa-only={} nova-only={} timeout={} skipped={}\n",
            tally.both_pass,
            tally.both_fail,
            tally.boa_only,
            tally.nova_only,
            tally.timeout,
            tally.skipped,
        );
        buf.push_str(&format!(
            "\n## Timeouts (hang or pathological slowness; engine) — {}\n",
            timeouts.len()
        ));
        for t in &timeouts {
            buf.push_str(t);
            buf.push('\n');
        }
        buf.push_str(&format!(
            "\n## Nova gaps (pass on Boa, fail on Nova) — {}\n",
            nova_worklist.len()
        ));
        for n in &nova_worklist {
            buf.push_str(n);
            buf.push('\n');
        }
        match std::fs::File::create(out_path).and_then(|mut f| f.write_all(buf.as_bytes())) {
            Ok(()) => println!(
                "\nworklist written to {out_path} ({} Nova gaps, {} timeouts)",
                nova_worklist.len(),
                timeouts.len()
            ),
            Err(e) => eprintln!("failed to write worklist to {out_path}: {e}"),
        }
    }
}

/// Phase 3: run testharness.js tests and report per-subtest results.
fn testharness(tests: &[TestCase], args: &Args) {
    let tests_root = Path::new(&args.tests_root);
    let th_path = tests_root.join("resources/testharness.js");
    let testharness_js = match fs::read_to_string(&th_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("testharness.js not found at {}", th_path.display());
            std::process::exit(2);
        },
    };

    // Server mode (netfetch): connect to / spawn a `wpt serve` so `fetch()` hits a
    // real server, `<script src>` is fetched (`.sub.js` substituted), and the
    // document base URL resolves relative URLs. Disk mode leaves this `None`.
    #[cfg(feature = "netfetch")]
    let server = setup_server(args);
    #[cfg(not(feature = "netfetch"))]
    if args.spawn_server || args.server_base.is_some() {
        eprintln!("server mode (--server-base / --spawn-server) needs `--features netfetch`");
        std::process::exit(2);
    }

    // Boa / the bridge can panic on unimplemented paths; report, don't spam.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut all_pass, mut with_fail, mut errored, mut no_results, mut skipped) = (0, 0, 0, 0, 0);
    let (mut sub_passed, mut sub_total) = (0usize, 0usize);
    let mut actuals = Vec::new();
    let mut nova_template = if args.engine == harness::Engine::Nova {
        match harness::NovaHarnessTemplate::new(&testharness_js) {
            Ok(template) => Some(template),
            Err(e) => {
                eprintln!("Nova harness template init failed: {e}");
                std::process::exit(2);
            },
        }
    } else {
        None
    };

    for test in tests {
        if test.kind != Kind::Testharness {
            skipped += 1;
            actuals.push(ActualRecord::with_reason(test, "skip", "non-testharness"));
            if args.verbose {
                println!("SKIP  non-testharness {}", test.name());
            }
            continue;
        }
        let ext = test.path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("xhtml") || ext.eq_ignore_ascii_case("xht") {
            skipped += 1;
            actuals.push(ActualRecord::with_reason(test, "skip", "xhtml"));
            if args.verbose {
                println!("SKIP  xhtml          {}", test.name());
            }
            continue;
        }
        // Build the testharness HTML: a real .html document's contents, or a
        // synthesized wrapper for a `.any.js` / `.window.js` test.
        let html = {
            #[cfg(feature = "netfetch")]
            if let Some(s) = &server {
                match net::http_get(&s.doc_url(test.name())) {
                    Some(t) => t,
                    None => {
                        errored += 1;
                        actuals.push(ActualRecord::with_reason(test, "error", "fetch-load-failed"));
                        println!("ERROR fetch   {}", test.name());
                        continue;
                    },
                }
            } else {
                match build_test_html_disk(test) {
                    TestHtml::Html(h) => h,
                    TestHtml::Skip(reason) => {
                        skipped += 1;
                        actuals.push(ActualRecord::with_reason(test, "skip", reason));
                        if args.verbose {
                            println!("SKIP  {reason:16} {}", test.name());
                        }
                        continue;
                    },
                    TestHtml::ReadError => {
                        errored += 1;
                        actuals.push(ActualRecord::with_reason(test, "error", "read-failed"));
                        println!("ERROR read    {}", test.name());
                        continue;
                    },
                }
            }
            #[cfg(not(feature = "netfetch"))]
            {
                match build_test_html_disk(test) {
                    TestHtml::Html(h) => h,
                    TestHtml::Skip(reason) => {
                        skipped += 1;
                        actuals.push(ActualRecord::with_reason(test, "skip", reason));
                        if args.verbose {
                            println!("SKIP  {reason:16} {}", test.name());
                        }
                        continue;
                    },
                    TestHtml::ReadError => {
                        errored += 1;
                        actuals.push(ActualRecord::with_reason(test, "error", "read-failed"));
                        println!("ERROR read    {}", test.name());
                        continue;
                    },
                }
            }
        };

        let base_dir = test.path.parent().unwrap_or(tests_root);
        let disk = harness::DiskLoader {
            base_dir,
            tests_root,
        };
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            // Server mode: a fresh per-test fetch-event channel feeds the drive loop,
            // so deferred fetches settle out of band, mid-flight abort works, and a
            // hung fetch hits the per-test deadline. The shared worker routes replies
            // to this channel; a late reply from a prior test lands on a dropped
            // channel and is harmlessly discarded.
            #[cfg(feature = "netfetch")]
            if let Some(s) = &server {
                let (ev_tx, ev_rx) = std::sync::mpsc::channel::<net::FetchEvent>();
                let doc_url = s.doc_url(test.name());
                let loader = s.loader(&doc_url);
                let handler = net::NetFetchHandler::new(ev_tx);
                let completion = net::ChannelCompletion::new(ev_rx);
                if let Some(template) = nova_template.as_mut() {
                    return template.run_test(
                        &html,
                        &loader,
                        Some(&doc_url),
                        Some(Box::new(handler)),
                        Some(&completion),
                    );
                }
                return harness::run_test(
                    &testharness_js,
                    &html,
                    &loader,
                    Some(&doc_url),
                    Some(Box::new(handler)),
                    Some(&completion),
                    args.engine,
                );
            }
            let doc_url = test.disk_doc_url();
            if let Some(template) = nova_template.as_mut() {
                return template.run_test(&html, &disk, Some(&doc_url), None, None);
            }
            harness::run_test(
                &testharness_js,
                &html,
                &disk,
                Some(&doc_url),
                None,
                None,
                args.engine,
            )
        }));
        let name = test.name();

        match result {
            Err(_) => {
                errored += 1;
                actuals.push(ActualRecord::with_reason(test, "error", "panic"));
                println!("ERROR panic   {name}");
            },
            Ok(harness::HarnessOutcome::Threw(msg)) => {
                errored += 1;
                actuals.push(ActualRecord::with_reason(test, "error", "evaluation-threw"));
                println!("ERROR {name}  ({msg})");
            },
            Ok(harness::HarnessOutcome::Ran(results)) => {
                let total = results.len();
                let passed = results.iter().filter(|r| r.passed()).count();
                sub_passed += passed;
                sub_total += total;
                if total == 0 {
                    no_results += 1;
                    actuals.push(ActualRecord::with_reason(test, "no-results", "no-subtests"));
                    if args.verbose {
                        println!("NORES {name}  (harness ran but reported no subtests)");
                    }
                } else if passed == total {
                    all_pass += 1;
                    actuals.push(ActualRecord::new(test, "pass"));
                    if args.verbose {
                        println!("PASS  {name}  ({passed}/{total})");
                    }
                } else {
                    with_fail += 1;
                    actuals.push(ActualRecord::new(test, "fail"));
                    println!("FAIL  {name}  ({passed}/{total} subtests)");
                    if args.verbose {
                        for r in results.iter().filter(|r| !r.passed()) {
                            let msg = r.message.as_deref().unwrap_or("");
                            println!("        [{}] {} {msg}", r.status, r.name);
                        }
                    }
                }
            },
        }
    }

    panic::set_hook(prev);

    println!(
        "\ntestharness [{}]: {all_pass} all-pass, {with_fail} with-failures, {errored} errored, \
         {no_results} no-results, {skipped} skipped (of {} files); \
         subtests {sub_passed}/{sub_total} passed",
        args.engine.label(),
        tests.len(),
    );
    finish_expectations(args, "testharness", &actuals);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Match,
    Mismatch,
}

/// The first `<link rel="match"|"mismatch" href="...">` in a reftest.
fn reftest_ref(html: &str) -> Option<(MatchKind, String)> {
    let doc = StaticDocument::parse_auto(html);
    let no_ns = layout_dom_api::Namespace::default();
    let rel = LocalName::from("rel");
    let href = LocalName::from("href");
    let mut stack = vec![doc.document()];
    while let Some(id) = stack.pop() {
        if doc
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == "link")
        {
            let kind = match doc.attribute(id, &no_ns, &rel) {
                Some("match") => Some(MatchKind::Match),
                Some("mismatch") => Some(MatchKind::Mismatch),
                _ => None,
            };
            if let Some(kind) = kind {
                if let Some(h) = doc.attribute(id, &no_ns, &href) {
                    return Some((kind, h.to_string()));
                }
            }
        }
        stack.extend(doc.dom_children(id));
    }
    None
}

/// Skip reftests needing things we cannot run: scripts (no JS yet).
/// Inline + linked CSS and local images are loaded; remote resources just
/// render as missing.
fn needs_script(html: &str) -> bool {
    html.to_ascii_lowercase().contains("<script")
}

/// WPT `<meta name="fuzzy" content="...">` tolerance, as
/// `(max_per_channel_difference, max_differing_pixels)` upper bounds.
/// Common forms: `maxDifference=0-2;totalPixels=0-100` or `0-2;0-100`.
fn parse_fuzzy(html: &str) -> Option<(u16, u64)> {
    let doc = StaticDocument::parse_auto(html);
    let no_ns = layout_dom_api::Namespace::default();
    let name = LocalName::from("name");
    let content = LocalName::from("content");
    let mut stack = vec![doc.document()];
    while let Some(id) = stack.pop() {
        if doc
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == "meta")
            && doc.attribute(id, &no_ns, &name) == Some("fuzzy")
        {
            if let Some(c) = doc.attribute(id, &no_ns, &content) {
                return parse_fuzzy_content(c);
            }
        }
        stack.extend(doc.dom_children(id));
    }
    None
}

fn parse_fuzzy_content(content: &str) -> Option<(u16, u64)> {
    let (a, b) = content.trim().split_once(';')?;
    Some((range_upper(a)? as u16, range_upper(b)?))
}

/// Upper bound of a fuzzy segment: `label=lo-hi` / `lo-hi` / `n` -> the
/// last number.
fn range_upper(seg: &str) -> Option<u64> {
    let after_eq = seg.rsplit('=').next().unwrap_or(seg);
    after_eq.rsplit('-').next()?.trim().parse::<u64>().ok()
}

/// Whether two images match under an optional fuzzy tolerance. With
/// `None`, exact; with `Some((max_diff, max_pixels))`, at most
/// `max_pixels` may differ by more than `max_diff` on any channel.
fn images_match(a: &render::Image, b: &render::Image, fuzzy: Option<(u16, u64)>) -> bool {
    if a.dimensions() != b.dimensions() {
        return false;
    }
    // A pixel "differs" only if its per-channel delta exceeds `max_diff`; at most
    // `max_pixels` such pixels are tolerated (WPT fuzzy semantics). Preserved
    // exactly from the pre-diagnostics version.
    let (max_diff, max_pixels) = fuzzy.unwrap_or((0, 0));
    let mut differing = 0u64;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let channel_max =
            pa.0.iter()
                .zip(pb.0.iter())
                .map(|(x, y)| (i16::from(*x) - i16::from(*y)).unsigned_abs())
                .max()
                .unwrap_or(0);
        if channel_max > max_diff {
            differing += 1;
            if differing > max_pixels {
                return false;
            }
        }
    }
    true
}

/// Full per-pixel diff between a test render and its reference. The shape of a
/// failure buckets it (Lever 2 diagnosis): `differing == total` with a large
/// `max_channel_diff` → whole render diverges (layout/parse/UA-stylesheet);
/// `max_channel_diff` small with many `differing` → anti-aliasing / sub-pixel
/// (a fuzzy-tolerance case); `max_channel_diff` large but `differing` localized →
/// a specific paint/feature gap. `!same_dims` → a sizing divergence before paint.
struct DiffStats {
    same_dims: bool,
    differing: u64,
    total: u64,
    max_channel_diff: u16,
}

fn diff_stats(a: &render::Image, b: &render::Image) -> DiffStats {
    let total = u64::from(a.width()) * u64::from(a.height());
    if a.dimensions() != b.dimensions() {
        return DiffStats {
            same_dims: false,
            differing: total,
            total,
            max_channel_diff: 255,
        };
    }
    let (mut differing, mut max_channel_diff) = (0u64, 0u16);
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let channel_max =
            pa.0.iter()
                .zip(pb.0.iter())
                .map(|(x, y)| (i16::from(*x) - i16::from(*y)).unsigned_abs())
                .max()
                .unwrap_or(0);
        if channel_max > 0 {
            differing += 1;
            max_channel_diff = max_channel_diff.max(channel_max);
        }
    }
    DiffStats {
        same_dims: true,
        differing,
        total,
        max_channel_diff,
    }
}

/// One-line classification of a FAIL's diff shape, for `-v` triage.
fn diff_label(s: &DiffStats) -> &'static str {
    if !s.same_dims {
        "dims" // different output size — layout/sizing divergence pre-paint
    } else if s.differing == 0 {
        "equal?" // identical yet failed match — a harness/tolerance quirk
    } else if s.total > 0 && s.differing * 100 / s.total >= 50 {
        "whole" // >=50% of pixels differ — wholesale (layout / UA stylesheet)
    } else if s.max_channel_diff <= 16 {
        "aa" // small per-channel diffs — anti-aliasing / sub-pixel
    } else {
        "local" // localized large diffs — a specific paint/feature gap
    }
}

/// Follow a `match` ref chain to its final reference, returning that
/// reference's path + HTML. `mismatch` chains are not followed (the direct
/// reference is used). Capped to avoid cycles.
fn final_ref(start: PathBuf, kind: MatchKind, tests_root: &Path) -> Option<(PathBuf, String)> {
    let mut ref_path = start;
    let mut html = String::from_utf8_lossy(&fs::read(&ref_path).ok()?).into_owned();
    if kind == MatchKind::Mismatch {
        return Some((ref_path, html));
    }
    for _ in 0..10 {
        match reftest_ref(&html) {
            Some((MatchKind::Match, next_href)) => {
                let Some(next) = resolve_ref(&ref_path, &next_href, tests_root) else {
                    break;
                };
                let Ok(bytes) = fs::read(&next) else { break };
                ref_path = next;
                html = String::from_utf8_lossy(&bytes).into_owned();
            },
            _ => break,
        }
    }
    Some((ref_path, html))
}

/// Resolve a reftest `href` to a file: `/`-absolute against the tests
/// root, otherwise relative to the test's directory. Drops fragment/query.
fn resolve_ref(test_path: &Path, href: &str, tests_root: &Path) -> Option<PathBuf> {
    let href = href.split(['#', '?']).next().unwrap_or(href);
    if href.is_empty() {
        return None;
    }
    Some(match href.strip_prefix('/') {
        Some(rest) => tests_root.join(rest),
        None => test_path.parent()?.join(href),
    })
}

fn images_equal(a: &render::Image, b: &render::Image) -> bool {
    a.dimensions() == b.dimensions() && a.as_raw() == b.as_raw()
}

fn reftest(tests: &[TestCase], args: &Args) {
    let renderer = match render::Renderer::boot() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot boot renderer (reftests need a GPU): {e}");
            std::process::exit(1);
        },
    };
    let tests_root = Path::new(&args.tests_root);

    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut passed, mut failed, mut skipped, mut errored) = (0, 0, 0, 0);
    let mut buckets: HashMap<&'static str, u64> = HashMap::new();
    for test in tests {
        let Ok(bytes) = fs::read(&test.path) else {
            errored += 1;
            continue;
        };
        let test_html = String::from_utf8_lossy(&bytes).into_owned();
        if test.kind != Kind::Reftest {
            skipped += 1;
            continue;
        }
        let (kind, href) = if let Some((href, cmp)) = test.refs.first() {
            let kind = match cmp {
                manifest::RefMatch::Equal => MatchKind::Match,
                manifest::RefMatch::NotEqual => MatchKind::Mismatch,
            };
            (kind, href.clone())
        } else {
            let Some((kind, href)) = reftest_ref(&test_html) else {
                skipped += 1;
                continue;
            };
            (kind, href)
        };
        let Some(direct_ref) = resolve_ref(&test.path, &href, tests_root) else {
            skipped += 1;
            continue;
        };
        let Some((ref_path, ref_html)) = final_ref(direct_ref, kind, tests_root) else {
            errored += 1;
            println!("ERROR ref-missing {}", test.name());
            continue;
        };
        if needs_script(&test_html) || needs_script(&ref_html) {
            skipped += 1;
            if args.verbose {
                println!("SKIP  script   {}", test.name());
            }
            continue;
        }

        // Apply the GPU-jitter floor (see FUZZ_FLOOR_*): never compare
        // tighter than it, so a deterministic-to-1/255 render scores stably.
        // A test's explicit <meta fuzzy> widens it where looser.
        let fuzzy = {
            let (d, p) = test
                .fuzzy
                .or_else(|| parse_fuzzy(&test_html))
                .unwrap_or((0, 0));
            Some((d.max(FUZZ_FLOOR_DIFF), p.max(FUZZ_FLOOR_PIXELS)))
        };
        let test_dir = test.path.parent().unwrap_or(tests_root);
        let ref_dir = ref_path.parent().unwrap_or(tests_root);
        let test_xml = is_xml_path(&test.path);
        let ref_xml = is_xml_path(&ref_path);
        let rendered = panic::catch_unwind(AssertUnwindSafe(|| {
            let t = renderer.render_html(
                &test_html, test_dir, tests_root, REFTEST_W, REFTEST_H, test_xml,
            );
            let r = renderer.render_html(
                &ref_html, ref_dir, tests_root, REFTEST_W, REFTEST_H, ref_xml,
            );
            (t, r)
        }));
        let (test_img, ref_img) = match rendered {
            Ok(pair) => pair,
            Err(_) => {
                failed += 1;
                println!("FAIL  crash    {}", test.name());
                continue;
            },
        };

        let matches = images_match(&test_img, &ref_img, fuzzy);
        let pass = match kind {
            MatchKind::Match => matches,
            MatchKind::Mismatch => !matches,
        };
        if pass {
            passed += 1;
            if args.verbose {
                println!("PASS  {}", test.name());
            }
        } else {
            failed += 1;
            let k = if kind == MatchKind::Match {
                "match   "
            } else {
                "mismatch"
            };
            // Diagnose the diff shape (Lever 2 triage). `match` failures get a
            // bucket from the test-vs-ref pixel diff; `mismatch` failures are
            // "matched when it shouldn't", a different shape, tallied separately.
            if kind == MatchKind::Match {
                let s = diff_stats(&test_img, &ref_img);
                let label = diff_label(&s);
                *buckets.entry(label).or_insert(0) += 1;
                if args.verbose {
                    let pct = if s.total > 0 {
                        s.differing * 100 / s.total
                    } else {
                        0
                    };
                    println!(
                        "FAIL  {k} [{label:5}] diff={pct}% maxδ={} {}",
                        s.max_channel_diff,
                        test.name()
                    );
                } else {
                    println!("FAIL  {k} [{label:5}] {}", test.name());
                }
            } else {
                *buckets.entry("mismatch-eq").or_insert(0) += 1;
                println!("FAIL  {k} {}", test.name());
            }
        }
    }

    panic::set_hook(prev);

    println!(
        "\nreftest: {} passed, {} failed, {} skipped, {} errored (of {} files)",
        passed,
        failed,
        skipped,
        errored,
        tests.len()
    );
    if !buckets.is_empty() {
        let mut sorted: Vec<_> = buckets.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        let legend = "dims=size differs | whole=>=50% pixels (layout/UA) | \
                      aa=small per-channel (anti-alias/tolerance) | \
                      local=localized large (feature/paint) | equal?=identical-yet-failed";
        println!(
            "fail buckets: {}",
            sorted
                .iter()
                .map(|(k, n)| format!("{k}={n}"))
                .collect::<Vec<_>>()
                .join("  ")
        );
        println!("  ({legend})");
    }
    if failed > 0 || errored > 0 {
        std::process::exit(1);
    }
}

/// Render each reftest in the subset + its reference to side-by-side
/// PNGs under `.cargo-check-logs/dump/`, for eyeball diagnosis of a
/// `local`-bucket failure. Writes `<stem>.test.png` / `<stem>.ref.png`.
fn dump(tests: &[TestCase], args: &Args) {
    let renderer = match render::Renderer::boot() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot boot renderer (needs a GPU): {e}");
            std::process::exit(1);
        },
    };
    let tests_root = Path::new(&args.tests_root);
    let out_dir = Path::new(".cargo-check-logs/dump");
    let _ = fs::create_dir_all(out_dir);
    for test in tests {
        let Ok(bytes) = fs::read(&test.path) else {
            continue;
        };
        let test_html = String::from_utf8_lossy(&bytes).into_owned();
        if test.kind != Kind::Reftest {
            continue;
        }
        let (kind, href) = if let Some((href, cmp)) = test.refs.first() {
            let kind = match cmp {
                manifest::RefMatch::Equal => MatchKind::Match,
                manifest::RefMatch::NotEqual => MatchKind::Mismatch,
            };
            (kind, href.clone())
        } else {
            let Some((kind, href)) = reftest_ref(&test_html) else {
                continue;
            };
            (kind, href)
        };
        let Some(direct_ref) = resolve_ref(&test.path, &href, tests_root) else {
            continue;
        };
        let Some((ref_path, ref_html)) = final_ref(direct_ref, kind, tests_root) else {
            continue;
        };
        let test_dir = test.path.parent().unwrap_or(tests_root);
        let ref_dir = ref_path.parent().unwrap_or(tests_root);
        let t = renderer.render_html(
            &test_html,
            test_dir,
            tests_root,
            REFTEST_W,
            REFTEST_H,
            is_xml_path(&test.path),
        );
        let r = renderer.render_html(
            &ref_html,
            ref_dir,
            tests_root,
            REFTEST_W,
            REFTEST_H,
            is_xml_path(&ref_path),
        );
        let stem = test
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("dump");
        let tp = out_dir.join(format!("{stem}.test.png"));
        let rp = out_dir.join(format!("{stem}.ref.png"));
        let _ = t.save(&tp);
        let _ = r.save(&rp);
        let s = diff_stats(&t, &r);
        let pct = if s.total > 0 {
            s.differing * 100 / s.total
        } else {
            0
        };
        println!(
            "DUMP {} -> {} / {}  (diff={pct}% maxδ={})",
            test.name(),
            tp.display(),
            rp.display(),
            s.max_channel_diff
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_expectations_path(name: &str) -> String {
        std::env::temp_dir()
            .join(format!("serval-wpt-{name}-{}.json", std::process::id()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn expectations_accept_exact_statuses() {
        let path = temp_expectations_path("exact");
        let actuals = vec![
            ActualRecord {
                test: "dom/example-a.html".into(),
                status: "pass",
                reason: None,
            },
            ActualRecord {
                test: "dom/example-b.html".into(),
                status: "fail",
                reason: None,
            },
        ];
        write_expectations(&path, "testharness", "boa", &actuals).expect("write expectations");
        check_expectations(&path, &actuals).expect("expectations match exactly");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn expectations_reject_changed_statuses() {
        let path = temp_expectations_path("changed");
        let expected = vec![ActualRecord {
            test: "dom/example.html".into(),
            status: "pass",
            reason: None,
        }];
        write_expectations(&path, "testharness", "boa", &expected).expect("write expectations");
        let actual = vec![ActualRecord {
            test: "dom/example.html".into(),
            status: "fail",
            reason: None,
        }];
        let err = check_expectations(&path, &actual).expect_err("changed status is unexpected");
        assert!(err.contains("expected pass, got fail"), "{err}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn expectations_accept_pinned_reasons_and_legacy_strings() {
        let path = temp_expectations_path("reason");
        let expected = vec![
            ActualRecord {
                test: "dom/example-a.html".into(),
                status: "skip",
                reason: Some("worker-only".into()),
            },
            ActualRecord {
                test: "dom/example-b.html".into(),
                status: "pass",
                reason: None,
            },
        ];
        write_expectations(&path, "testharness", "boa", &expected).expect("write expectations");
        check_expectations(&path, &expected).expect("expectations match exact reason");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn expectations_reject_changed_pinned_reason() {
        let path = temp_expectations_path("reason-changed");
        let expected = vec![ActualRecord {
            test: "dom/example.html".into(),
            status: "skip",
            reason: Some("worker-only".into()),
        }];
        write_expectations(&path, "testharness", "boa", &expected).expect("write expectations");
        let actual = vec![ActualRecord {
            test: "dom/example.html".into(),
            status: "skip",
            reason: Some("xhtml".into()),
        }];
        let err = check_expectations(&path, &actual).expect_err("changed reason is unexpected");
        assert!(err.contains("expected skip (worker-only), got skip (xhtml)"), "{err}");
        let _ = fs::remove_file(path);
    }
}

/// Server mode: drive the `fetch/` corpus against a live `wpt serve` (the netfetch
/// feature). The runtime gets a netfetcher-backed `fetch()` handler, `<script src>`
/// resources are HTTP-fetched (so `.sub.js` substitution happens), and the document
/// base URL is set so relative `fetch()` / `Request` URLs resolve to the server.
#[cfg(feature = "netfetch")]
mod net {
    use std::io::{BufRead, BufReader};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::OnceLock;

    use script_runtime_api::{FetchHandler, FetchOutcome, FetchRequest};

    use crate::harness::ScriptSrcLoader;

    /// One persistent worker thread owns the ONLY Tokio runtime that touches
    /// netfetcher, so netfetcher's process-wide hyper client pool binds to a runtime
    /// that is always being driven. Both blocking resource GETs (`Job::Get`) and
    /// deferred `fetch()` calls (`Job::Fetch`) route through it. A current-thread
    /// runtime + `spawn_blocking` job intake keeps the runtime thread free to drive
    /// in-flight fetches; only plain owned data crosses the channel, so the engine
    /// stays `!Send`.
    fn worker_jobs() -> std::sync::mpsc::Sender<Job> {
        static WORKER: OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> = OnceLock::new();
        WORKER
            .get_or_init(|| {
                let (tx, rx) = std::sync::mpsc::channel::<Job>();
                std::thread::spawn(move || worker_loop(rx));
                std::sync::Mutex::new(tx)
            })
            .lock()
            .expect("worker job sender")
            .clone()
    }

    fn worker_loop(rx: std::sync::mpsc::Receiver<Job>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("worker tokio runtime");
        rt.block_on(async move {
            let mut handles: std::collections::HashMap<u64, tokio::task::AbortHandle> =
                std::collections::HashMap::new();
            // Per-fetch pull credit: a chunk is streamed only when the JS body
            // ReadableStream demands one (Job::Pull). Keyed by JS id (the routing key
            // the reply events already use).
            let mut pulls: std::collections::HashMap<u64, tokio::sync::mpsc::UnboundedSender<()>> =
                std::collections::HashMap::new();
            let mut rx = Some(rx);
            loop {
                // Await the next job on the blocking pool, so the runtime thread stays
                // free to drive in-flight fetch tasks meanwhile.
                let owned = rx.take().unwrap();
                let (owned, job) = tokio::task::spawn_blocking(move || {
                    let j = owned.recv();
                    (owned, j)
                })
                .await
                .expect("worker recv join");
                rx = Some(owned);
                match job {
                    Ok(Job::Get(url, reply)) => {
                        tokio::spawn(async move {
                            let _ = reply.send(do_get(&url).await);
                        });
                    },
                    Ok(Job::Fetch(key, id, req, reply)) => {
                        let (pull_tx, pull_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
                        pulls.insert(id, pull_tx);
                        let h = tokio::spawn(run_fetch_streaming(id, req, reply, pull_rx))
                            .abort_handle();
                        handles.insert(key, h);
                    },
                    Ok(Job::Pull(id)) => {
                        // Grant one chunk of credit; a dead receiver (task finished)
                        // means the entry is stale, so drop it.
                        if let Some(tx) = pulls.get(&id) {
                            if tx.send(()).is_err() {
                                pulls.remove(&id);
                            }
                        }
                    },
                    Ok(Job::Cancel(key)) => {
                        if let Some(h) = handles.remove(&key) {
                            h.abort(); // drop the in-flight future
                        }
                    },
                    Err(_) => break, // all senders dropped: shut down
                }
                handles.retain(|_, h| !h.is_finished());
            }
        });
    }

    /// One process-wide HTTP cache, shared across every deferred fetch so the
    /// request cache modes (default / force-cache / only-if-cached / ...) have a
    /// persistent store to act against. WPT cache tests key on a per-subtest uuid,
    /// so a global cache does not cross subtests.
    fn shared_cache() -> std::sync::Arc<netfetcher::InMemoryHttpCache> {
        static CACHE: std::sync::OnceLock<std::sync::Arc<netfetcher::InMemoryHttpCache>> =
            std::sync::OnceLock::new();
        CACHE
            .get_or_init(|| std::sync::Arc::new(netfetcher::InMemoryHttpCache::new()))
            .clone()
    }

    /// One process-wide cookie jar, shared across every deferred fetch so a
    /// `Set-Cookie` from one request is attached to the next (credentials tests set
    /// a cookie, then verify the following request carries it).
    fn shared_cookies() -> std::sync::Arc<netfetcher::InMemoryCookieJar> {
        static JAR: std::sync::OnceLock<std::sync::Arc<netfetcher::InMemoryCookieJar>> =
            std::sync::OnceLock::new();
        JAR.get_or_init(|| std::sync::Arc::new(netfetcher::InMemoryCookieJar::default()))
            .clone()
    }

    /// A `CookieStore` view over the shared jar (`FetchContext.cookies` is a `Box`,
    /// so each context wraps a cheap clone of the shared `Arc`).
    struct SharedJar(std::sync::Arc<netfetcher::InMemoryCookieJar>);
    impl netfetcher::CookieStore for SharedJar {
        fn cookies_for(&self, url: &url::Url, ctx: netfetcher::SameSiteContext) -> Vec<String> {
            self.0.cookies_for(url, ctx)
        }
        fn set_cookie(&self, url: &url::Url, header: &str) {
            self.0.set_cookie(url, header)
        }
    }

    /// A fetch context with the shared HTTP cache + cookie jar wired in (the
    /// `fetch()` path).
    fn fetch_context() -> netfetcher::FetchContext {
        let mut cx = netfetcher::FetchContext::permissive();
        cx.cache = shared_cache();
        cx.cookies = Box::new(SharedJar(shared_cookies()));
        cx
    }

    /// The document (page) origin every fetch is initiated from — the WPT server
    /// origin. Drives cross-origin detection (CORS / response tainting): a request
    /// whose target origin differs is cross-origin. Set once when server mode is
    /// established; `None` in disk mode (every fetch treated as same-origin).
    static PAGE_ORIGIN: std::sync::OnceLock<url::Origin> = std::sync::OnceLock::new();

    /// Record the page origin from the server base (idempotent; first wins).
    pub fn set_page_origin(origin_str: &str) {
        if let Ok(u) = url::Url::parse(origin_str) {
            let _ = PAGE_ORIGIN.set(u.origin());
        }
    }

    fn page_origin() -> Option<url::Origin> {
        PAGE_ORIGIN.get().cloned()
    }

    /// A globally-unique abort key (the JS `id` is per-test, so it cannot key the
    /// shared worker's abort map).
    fn next_key() -> u64 {
        static KEY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Blocking HTTP GET on the worker runtime, body as a (UTF-8-lossy) string.
    /// `None` on parse / network error or non-2xx. Used for `<script src>` and
    /// readiness probes; the caller blocks on the reply.
    pub fn http_get(url: &str) -> Option<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        worker_jobs().send(Job::Get(url.to_owned(), tx)).ok()?;
        rx.recv().ok().flatten()
    }

    async fn do_get(url: &str) -> Option<String> {
        let u = url::Url::parse(url).ok()?;
        let req = netfetcher::Request::get(u);
        let cx = netfetcher::FetchContext::permissive();
        let resp = netfetcher::fetch(req, &cx).await;
        if resp.is_network_error() || resp.status < 200 || resp.status >= 300 {
            return None;
        }
        resp.bytes()
            .await
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The canonical HTTP reason phrase for a status code (netfetcher discards the
    /// wire reason). WPT checks `response.statusText`, so synthesize it.
    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK",
            201 => "Created",
            202 => "Accepted",
            203 => "Non-Authoritative Information",
            204 => "No Content",
            205 => "Reset Content",
            206 => "Partial Content",
            300 => "Multiple Choices",
            301 => "Moved Permanently",
            302 => "Found",
            303 => "See Other",
            304 => "Not Modified",
            307 => "Temporary Redirect",
            308 => "Permanent Redirect",
            400 => "Bad Request",
            401 => "Unauthorized",
            402 => "Payment Required",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            406 => "Not Acceptable",
            408 => "Request Timeout",
            409 => "Conflict",
            410 => "Gone",
            411 => "Length Required",
            412 => "Precondition Failed",
            413 => "Payload Too Large",
            414 => "URI Too Long",
            415 => "Unsupported Media Type",
            416 => "Range Not Satisfiable",
            417 => "Expectation Failed",
            418 => "I'm a Teapot",
            421 => "Misdirected Request",
            422 => "Unprocessable Entity",
            425 => "Too Early",
            426 => "Upgrade Required",
            428 => "Precondition Required",
            429 => "Too Many Requests",
            431 => "Request Header Fields Too Large",
            451 => "Unavailable For Legal Reasons",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            502 => "Bad Gateway",
            503 => "Service Unavailable",
            504 => "Gateway Timeout",
            505 => "HTTP Version Not Supported",
            _ => "",
        }
    }

    fn map_response_type(t: netfetcher::ResponseType) -> String {
        match t {
            netfetcher::ResponseType::Basic => "basic",
            netfetcher::ResponseType::Cors => "cors",
            netfetcher::ResponseType::Opaque => "opaque",
            netfetcher::ResponseType::OpaqueRedirect => "opaqueredirect",
            netfetcher::ResponseType::Error => "error",
        }
        .to_owned()
    }

    /// Run a deferred fetch and report it to the test's channel: a network error is
    /// `Fail`; otherwise `StartStream` once the headers are in (so `await fetch()`
    /// resolves before the body finishes, which is what lets a mid-flight abort run),
    /// then a `Chunk` per body chunk as it decodes, then `Close` (or `Error` if a
    /// chunk fails to decode, which errors the already-resolved response's body).
    /// Dropping this task (Job::Cancel) drops the in-flight body future, cancelling
    /// the request.
    async fn run_fetch_streaming(
        id: u64,
        req: FetchRequest,
        reply: std::sync::mpsc::Sender<FetchEvent>,
        mut pull_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    ) {
        let Ok(url) = url::Url::parse(&req.url) else {
            let _ = reply.send(FetchEvent::Fail(id, "Failed to fetch".to_string()));
            return;
        };
        let mut request = netfetcher::Request::get(url);
        request.method = match req.method.as_str() {
            "GET" => netfetcher::Method::Get,
            "HEAD" => netfetcher::Method::Head,
            "POST" => netfetcher::Method::Post,
            "PUT" => netfetcher::Method::Put,
            "DELETE" => netfetcher::Method::Delete,
            "PATCH" => netfetcher::Method::Patch,
            "OPTIONS" => netfetcher::Method::Options,
            // A custom method token (e.g. "patcH", "REPORT") — kept verbatim so it
            // is treated as non-simple (preflighted) and sent as-is.
            other => netfetcher::Method::Other(other.to_string()),
        };
        request.headers = req.headers;
        request.body = req.body.map(bytes::Bytes::from);
        request.cache = match req.cache.as_str() {
            "no-store" => netfetcher::CacheMode::NoStore,
            "reload" => netfetcher::CacheMode::Reload,
            "no-cache" => netfetcher::CacheMode::NoCache,
            "force-cache" => netfetcher::CacheMode::ForceCache,
            "only-if-cached" => netfetcher::CacheMode::OnlyIfCached,
            _ => netfetcher::CacheMode::Default,
        };
        request.redirect = match req.redirect.as_str() {
            "error" => netfetcher::RedirectMode::Error,
            "manual" => netfetcher::RedirectMode::Manual,
            _ => netfetcher::RedirectMode::Follow,
        };
        request.mode = match req.mode.as_str() {
            "no-cors" => netfetcher::RequestMode::NoCors,
            "same-origin" => netfetcher::RequestMode::SameOrigin,
            "navigate" => netfetcher::RequestMode::Navigate,
            _ => netfetcher::RequestMode::Cors,
        };
        // The initiator origin (the WPT page) drives cross-origin detection. In disk
        // mode it stays None (every fetch is same-origin).
        request.origin = page_origin();
        // Referrer + policy drive the `Referer` header (empty referrer = none).
        request.referrer = (!req.referrer.is_empty())
            .then(|| url::Url::parse(&req.referrer).ok())
            .flatten();
        request.referrer_policy = match req.referrer_policy.as_str() {
            "no-referrer" => netfetcher::ReferrerPolicy::NoReferrer,
            "no-referrer-when-downgrade" => netfetcher::ReferrerPolicy::NoReferrerWhenDowngrade,
            "same-origin" => netfetcher::ReferrerPolicy::SameOrigin,
            "origin" => netfetcher::ReferrerPolicy::Origin,
            "strict-origin" => netfetcher::ReferrerPolicy::StrictOrigin,
            "origin-when-cross-origin" => netfetcher::ReferrerPolicy::OriginWhenCrossOrigin,
            "strict-origin-when-cross-origin" => {
                netfetcher::ReferrerPolicy::StrictOriginWhenCrossOrigin
            },
            "unsafe-url" => netfetcher::ReferrerPolicy::UnsafeUrl,
            _ => netfetcher::ReferrerPolicy::Empty,
        };
        request.credentials = match req.credentials.as_str() {
            "omit" => netfetcher::Credentials::Omit,
            "include" => netfetcher::Credentials::Include,
            _ => netfetcher::Credentials::SameOrigin,
        };
        request.integrity = req.integrity.clone();

        let cx = fetch_context();
        let mut resp = netfetcher::fetch(request, &cx).await;
        if resp.is_network_error() {
            let _ = reply.send(FetchEvent::Fail(id, "Failed to fetch".to_string()));
            return;
        }
        let meta = FetchOutcome {
            network_error: false,
            status: resp.status,
            status_text: reason_phrase(resp.status).to_string(),
            response_type: map_response_type(resp.response_type),
            url: resp
                .url_list
                .last()
                .map(|u| u.to_string())
                .unwrap_or_default(),
            redirected: resp.url_list.len() > 1,
            headers: resp.headers.clone(),
            body: vec![],
        };
        if reply.send(FetchEvent::StartStream(id, meta)).is_err() {
            return;
        }
        // Pull-driven body: stream one chunk per credit from the JS ReadableStream.
        // A body the script never reads sends no credit, so it is never fetched (no
        // streaming a 300 MB response nobody consumes); the task idles here until the
        // test ends and Job::Cancel aborts it.
        while pull_rx.recv().await.is_some() {
            match resp.body.next_chunk().await {
                Some(Ok(bytes)) => {
                    if reply.send(FetchEvent::Chunk(id, bytes.to_vec())).is_err() {
                        return; // the test's channel is gone (run ended)
                    }
                },
                Some(Err(_)) => {
                    // Body decode error (e.g. a bad Content-Encoding): error the
                    // body stream so reads reject, rather than closing it cleanly.
                    let _ = reply.send(FetchEvent::Error(id));
                    return;
                },
                None => {
                    let _ = reply.send(FetchEvent::Close(id));
                    return;
                },
            }
        }
    }

    /// A job for the persistent worker. `Get` is a blocking resource GET (reply: the
    /// body or `None`); `Fetch` is a deferred `fetch()` (reply: a `FetchEvent` to the
    /// test's channel); `Cancel` aborts an in-flight fetch by its global key.
    pub enum Job {
        Get(String, std::sync::mpsc::Sender<Option<String>>),
        Fetch(u64, u64, FetchRequest, std::sync::mpsc::Sender<FetchEvent>),
        /// Demand the next body chunk for a streaming fetch, by its JS id.
        Pull(u64),
        Cancel(u64),
    }

    /// A deferred fetch event, routed to the originating test's channel by the JS
    /// `id` (not the global abort key). A response streams as `StartStream` (status +
    /// headers) -> `Chunk`* (body, as it arrives) -> `Close`, or `Error` if the body
    /// fails partway (e.g. a `Content-Encoding` decode error: the response already
    /// resolved, so its body stream errors and body reads reject). A network error
    /// before the headers is `Fail` (the `fetch()` Promise rejects as a `TypeError`).
    pub enum FetchEvent {
        StartStream(u64, FetchOutcome),
        Chunk(u64, Vec<u8>),
        Close(u64),
        Error(u64),
        Fail(u64, String),
    }

    /// The deferred host `fetch()` seam: `start` hands the request to the shared
    /// worker (tagged with a global key for cancellation + the JS id for routing) and
    /// leaves the JS Promise pending; `cancel` relays an abort. The reply settles
    /// later via the drive loop. This is the actor-mailbox shape: the handler owns a
    /// send into the worker's inbox plus the test's reply channel. Per-test (a fresh
    /// reply channel + key map), so a late reply from a prior test cannot cross over.
    pub struct NetFetchHandler {
        reply: std::sync::mpsc::Sender<FetchEvent>,
        keys: std::cell::RefCell<std::collections::HashMap<u64, u64>>, // js id -> global key
    }

    impl NetFetchHandler {
        pub fn new(reply: std::sync::mpsc::Sender<FetchEvent>) -> Self {
            Self {
                reply,
                keys: std::cell::RefCell::new(std::collections::HashMap::new()),
            }
        }
    }

    impl FetchHandler for NetFetchHandler {
        fn start(&self, id: u64, request: FetchRequest) -> Option<FetchOutcome> {
            let key = next_key();
            self.keys.borrow_mut().insert(id, key);
            let _ = worker_jobs().send(Job::Fetch(key, id, request, self.reply.clone()));
            None // deferred: the drive loop settles it when the reply arrives
        }
        fn cancel(&self, id: u64) {
            if let Some(key) = self.keys.borrow_mut().remove(&id) {
                let _ = worker_jobs().send(Job::Cancel(key));
            }
        }
        fn request_chunk(&self, id: u64) {
            // The body's ReadableStream was read with an empty buffer: ask the worker
            // to stream one more chunk for this fetch (routed by JS id).
            let _ = worker_jobs().send(Job::Pull(id));
        }
    }

    impl Drop for NetFetchHandler {
        // When the per-test handler drops (the Runtime is torn down, e.g. after the
        // drive loop's deadline), cancel every fetch it ever started so the worker
        // drops any still-in-flight future instead of leaking a hung task and a
        // checked-out hyper connection. Cancelling an already-finished key is a no-op.
        fn drop(&mut self) {
            for key in self.keys.borrow().values() {
                let _ = worker_jobs().send(Job::Cancel(*key));
            }
        }
    }

    /// Bridges a test's fetch-event channel to the harness drive loop. Owns the
    /// receiver (per test, created alongside the handler's `Sender`).
    pub struct ChannelCompletion {
        rx: std::sync::mpsc::Receiver<FetchEvent>,
    }

    impl ChannelCompletion {
        pub fn new(rx: std::sync::mpsc::Receiver<FetchEvent>) -> Self {
            Self { rx }
        }
    }

    impl crate::harness::CompletionSource for ChannelCompletion {
        fn drain(&self, apply: &mut dyn FnMut(crate::harness::FetchCompletion)) -> usize {
            let mut n = 0;
            while let Ok(ev) = self.rx.try_recv() {
                apply(to_completion(ev));
                n += 1;
            }
            n
        }
        fn wait(
            &self,
            timeout: std::time::Duration,
            apply: &mut dyn FnMut(crate::harness::FetchCompletion),
        ) -> usize {
            match self.rx.recv_timeout(timeout) {
                Ok(ev) => {
                    apply(to_completion(ev));
                    1
                },
                Err(_) => 0,
            }
        }
    }

    fn to_completion(ev: FetchEvent) -> crate::harness::FetchCompletion {
        match ev {
            FetchEvent::StartStream(id, o) => crate::harness::FetchCompletion::StartStream(id, o),
            FetchEvent::Chunk(id, b) => crate::harness::FetchCompletion::Chunk(id, b),
            FetchEvent::Close(id) => crate::harness::FetchCompletion::Close(id),
            FetchEvent::Error(id) => crate::harness::FetchCompletion::Error(id),
            FetchEvent::Fail(id, m) => crate::harness::FetchCompletion::Fail(id, m),
        }
    }

    /// Loads `<script src>` by HTTP GET, resolving each `src` against the test's
    /// document URL (so `.sub.js` helpers like `get-host-info.sub.js` come back
    /// substituted). One per test (cheap: it owns only the doc URL string).
    pub struct ServerLoader {
        pub doc_url: String,
    }

    impl ScriptSrcLoader for ServerLoader {
        fn load_script(&self, src: &str) -> Option<String> {
            let base = url::Url::parse(&self.doc_url).ok()?;
            let abs = base.join(src).ok()?;
            http_get(abs.as_str())
        }
    }

    /// A connected (or spawned) `wpt serve`. `origin` is the plain-http origin the
    /// runner drives. A spawned server is torn down on drop.
    pub struct ServerCtx {
        pub origin: String,
        _spawned: Option<ServerHandle>,
    }

    impl ServerCtx {
        /// Connect to an already-running server at `origin` (the `--server-base`
        /// path). Probes once so a typo / down server fails loudly up front.
        pub fn connect(origin: String) -> Result<Self, String> {
            let origin = origin.trim_end_matches('/').to_owned();
            if http_get(&format!("{origin}/common/blank.html")).is_none() {
                return Err(format!(
                    "no WPT server reachable at {origin} (is `wpt serve` up?)"
                ));
            }
            Ok(Self {
                origin,
                _spawned: None,
            })
        }

        /// Spawn `python wpt serve` under `tests_root`, discover its plain-http
        /// origin, and wait until it answers. Torn down when the returned ctx drops.
        pub fn spawn(tests_root: &Path) -> Result<Self, String> {
            let handle = ServerHandle::spawn(tests_root)?;
            let origin = handle.origin.clone();
            Ok(Self {
                origin,
                _spawned: Some(handle),
            })
        }

        /// The document URL for a test, given its path relative to the tests root.
        pub fn doc_url(&self, test_rel: &str) -> String {
            format!("{}/{}", self.origin, test_rel.trim_start_matches('/'))
        }

        pub fn loader(&self, doc_url: &str) -> ServerLoader {
            ServerLoader {
                doc_url: doc_url.to_owned(),
            }
        }
    }

    /// A spawned `wpt serve` child; killed (whole tree) on drop.
    pub struct ServerHandle {
        child: Child,
        pub origin: String,
    }

    impl ServerHandle {
        fn spawn(tests_root: &Path) -> Result<Self, String> {
            let mut child = Command::new("python")
                .arg("wpt")
                .arg("serve")
                .current_dir(tests_root)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("spawning `python wpt serve`: {e}"))?;

            // Read stdout until the canonical plain-http server announces its port,
            // then drain the rest off-thread so the pipe never backs up.
            let stdout = child.stdout.take().ok_or("no stdout from wpt serve")?;
            let mut reader = BufReader::new(stdout);
            let mut port = None;
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break, // EOF: server exited before binding
                    Ok(_) => {
                        if let Some(p) = parse_http_port(&line) {
                            port = Some(p);
                            break;
                        }
                    },
                    Err(_) => break,
                }
            }
            std::thread::spawn(move || {
                let mut sink = String::new();
                while reader.read_line(&mut sink).map(|n| n > 0).unwrap_or(false) {
                    sink.clear();
                }
            });

            let port = port.ok_or("could not read the wpt serve http port from its output")?;
            let origin = format!("http://web-platform.test:{port}");

            // Readiness: poll until the server answers (it logs the port before the
            // listener is fully up).
            for _ in 0..50 {
                if http_get(&format!("{origin}/common/blank.html")).is_some() {
                    return Ok(Self { child, origin });
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let _ = child.kill();
            Err(format!("wpt serve bound {origin} but never answered"))
        }
    }

    impl Drop for ServerHandle {
        fn drop(&mut self) {
            // Kill the whole process tree: wpt serve forks per-protocol workers that
            // a bare child.kill() would orphan.
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/T", "/F", "/PID", &self.child.id().to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .output();
            }
            #[cfg(not(windows))]
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// The primary plain-http port from a `wpt serve` log line. The canonical
    /// server is tagged ` http on port N]` (with surrounding spaces, so it does not
    /// match `http-local` / `http-public` / `http2`); the first such line is
    /// `ports.http[0]`, the origin tests fetch from.
    fn parse_http_port(line: &str) -> Option<u16> {
        let tag = " http on port ";
        let start = line.find(tag)? + tag.len();
        let rest = &line[start..];
        let end = rest.find(']')?;
        rest[..end].trim().parse().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_the_primary_http_port_only() {
            // The canonical server line.
            assert_eq!(
                parse_http_port(
                    "[2026-06-02 21:48:27,647 http on port 8000] INFO - Starting http server on http://web-platform.test:8000"
                ),
                Some(8000)
            );
            // The variant servers must not match (their tag is not ` http on port `).
            assert_eq!(
                parse_http_port("[ts http-local on port 62276] INFO - ..."),
                None
            );
            assert_eq!(
                parse_http_port("[ts http-public on port 62277] INFO - ..."),
                None
            );
            assert_eq!(parse_http_port("[ts h2 on port 9000] INFO - ..."), None);
            assert_eq!(parse_http_port("[ts ws on port 62280] INFO - ..."), None);
            // Noise lines.
            assert_eq!(parse_http_port("INFO:root:Status of subprocess ..."), None);
        }

        #[test]
        fn doc_url_joins_origin_and_test_path() {
            let ctx = ServerCtx {
                origin: "http://web-platform.test:8000".into(),
                _spawned: None,
            };
            assert_eq!(
                ctx.doc_url("fetch/api/basic/x.any.js"),
                "http://web-platform.test:8000/fetch/api/basic/x.any.js"
            );
            // A leading slash on the rel path is not doubled.
            assert_eq!(
                ctx.doc_url("/fetch/api/basic/x.any.js"),
                "http://web-platform.test:8000/fetch/api/basic/x.any.js"
            );
        }
    }
}
