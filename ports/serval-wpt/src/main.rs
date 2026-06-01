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
use serval_static_dom::StaticDocument;

mod harness;
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
    serval-wpt list        <subset>   enumerate + classify tests in a subset
    serval-wpt run         <subset>   crash-smoke a subset (parse + layout)
    serval-wpt reftest     <subset>   render + pixel-compare reftests (needs a GPU)
    serval-wpt testharness <subset>   run testharness.js tests + collect results (Boa)

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

    // bench needs only the tests root (for resources/testharness.js), not a subset
    // walk; handle it before the corpus collection below.
    if args.command == "bench" {
        harness::bench(&args.tests_root);
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

    // Boa / the bridge can panic on unimplemented paths; report, don't spam.
    let prev = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));

    let (mut all_pass, mut with_fail, mut errored, mut no_results, mut skipped) = (0, 0, 0, 0, 0);
    let (mut sub_passed, mut sub_total) = (0usize, 0usize);

    for path in tests {
        let html = match fs::read(path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(_) => {
                errored += 1;
                println!("ERROR read    {}", rel(path, &args.tests_root));
                continue;
            }
        };
        if classify(path, &html) != Kind::Testharness {
            skipped += 1;
            if args.verbose {
                println!("SKIP  non-testharness {}", rel(path, &args.tests_root));
            }
            continue;
        }
        // XHTML is a distinct (XML) parse mode serval's HTML parser doesn't handle;
        // skip rather than report spurious syntax errors.
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("xhtml") || ext.eq_ignore_ascii_case("xht") {
            skipped += 1;
            if args.verbose {
                println!("SKIP  xhtml          {}", rel(path, &args.tests_root));
            }
            continue;
        }

        let base_dir = path.parent().unwrap_or(tests_root);
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            harness::run_test(&testharness_js, &html, base_dir, tests_root)
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
        "\ntestharness: {all_pass} all-pass, {with_fail} with-failures, {errored} errored, \
         {no_results} no-results, {skipped} skipped (of {} files); \
         subtests {sub_passed}/{sub_total} passed",
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

        let fuzzy = parse_fuzzy(&test_html);
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
