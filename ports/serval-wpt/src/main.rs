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
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
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

/// Crash-smoke one test: parse + cascade + layout, catching panics.
fn smoke_test(path: &Path) -> (Kind, Outcome) {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(_) => return (Kind::Load, Outcome::ReadError),
    };
    let html = String::from_utf8_lossy(&bytes).into_owned();
    let kind = classify(path, &html);
    if !kind.runs_in_phase1() {
        return (kind, Outcome::Skipped);
    }

    let is_xml = is_xml_path(path);
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let document =
            if is_xml { StaticDocument::parse_xml(&html) } else { StaticDocument::parse(&html) };
        let sheets = serval_layout::inline_stylesheets_from_source(&html);
        let sheet_refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
        let _fragments = serval_layout::render(&document, &sheet_refs, VIEWPORT_W, VIEWPORT_H);
    }));

    (kind, if result.is_ok() { Outcome::Passed } else { Outcome::Failed })
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
    let Ok(entries) = fs::read_dir(root) else { return };
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
fn synthesize_any_js(path: &Path) -> Option<String> {
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
    html.push_str(&format!("<script src=\"{name}\"></script>\n"));
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
}

fn parse_args() -> Result<Args, String> {
    let mut command = None;
    let mut subset = None;
    let mut tests_root = DEFAULT_TESTS_ROOT.to_string();
    let mut verbose = false;
    let mut engine = harness::Engine::default();
    let mut server_base = None;
    let mut spawn_server = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tests-root" => {
                tests_root = it.next().ok_or("--tests-root needs a value")?;
            }
            "--engine" => {
                let v = it.next().ok_or("--engine needs a value (boa | nova)")?;
                engine = harness::Engine::parse(&v)
                    .ok_or_else(|| format!("unknown engine: {v} (expected boa | nova)"))?;
            }
            "--server-base" => {
                server_base = Some(it.next().ok_or("--server-base needs a URL")?);
            }
            "--spawn-server" => spawn_server = true,
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
        }
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

    let root = Path::new(&args.tests_root).join(&args.subset);
    if !root.exists() {
        eprintln!("subset path does not exist: {}", root.display());
        std::process::exit(2);
    }

    let mut tests = Vec::new();
    collect(&root, &mut tests);
    if tests.is_empty() {
        eprintln!("no HTML tests found under {}", root.display());
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
        }
    }
}

/// Enumerate tests under a subset from MANIFEST.json (harness-exactness H1), for
/// diffing the authoritative manifest enumeration against the directory walk. The
/// manifest sits at `<tests-root>/../meta/MANIFEST.json`. Worker variants are counted
/// but excluded from the runnable total (this window-shaped runner cannot host them).
fn manifest_list(args: &Args) {
    let manifest_path = Path::new(&args.tests_root)
        .parent()
        .map(|p| p.join("meta/MANIFEST.json"))
        .unwrap_or_else(|| PathBuf::from("MANIFEST.json"));
    let manifest = match manifest::Manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("manifest load failed ({}): {e}", manifest_path.display());
            std::process::exit(1);
        }
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
        if args.subset.is_empty() { "<all>" } else { &args.subset },
        by_kind.join(", "),
    );
}

fn rel(path: &Path, tests_root: &str) -> String {
    path.strip_prefix(tests_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn list(tests: &[PathBuf], args: &Args) {
    let mut counts = [0usize; 6];
    for path in tests {
        let contents = fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let kind = classify(path, &contents);
        counts[kind as usize] += 1;
        println!("{:<12} {}", kind.label(), rel(path, &args.tests_root));
    }
    println!(
        "\n{} tests: {} reftest, {} testharness, {} crashtest, {} load, {} manual, {} reference",
        tests.len(),
        counts[Kind::Reftest as usize],
        counts[Kind::Testharness as usize],
        counts[Kind::Crashtest as usize],
        counts[Kind::Load as usize],
        counts[Kind::Manual as usize],
        counts[Kind::Reference as usize],
    );
}

fn run(tests: &[PathBuf], args: &Args) {
    // Quiet the default panic hook so crash-smoke failures do not spam
    // backtraces; the runner reports them itself.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut passed, mut failed, mut skipped, mut errored) = (0, 0, 0, 0);
    for path in tests {
        let (kind, outcome) = smoke_test(path);
        match outcome {
            Outcome::Passed => {
                passed += 1;
                if args.verbose {
                    println!("PASS  {:<12} {}", kind.label(), rel(path, &args.tests_root));
                }
            }
            Outcome::Failed => {
                failed += 1;
                println!("FAIL  {:<12} {}", kind.label(), rel(path, &args.tests_root));
            }
            Outcome::ReadError => {
                errored += 1;
                println!("ERROR read    {}", rel(path, &args.tests_root));
            }
            Outcome::Skipped => {
                skipped += 1;
                if args.verbose {
                    println!("SKIP  {:<12} {}", kind.label(), rel(path, &args.tests_root));
                }
            }
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
        }
        Err(e) => {
            eprintln!("server mode setup failed: {e}");
            std::process::exit(2);
        }
    }
}

/// Disk-mode testharness HTML for a test path: the file's contents (testharness
/// only; XHTML and non-testharness skipped) or a synthesized `.any.js` wrapper.
/// Mirrors the disk branch of [`testharness`], shared by [`compare`].
enum TestHtml {
    Html(String),
    Skip,
}

fn build_test_html_disk(path: &Path) -> TestHtml {
    if is_any_js(path) {
        return match synthesize_any_js(path) {
            Some(h) => TestHtml::Html(h),
            None => TestHtml::Skip, // worker-only
        };
    }
    let Ok(bytes) = fs::read(path) else {
        return TestHtml::Skip;
    };
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    if classify(path, &raw) != Kind::Testharness {
        return TestHtml::Skip;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("xhtml") || ext.eq_ignore_ascii_case("xht") {
        return TestHtml::Skip; // XML parse mode serval's HTML parser doesn't handle
    }
    TestHtml::Html(raw)
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

/// Phase 3 / harness-exactness H2b: run each testharness test on **both** engines
/// (Boa + Nova) and diff. A test that passes on Boa but fails on Nova is a **Nova
/// JS-engine gap** (Nova's worklist, the fork-improvement signal); a test that
/// fails on both is a **serval-platform gap** (layout / DOM). Disk mode only.
fn compare(tests: &[PathBuf], args: &Args) {
    let tests_root = Path::new(&args.tests_root);
    let testharness_js = match fs::read_to_string(tests_root.join("resources/testharness.js")) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("testharness.js not found under {}", tests_root.display());
            std::process::exit(2);
        }
    };
    // Boa / Nova can panic on unimplemented paths; swallow the hooks like `testharness`.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut both_pass, mut both_fail, mut boa_only, mut nova_only, mut skipped) = (0, 0, 0, 0, 0);
    let mut nova_worklist: Vec<String> = Vec::new();

    for path in tests {
        let html = match build_test_html_disk(path) {
            TestHtml::Html(h) => h,
            TestHtml::Skip => {
                skipped += 1;
                continue;
            }
        };
        let base_dir = path.parent().unwrap_or(tests_root);
        let disk = harness::DiskLoader { base_dir, tests_root };
        let run = |engine| {
            panic::catch_unwind(AssertUnwindSafe(|| {
                harness::run_test(&testharness_js, &html, &disk, None, None, None, engine)
            }))
        };
        let boa = run(harness::Engine::Boa);
        let nova = run(harness::Engine::Nova);
        let name = rel(path, &args.tests_root);
        match (outcome_passes(&boa), outcome_passes(&nova)) {
            (true, true) => both_pass += 1,
            (false, false) => both_fail += 1,
            (false, true) => nova_only += 1,
            (true, false) => {
                boa_only += 1;
                nova_worklist.push(name.clone());
                if args.verbose {
                    println!("NOVA-GAP  {name}");
                }
            }
        }
    }
    panic::set_hook(prev);

    println!(
        "\ncompare [{}]: both-pass={both_pass} both-fail={both_fail} (serval-platform gap) \
         boa-only={boa_only} (Nova gap) nova-only={nova_only} skipped={skipped}",
        if args.subset.is_empty() { "<all>" } else { &args.subset },
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

/// Run one test262 test on engine `E`: assemble (harness + includes + flags + test)
/// for each strict variant, eval, and check the result against `negative:`. A
/// positive test passes iff no variant throws; a negative test passes iff every
/// variant throws. v1 skips `module` + `async` (need eval_module / `$DONE`) and a
/// test whose include is missing.
fn run_262<E: ScriptEngine>(
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
) -> T262 {
    if meta.flags.module || meta.flags.r#async {
        return T262::Skip;
    }
    let negative = meta.negative.is_some();
    for &strict in &test262::strict_variants(&meta.flags) {
        let Ok(script) = hns.assemble(test_src, meta, strict) else {
            return T262::Skip; // a missing include file
        };
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            match script_runtime_api::Runtime::<E>::new() {
                Ok(mut rt) => rt.eval(&script).is_err(), // true = threw an uncaught error
                Err(_) => true,
            }
        }));
        let threw = match outcome {
            Ok(t) => t,
            Err(_) => return T262::Fail, // the engine panicked on this source
        };
        if threw != negative {
            return T262::Fail; // positive must not throw; negative must throw
        }
    }
    T262::Pass
}

/// Dispatch [`run_262`] to the concrete engine, mirroring `harness::run_test`.
fn run_262_on(
    engine: harness::Engine,
    hns: &test262::Harness,
    test_src: &str,
    meta: &test262::Test262Meta,
) -> T262 {
    match engine {
        harness::Engine::Boa => run_262::<script_engine_boa::BoaEngine>(hns, test_src, meta),
        harness::Engine::Nova => run_262::<script_engine_nova::NovaEngine>(hns, test_src, meta),
    }
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
    let Ok(entries) = fs::read_dir(dir) else { return };
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
    let hns = match test262::Harness::load(&t262_root.join("harness")) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("test262 harness load failed ({}): {e}", t262_root.display());
            std::process::exit(2);
        }
    };
    let subset_dir = t262_root.join("test").join(&args.subset);
    if !subset_dir.exists() {
        eprintln!("test262 subset path does not exist: {}", subset_dir.display());
        std::process::exit(2);
    }
    let mut files = Vec::new();
    collect_262(&subset_dir, &mut files);
    let test_root = t262_root.join("test");

    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let (mut both_pass, mut both_fail, mut boa_only, mut nova_only, mut skipped) = (0, 0, 0, 0, 0);
    let mut nova_worklist: Vec<String> = Vec::new();

    for path in &files {
        let Ok(src) = fs::read_to_string(path) else {
            skipped += 1;
            continue;
        };
        let meta = test262::parse_meta(&src);
        let name = path
            .strip_prefix(&test_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        match (
            run_262_on(harness::Engine::Boa, &hns, &src, &meta),
            run_262_on(harness::Engine::Nova, &hns, &src, &meta),
        ) {
            (T262::Skip, _) | (_, T262::Skip) => skipped += 1,
            (T262::Pass, T262::Pass) => both_pass += 1,
            (T262::Fail, T262::Fail) => both_fail += 1,
            (T262::Pass, T262::Fail) => {
                boa_only += 1;
                nova_worklist.push(name.clone());
                if args.verbose {
                    println!("NOVA-GAP  {name}");
                }
            }
            (T262::Fail, T262::Pass) => nova_only += 1,
        }
    }
    panic::set_hook(prev);

    println!(
        "\ntest262 compare [{}]: both-pass={both_pass} both-fail={both_fail} \
         boa-only={boa_only} (Nova gap) nova-only={nova_only} skipped={skipped} (module/async/missing)",
        if args.subset.is_empty() { "<all>" } else { &args.subset },
    );
    if !nova_worklist.is_empty() {
        println!("\nNova worklist (pass on Boa, fail on Nova) — {} test(s):", nova_worklist.len());
        for name in nova_worklist.iter().take(40) {
            println!("  {name}");
        }
        if nova_worklist.len() > 40 {
            println!("  … and {} more", nova_worklist.len() - 40);
        }
    }
}

/// Phase 3: run testharness.js tests and report per-subtest results.
fn testharness(tests: &[PathBuf], args: &Args) {
    let tests_root = Path::new(&args.tests_root);
    let th_path = tests_root.join("resources/testharness.js");
    let testharness_js = match fs::read_to_string(&th_path) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("testharness.js not found at {}", th_path.display());
            std::process::exit(2);
        }
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

    for path in tests {
        // Build the testharness HTML: a real .html document's contents, or a
        // synthesized wrapper for a `.any.js` / `.window.js` test.
        let html = if is_any_js(path) {
            match synthesize_any_js(path) {
                Some(h) => h,
                None => {
                    skipped += 1;
                    if args.verbose {
                        println!("SKIP  worker-only    {}", rel(path, &args.tests_root));
                    }
                    continue;
                }
            }
        } else {
            // Server mode loads the page over HTTP (so `.sub.html` template
            // substitution happens); disk mode reads the file. `server_page` is
            // `None` in disk mode, `Some(None)` if a server fetch failed.
            #[cfg(feature = "netfetch")]
            let server_page =
                server.as_ref().map(|s| net::http_get(&s.doc_url(&rel(path, &args.tests_root))));
            #[cfg(not(feature = "netfetch"))]
            let server_page: Option<Option<String>> = None;
            let raw = match server_page {
                Some(Some(t)) => t,
                Some(None) => {
                    errored += 1;
                    println!("ERROR fetch   {}", rel(path, &args.tests_root));
                    continue;
                }
                None => match fs::read(path) {
                    Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                    Err(_) => {
                        errored += 1;
                        println!("ERROR read    {}", rel(path, &args.tests_root));
                        continue;
                    }
                },
            };
            if classify(path, &raw) != Kind::Testharness {
                skipped += 1;
                if args.verbose {
                    println!("SKIP  non-testharness {}", rel(path, &args.tests_root));
                }
                continue;
            }
            // XHTML is a distinct (XML) parse mode serval's HTML parser doesn't
            // handle; skip rather than report spurious syntax errors.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext.eq_ignore_ascii_case("xhtml") || ext.eq_ignore_ascii_case("xht") {
                skipped += 1;
                if args.verbose {
                    println!("SKIP  xhtml          {}", rel(path, &args.tests_root));
                }
                continue;
            }
            raw
        };

        let base_dir = path.parent().unwrap_or(tests_root);
        let disk = harness::DiskLoader { base_dir, tests_root };
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            // Server mode: a fresh per-test fetch-event channel feeds the drive loop,
            // so deferred fetches settle out of band, mid-flight abort works, and a
            // hung fetch hits the per-test deadline. The shared worker routes replies
            // to this channel; a late reply from a prior test lands on a dropped
            // channel and is harmlessly discarded.
            #[cfg(feature = "netfetch")]
            if let Some(s) = &server {
                let (ev_tx, ev_rx) = std::sync::mpsc::channel::<net::FetchEvent>();
                let doc_url = s.doc_url(&rel(path, &args.tests_root));
                let loader = s.loader(&doc_url);
                let handler = net::NetFetchHandler::new(ev_tx);
                let completion = net::ChannelCompletion::new(ev_rx);
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
            harness::run_test(&testharness_js, &html, &disk, None, None, None, args.engine)
        }));
        let name = rel(path, &args.tests_root);

        match result {
            Err(_) => {
                errored += 1;
                println!("ERROR panic   {name}");
            }
            Ok(harness::HarnessOutcome::Threw(msg)) => {
                errored += 1;
                println!("ERROR {name}  ({msg})");
            }
            Ok(harness::HarnessOutcome::Ran(results)) => {
                let total = results.len();
                let passed = results.iter().filter(|r| r.passed()).count();
                sub_passed += passed;
                sub_total += total;
                if total == 0 {
                    no_results += 1;
                    if args.verbose {
                        println!("NORES {name}  (harness ran but reported no subtests)");
                    }
                } else if passed == total {
                    all_pass += 1;
                    if args.verbose {
                        println!("PASS  {name}  ({passed}/{total})");
                    }
                } else {
                    with_fail += 1;
                    println!("FAIL  {name}  ({passed}/{total} subtests)");
                    if args.verbose {
                        for r in results.iter().filter(|r| !r.passed()) {
                            let msg = r.message.as_deref().unwrap_or("");
                            println!("        [{}] {} {msg}", r.status, r.name);
                        }
                    }
                }
            }
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
        if doc.element_name(id).is_some_and(|q| q.local.as_ref() == "link") {
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
        if doc.element_name(id).is_some_and(|q| q.local.as_ref() == "meta")
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
        let channel_max = pa
            .0
            .iter()
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
        return DiffStats { same_dims: false, differing: total, total, max_channel_diff: 255 };
    }
    let (mut differing, mut max_channel_diff) = (0u64, 0u16);
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let channel_max = pa
            .0
            .iter()
            .zip(pb.0.iter())
            .map(|(x, y)| (i16::from(*x) - i16::from(*y)).unsigned_abs())
            .max()
            .unwrap_or(0);
        if channel_max > 0 {
            differing += 1;
            max_channel_diff = max_channel_diff.max(channel_max);
        }
    }
    DiffStats { same_dims: true, differing, total, max_channel_diff }
}

/// One-line classification of a FAIL's diff shape, for `-v` triage.
fn diff_label(s: &DiffStats) -> &'static str {
    if !s.same_dims {
        "dims"          // different output size — layout/sizing divergence pre-paint
    } else if s.differing == 0 {
        "equal?"        // identical yet failed match — a harness/tolerance quirk
    } else if s.total > 0 && s.differing * 100 / s.total >= 50 {
        "whole"         // >=50% of pixels differ — wholesale (layout / UA stylesheet)
    } else if s.max_channel_diff <= 16 {
        "aa"            // small per-channel diffs — anti-aliasing / sub-pixel
    } else {
        "local"         // localized large diffs — a specific paint/feature gap
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
                let Some(next) = resolve_ref(&ref_path, &next_href, tests_root) else { break };
                let Ok(bytes) = fs::read(&next) else { break };
                ref_path = next;
                html = String::from_utf8_lossy(&bytes).into_owned();
            }
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

fn reftest(tests: &[PathBuf], args: &Args) {
    let renderer = match render::Renderer::boot() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot boot renderer (reftests need a GPU): {e}");
            std::process::exit(1);
        }
    };
    let tests_root = Path::new(&args.tests_root);

    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut passed, mut failed, mut skipped, mut errored) = (0, 0, 0, 0);
    let mut buckets: std::collections::HashMap<&'static str, u64> = std::collections::HashMap::new();
    for path in tests {
        let Ok(bytes) = fs::read(path) else {
            errored += 1;
            continue;
        };
        let test_html = String::from_utf8_lossy(&bytes).into_owned();
        if classify(path, &test_html) != Kind::Reftest {
            skipped += 1;
            continue;
        }
        let Some((kind, href)) = reftest_ref(&test_html) else {
            skipped += 1;
            continue;
        };
        let Some(direct_ref) = resolve_ref(path, &href, tests_root) else {
            skipped += 1;
            continue;
        };
        let Some((ref_path, ref_html)) = final_ref(direct_ref, kind, tests_root) else {
            errored += 1;
            println!("ERROR ref-missing {}", rel(path, &args.tests_root));
            continue;
        };
        if needs_script(&test_html) || needs_script(&ref_html) {
            skipped += 1;
            if args.verbose {
                println!("SKIP  script   {}", rel(path, &args.tests_root));
            }
            continue;
        }

        // Apply the GPU-jitter floor (see FUZZ_FLOOR_*): never compare
        // tighter than it, so a deterministic-to-1/255 render scores stably.
        // A test's explicit <meta fuzzy> widens it where looser.
        let fuzzy = {
            let (d, p) = parse_fuzzy(&test_html).unwrap_or((0, 0));
            Some((d.max(FUZZ_FLOOR_DIFF), p.max(FUZZ_FLOOR_PIXELS)))
        };
        let test_dir = path.parent().unwrap_or(tests_root);
        let ref_dir = ref_path.parent().unwrap_or(tests_root);
        let test_xml = is_xml_path(path);
        let ref_xml = is_xml_path(&ref_path);
        let rendered = panic::catch_unwind(AssertUnwindSafe(|| {
            let t = renderer.render_html(&test_html, test_dir, tests_root, REFTEST_W, REFTEST_H, test_xml);
            let r = renderer.render_html(&ref_html, ref_dir, tests_root, REFTEST_W, REFTEST_H, ref_xml);
            (t, r)
        }));
        let (test_img, ref_img) = match rendered {
            Ok(pair) => pair,
            Err(_) => {
                failed += 1;
                println!("FAIL  crash    {}", rel(path, &args.tests_root));
                continue;
            }
        };

        let matches = images_match(&test_img, &ref_img, fuzzy);
        let pass = match kind {
            MatchKind::Match => matches,
            MatchKind::Mismatch => !matches,
        };
        if pass {
            passed += 1;
            if args.verbose {
                println!("PASS  {}", rel(path, &args.tests_root));
            }
        } else {
            failed += 1;
            let k = if kind == MatchKind::Match { "match   " } else { "mismatch" };
            // Diagnose the diff shape (Lever 2 triage). `match` failures get a
            // bucket from the test-vs-ref pixel diff; `mismatch` failures are
            // "matched when it shouldn't", a different shape, tallied separately.
            if kind == MatchKind::Match {
                let s = diff_stats(&test_img, &ref_img);
                let label = diff_label(&s);
                *buckets.entry(label).or_insert(0) += 1;
                if args.verbose {
                    let pct = if s.total > 0 { s.differing * 100 / s.total } else { 0 };
                    println!(
                        "FAIL  {k} [{label:5}] diff={pct}% maxδ={} {}",
                        s.max_channel_diff,
                        rel(path, &args.tests_root)
                    );
                } else {
                    println!("FAIL  {k} [{label:5}] {}", rel(path, &args.tests_root));
                }
            } else {
                *buckets.entry("mismatch-eq").or_insert(0) += 1;
                println!("FAIL  {k} {}", rel(path, &args.tests_root));
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
        println!("fail buckets: {}", sorted.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join("  "));
        println!("  ({legend})");
    }
    if failed > 0 || errored > 0 {
        std::process::exit(1);
    }
}

/// Render each reftest in the subset + its reference to side-by-side
/// PNGs under `.cargo-check-logs/dump/`, for eyeball diagnosis of a
/// `local`-bucket failure. Writes `<stem>.test.png` / `<stem>.ref.png`.
fn dump(tests: &[PathBuf], args: &Args) {
    let renderer = match render::Renderer::boot() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot boot renderer (needs a GPU): {e}");
            std::process::exit(1);
        }
    };
    let tests_root = Path::new(&args.tests_root);
    let out_dir = Path::new(".cargo-check-logs/dump");
    let _ = fs::create_dir_all(out_dir);
    for path in tests {
        let Ok(bytes) = fs::read(path) else { continue };
        let test_html = String::from_utf8_lossy(&bytes).into_owned();
        if classify(path, &test_html) != Kind::Reftest {
            continue;
        }
        let Some((kind, href)) = reftest_ref(&test_html) else { continue };
        let Some(direct_ref) = resolve_ref(path, &href, tests_root) else { continue };
        let Some((ref_path, ref_html)) = final_ref(direct_ref, kind, tests_root) else { continue };
        let test_dir = path.parent().unwrap_or(tests_root);
        let ref_dir = ref_path.parent().unwrap_or(tests_root);
        let t = renderer.render_html(&test_html, test_dir, tests_root, REFTEST_W, REFTEST_H, is_xml_path(path));
        let r = renderer.render_html(&ref_html, ref_dir, tests_root, REFTEST_W, REFTEST_H, is_xml_path(&ref_path));
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("dump");
        let tp = out_dir.join(format!("{stem}.test.png"));
        let rp = out_dir.join(format!("{stem}.ref.png"));
        let _ = t.save(&tp);
        let _ = r.save(&rp);
        let s = diff_stats(&t, &r);
        let pct = if s.total > 0 { s.differing * 100 / s.total } else { 0 };
        println!("DUMP {} -> {} / {}  (diff={pct}% maxδ={})", rel(path, &args.tests_root), tp.display(), rp.display(), s.max_channel_diff);
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
                    }
                    Ok(Job::Fetch(key, id, req, reply)) => {
                        let (pull_tx, pull_rx) = tokio::sync::mpsc::unbounded_channel::<()>();
                        pulls.insert(id, pull_tx);
                        let h = tokio::spawn(run_fetch_streaming(id, req, reply, pull_rx))
                            .abort_handle();
                        handles.insert(key, h);
                    }
                    Ok(Job::Pull(id)) => {
                        // Grant one chunk of credit; a dead receiver (task finished)
                        // means the entry is stale, so drop it.
                        if let Some(tx) = pulls.get(&id) {
                            if tx.send(()).is_err() {
                                pulls.remove(&id);
                            }
                        }
                    }
                    Ok(Job::Cancel(key)) => {
                        if let Some(h) = handles.remove(&key) {
                            h.abort(); // drop the in-flight future
                        }
                    }
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
        CACHE.get_or_init(|| std::sync::Arc::new(netfetcher::InMemoryHttpCache::new())).clone()
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
        fn cookies_for(
            &self,
            url: &url::Url,
            ctx: netfetcher::SameSiteContext,
        ) -> Vec<String> {
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
        resp.bytes().await.ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The canonical HTTP reason phrase for a status code (netfetcher discards the
    /// wire reason). WPT checks `response.statusText`, so synthesize it.
    fn reason_phrase(status: u16) -> &'static str {
        match status {
            200 => "OK", 201 => "Created", 202 => "Accepted", 203 => "Non-Authoritative Information",
            204 => "No Content", 205 => "Reset Content", 206 => "Partial Content",
            300 => "Multiple Choices", 301 => "Moved Permanently", 302 => "Found", 303 => "See Other",
            304 => "Not Modified", 307 => "Temporary Redirect", 308 => "Permanent Redirect",
            400 => "Bad Request", 401 => "Unauthorized", 402 => "Payment Required", 403 => "Forbidden",
            404 => "Not Found", 405 => "Method Not Allowed", 406 => "Not Acceptable",
            408 => "Request Timeout", 409 => "Conflict", 410 => "Gone", 411 => "Length Required",
            412 => "Precondition Failed", 413 => "Payload Too Large", 414 => "URI Too Long",
            415 => "Unsupported Media Type", 416 => "Range Not Satisfiable", 417 => "Expectation Failed",
            418 => "I'm a Teapot", 421 => "Misdirected Request", 422 => "Unprocessable Entity",
            425 => "Too Early", 426 => "Upgrade Required", 428 => "Precondition Required",
            429 => "Too Many Requests", 431 => "Request Header Fields Too Large",
            451 => "Unavailable For Legal Reasons", 500 => "Internal Server Error",
            501 => "Not Implemented", 502 => "Bad Gateway", 503 => "Service Unavailable",
            504 => "Gateway Timeout", 505 => "HTTP Version Not Supported", _ => "",
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
            }
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
            url: resp.url_list.last().map(|u| u.to_string()).unwrap_or_default(),
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
                }
                Some(Err(_)) => {
                    // Body decode error (e.g. a bad Content-Encoding): error the
                    // body stream so reads reject, rather than closing it cleanly.
                    let _ = reply.send(FetchEvent::Error(id));
                    return;
                }
                None => {
                    let _ = reply.send(FetchEvent::Close(id));
                    return;
                }
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
            Self { reply, keys: std::cell::RefCell::new(std::collections::HashMap::new()) }
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
                }
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
                return Err(format!("no WPT server reachable at {origin} (is `wpt serve` up?)"));
            }
            Ok(Self { origin, _spawned: None })
        }

        /// Spawn `python wpt serve` under `tests_root`, discover its plain-http
        /// origin, and wait until it answers. Torn down when the returned ctx drops.
        pub fn spawn(tests_root: &Path) -> Result<Self, String> {
            let handle = ServerHandle::spawn(tests_root)?;
            let origin = handle.origin.clone();
            Ok(Self { origin, _spawned: Some(handle) })
        }

        /// The document URL for a test, given its path relative to the tests root.
        pub fn doc_url(&self, test_rel: &str) -> String {
            format!("{}/{}", self.origin, test_rel.trim_start_matches('/'))
        }

        pub fn loader(&self, doc_url: &str) -> ServerLoader {
            ServerLoader { doc_url: doc_url.to_owned() }
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
                    }
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
                parse_http_port("[2026-06-02 21:48:27,647 http on port 8000] INFO - Starting http server on http://web-platform.test:8000"),
                Some(8000)
            );
            // The variant servers must not match (their tag is not ` http on port `).
            assert_eq!(parse_http_port("[ts http-local on port 62276] INFO - ..."), None);
            assert_eq!(parse_http_port("[ts http-public on port 62277] INFO - ..."), None);
            assert_eq!(parse_http_port("[ts h2 on port 9000] INFO - ..."), None);
            assert_eq!(parse_http_port("[ts ws on port 62280] INFO - ..."), None);
            // Noise lines.
            assert_eq!(parse_http_port("INFO:root:Status of subprocess ..."), None);
        }

        #[test]
        fn doc_url_joins_origin_and_test_path() {
            let ctx = ServerCtx { origin: "http://web-platform.test:8000".into(), _spawned: None };
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
