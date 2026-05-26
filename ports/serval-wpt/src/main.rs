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

use layout_dom_api::{LayoutDom, LocalName, NodeKind};
use serval_static_dom::StaticDocument;

mod render;

// The upstream WPT checkout lives under `tests/wpt/tests/`
// (`tests/wpt/mozilla/` holds servo-specific tests).
const DEFAULT_TESTS_ROOT: &str = "tests/wpt/tests";
const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: f32 = 600.0;
// Reftest render size (the WPT default viewport).
const REFTEST_W: u32 = 800;
const REFTEST_H: u32 = 600;

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

/// Extract the text of every `<style>` block from raw HTML. Crude (ignores
/// attributes and nesting), but enough to exercise the cascade with the
/// page's own rules during the smoke. Robust to parse failures since it
/// works on the source string.
fn extract_inline_styles(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(open) = lower[from..].find("<style") {
        let open = from + open;
        let Some(gt) = lower[open..].find('>') else { break };
        let content_start = open + gt + 1;
        let Some(close_rel) = lower[content_start..].find("</style>") else { break };
        let close = content_start + close_rel;
        out.push(html[content_start..close].to_string());
        from = close + "</style>".len();
    }
    out
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

    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        let document = StaticDocument::parse(&html);
        let sheets = extract_inline_styles(&html);
        let sheet_refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
        let _fragments = serval_layout::render(&document, &sheet_refs, VIEWPORT_W, VIEWPORT_H);
    }));

    (kind, if result.is_ok() { Outcome::Passed } else { Outcome::Failed })
}

/// Collect HTML test files under `root` (a dir or a single file).
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        if is_html(root) {
            out.push(root.to_path_buf());
        }
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if is_html(&path) {
            out.push(path);
        }
    }
}

struct Args {
    command: String,
    subset: String,
    tests_root: String,
    verbose: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut command = None;
    let mut subset = None;
    let mut tests_root = DEFAULT_TESTS_ROOT.to_string();
    let mut verbose = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tests-root" => {
                tests_root = it.next().ok_or("--tests-root needs a value")?;
            }
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
    })
}

fn usage() -> String {
    "\
serval-wpt - serval-native web-platform-tests runner (phase 1: crash-smoke)

Usage:
    serval-wpt list    <subset>       enumerate + classify tests in a subset
    serval-wpt run     <subset>       crash-smoke a subset (parse + layout)
    serval-wpt reftest <subset>       render + pixel-compare reftests (needs a GPU)

Options:
    --tests-root <dir>   tests root (default: tests/wpt)
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
        other => {
            eprintln!("unknown command: {other}\n{}", usage());
            std::process::exit(2);
        }
    }
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Match,
    Mismatch,
}

/// The first `<link rel="match"|"mismatch" href="...">` in a reftest.
fn reftest_ref(html: &str) -> Option<(MatchKind, String)> {
    let doc = StaticDocument::parse(html);
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

/// Slice-1 conservatism: skip reftests needing resources we do not yet
/// load (linked stylesheets, scripts). Inline `<style>` and data-URI
/// images are handled.
fn needs_external_resources(html: &str) -> bool {
    let l = html.to_ascii_lowercase();
    l.contains("rel=\"stylesheet\"") || l.contains("rel=stylesheet") || l.contains("<script")
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
        let Some(ref_path) = resolve_ref(path, &href, tests_root) else {
            skipped += 1;
            continue;
        };
        let Ok(ref_bytes) = fs::read(&ref_path) else {
            errored += 1;
            println!("ERROR ref-missing {}", rel(path, &args.tests_root));
            continue;
        };
        let ref_html = String::from_utf8_lossy(&ref_bytes).into_owned();
        if needs_external_resources(&test_html) || needs_external_resources(&ref_html) {
            skipped += 1;
            if args.verbose {
                println!("SKIP  resources {}", rel(path, &args.tests_root));
            }
            continue;
        }

        let test_sheets = extract_inline_styles(&test_html);
        let ref_sheets = extract_inline_styles(&ref_html);
        let rendered = panic::catch_unwind(AssertUnwindSafe(|| {
            let t = renderer.render_html(
                &test_html,
                &test_sheets.iter().map(String::as_str).collect::<Vec<_>>(),
                REFTEST_W,
                REFTEST_H,
            );
            let r = renderer.render_html(
                &ref_html,
                &ref_sheets.iter().map(String::as_str).collect::<Vec<_>>(),
                REFTEST_W,
                REFTEST_H,
            );
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

        let equal = images_equal(&test_img, &ref_img);
        let pass = match kind {
            MatchKind::Match => equal,
            MatchKind::Mismatch => !equal,
        };
        if pass {
            passed += 1;
            if args.verbose {
                println!("PASS  {}", rel(path, &args.tests_root));
            }
        } else {
            failed += 1;
            let k = if kind == MatchKind::Match { "match   " } else { "mismatch" };
            println!("FAIL  {k} {}", rel(path, &args.tests_root));
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
    if failed > 0 || errored > 0 {
        std::process::exit(1);
    }
}
