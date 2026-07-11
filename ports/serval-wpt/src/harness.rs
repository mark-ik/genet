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
use std::time::{Duration, Instant};

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace};
use script_engine_api::ScriptEngine;
use script_runtime_api::{
    ComputedStyleHandler, FetchHandler, FetchOutcome, MediaQueryHandler, Runtime, TestResult,
    WebGlFactory,
};
use serval_layout::{Applied, IncrementalLayout, inline_stylesheets};
use serval_scripted_dom::NodeId as DomNodeId;
use serval_static_dom::StaticDocument;

thread_local! {
    /// One media-query evaluator per thread (the WPT 800x600 viewport, default
    /// media environment), shared across tests so `matchMedia` works without a
    /// per-test stylist build.
    static MEDIA_QUERY_EVAL: std::rc::Rc<serval_layout::MediaQueryEvaluator> =
        std::rc::Rc::new(serval_layout::MediaQueryEvaluator::new(800.0, 600.0));
}

/// `matchMedia` seam for the WPT runner: evaluates against a default device.
struct WptMediaQueries(std::rc::Rc<serval_layout::MediaQueryEvaluator>);
impl MediaQueryHandler for WptMediaQueries {
    fn evaluate(&self, query: &str) -> (String, bool) {
        self.0.evaluate(query)
    }
}

/// A deferred `fetch()` completion, applied to the runtime by the drive loop. A
/// response streams as `StartStream` (status + headers) -> `Chunk`* (body) ->
/// `Close`, or `Error` if the body fails partway (e.g. a `Content-Encoding`
/// decode error: the response resolved, but body reads reject). `Fail` is a
/// network error before the headers.
pub enum FetchCompletion {
    StartStream(u64, FetchOutcome),
    Chunk(u64, Vec<u8>),
    Close(u64),
    Error(u64),
    Fail(u64, String),
}

/// A source of deferred fetch completions (the netfetch worker's channel). The
/// drive loop pulls completions and applies each via the callback. Disk / sync
/// runs pass `None` and never touch this.
pub trait CompletionSource {
    /// Apply every currently-ready completion; return how many were applied.
    fn drain(&self, apply: &mut dyn FnMut(FetchCompletion)) -> usize;
    /// Block up to `timeout` for one completion, then apply it; return 0 or 1.
    fn wait(&self, timeout: Duration, apply: &mut dyn FnMut(FetchCompletion)) -> usize;
}

/// Per-test wall-clock ceiling for the deferred drive loop: a test that awaits a
/// never-settling fetch fails (TIMEOUT) instead of hanging the runner.
const DRIVE_DEADLINE: Duration = Duration::from_secs(15);
/// Timers fired per drive turn before re-checking the completion channel.
const TIMER_BUDGET: u32 = 64;
/// The drive loop's rendering cadence (one 60Hz frame, ms). In disk mode this is
/// a *virtual* step — rAF callbacks and the animation clock advance by it with no
/// sleeping, so a 2s animation costs evaluation time, not wall time. In server
/// mode it caps the idle wait while frames are pending.
const FRAME_MS: f64 = 1000.0 / 60.0;

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
    completion: Option<&dyn CompletionSource>,
    engine: Engine,
) -> HarnessOutcome {
    run_test_with_webgl(
        testharness_js,
        html,
        loader,
        base_url,
        handler,
        completion,
        None,
        engine,
    )
}

/// Like [`run_test`] but also installs a WebGL context factory, so a test that
/// calls `canvas.getContext('webgl')` (e.g. the Khronos conformance suite via
/// `webgl-test-utils.js`) draws against a real backend. The factory is minted
/// per `getContext`; pass `None` for the graphics-free default.
#[allow(clippy::too_many_arguments)]
pub fn run_test_with_webgl(
    testharness_js: &str,
    html: &str,
    loader: &dyn ScriptSrcLoader,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    webgl: Option<WebGlFactory>,
    engine: Engine,
) -> HarnessOutcome {
    let doc = StaticDocument::parse(html);
    let mut scripts = Vec::new();
    collect_scripts(&doc, doc.document(), loader, &mut scripts);
    let test_src = scripts.join("\n;\n");

    match engine {
        Engine::Boa => run_with::<script_engine_boa::BoaEngine>(
            testharness_js,
            &test_src,
            &doc,
            base_url,
            handler,
            completion,
            webgl,
        ),
        Engine::Nova => run_with::<script_engine_nova::NovaEngine>(
            testharness_js,
            &test_src,
            &doc,
            base_url,
            handler,
            completion,
            webgl,
        ),
    }
}

/// A Nova runtime snapshotted after the host surface and `testharness.js` are
/// loaded. Each test clones this template, installs fresh host state, loads that
/// test's DOM, and runs only the test body.
pub struct NovaHarnessTemplate {
    rt: Runtime<script_engine_nova::NovaEngine>,
}

impl NovaHarnessTemplate {
    pub fn new(testharness_js: &str) -> Result<Self, String> {
        let mut rt = Runtime::<script_engine_nova::NovaEngine>::new()
            .map_err(|e| format!("runtime init: {e:?}"))?;
        rt.load_testharness(testharness_js)
            .map_err(|e| format!("testharness load: {e:?}"))?;
        Ok(Self { rt })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_test_with_webgl(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
        webgl: Option<WebGlFactory>,
    ) -> HarnessOutcome {
        let doc = StaticDocument::parse(html);
        let mut scripts = Vec::new();
        collect_scripts(&doc, doc.document(), loader, &mut scripts);
        let test_src = scripts.join("\n;\n");
        let mut rt = match self.rt.snapshot_clone() {
            Ok(rt) => rt,
            Err(e) => return HarnessOutcome::Threw(format!("runtime snapshot clone: {e:?}")),
        };
        prepare_runtime(&mut rt, &doc, base_url, handler, webgl);
        run_loaded_with(&mut rt, &test_src, completion)
    }

    pub fn run_test(
        &mut self,
        html: &str,
        loader: &dyn ScriptSrcLoader,
        base_url: Option<&str>,
        handler: Option<Box<dyn FetchHandler>>,
        completion: Option<&dyn CompletionSource>,
    ) -> HarnessOutcome {
        self.run_test_with_webgl(html, loader, base_url, handler, completion, None)
    }
}

/// Engine-generic core: build a `Runtime<E>`, load the test's body as the live
/// DOM, set the base URL + fetch handler if given, run the harness, collect
/// results. With a `completion` source (deferred / server mode) it drives the
/// event loop and the fetch-completion channel to quiescence (or a deadline)
/// itself, because deferred replies arrive out of band; without one it uses the
/// synchronous one-shot path.
#[allow(clippy::too_many_arguments)]
fn run_with<E: ScriptEngine>(
    testharness_js: &str,
    test_src: &str,
    doc: &StaticDocument,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    completion: Option<&dyn CompletionSource>,
    webgl: Option<WebGlFactory>,
) -> HarnessOutcome {
    let mut rt = match Runtime::<E>::new() {
        Ok(rt) => rt,
        Err(e) => return HarnessOutcome::Threw(format!("runtime init: {e:?}")),
    };
    prepare_runtime(&mut rt, doc, base_url, handler, webgl);
    if let Err(e) = rt.load_testharness(testharness_js) {
        return HarnessOutcome::Threw(format!("testharness load: {e:?}"));
    }
    run_loaded_with(&mut rt, test_src, completion)
}

fn prepare_runtime<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    doc: &StaticDocument,
    base_url: Option<&str>,
    handler: Option<Box<dyn FetchHandler>>,
    webgl: Option<WebGlFactory>,
) {
    // The test's body becomes the live DOM, so scripts querying body elements
    // (getElementById / querySelector / document.body) see them.
    rt.load_dom(doc);
    if let Some(base) = base_url {
        let _ = rt.set_base_url(base);
    }
    if let Some(h) = handler {
        rt.set_fetch_handler(h);
    }
    // `window.matchMedia` over a default device, so css/mediaqueries tests can
    // parse + evaluate queries (they never render).
    rt.set_media_query_handler(Box::new(WptMediaQueries(
        MEDIA_QUERY_EVAL.with(|e| e.clone()),
    )));
    if let Some(factory) = webgl {
        rt.set_webgl_factory(factory);
    }
}

/// H7a (harness-exactness plan): the testharness lane's rendering session.
///
/// An [`IncrementalLayout`] over the runtime's live DOM, doing per drive turn
/// what a host's frame loop does: apply pending DOM mutations, pump rAF
/// callbacks, tick CSS animations/transitions on the drive clock, and dispatch
/// the harvested lifecycle events through the runtime. It also backs
/// `getComputedStyle` (the serval-scripted `ComputedStyleBridge` pattern), so
/// style reads return the session's cascade instead of nothing. Cheap when a
/// test never animates: every hook no-ops on an empty set.
struct RenderSession {
    layout: Rc<RefCell<Option<IncrementalLayout<DomNodeId>>>>,
    sheets: Vec<String>,
}

/// `getComputedStyle` over the session's retained cascade. One restyle stale by
/// construction (script runs before the next turn's apply), the standard
/// tradeoff of the script-before-layout split.
struct WptComputedStyle {
    layout: Rc<RefCell<Option<IncrementalLayout<DomNodeId>>>>,
}

impl ComputedStyleHandler for WptComputedStyle {
    fn computed_value(&self, node: u64, property: &str) -> Option<String> {
        self.layout
            .borrow()
            .as_ref()?
            .computed_value(DomNodeId::from_raw(node as usize), property)
    }
}

impl RenderSession {
    /// Build the session over the runtime's already-loaded DOM (before the test
    /// body runs, so early style reads see the parse-time cascade) and register
    /// the `getComputedStyle` bridge.
    fn new<E: ScriptEngine>(rt: &mut Runtime<E>) -> Self {
        let layout = Rc::new(RefCell::new(None));
        let sheets;
        {
            let mut host = rt.host().borrow_mut();
            sheets = inline_stylesheets(&host.dom);
            // Discard the load-time mutation backlog: `new` cascades the DOM's
            // current state, so replaying it through `apply` would be double work.
            let mut discard = Vec::new();
            host.dom.drain_mutations(&mut discard);
            let refs: Vec<&str> = sheets.iter().map(String::as_str).collect();
            *layout.borrow_mut() = Some(IncrementalLayout::new(&host.dom, &refs, 800.0, 600.0));
        }
        rt.set_computed_style_handler(Box::new(WptComputedStyle {
            layout: layout.clone(),
        }));
        Self { layout, sheets }
    }

    /// Whether a declared CSS animation/transition is still live on the session
    /// clock — the drive loop keeps producing frames while true.
    fn animating(&self) -> bool {
        self.layout
            .borrow()
            .as_ref()
            .is_some_and(|l| l.has_active_animations())
    }

    /// One rendering turn at `now_ms`: rAF callbacks, mutation apply, animation
    /// tick, then transition + animation event dispatch. Returns how much work
    /// happened (0 = the turn was a no-op), which feeds the quiescence check.
    fn turn<E: ScriptEngine>(&self, rt: &mut Runtime<E>, now_ms: f64) -> usize {
        let mut work = rt.run_animation_frame_callbacks(now_ms).unwrap_or(0);
        let (t_events, a_events, restyled) = {
            let mut host = rt.host().borrow_mut();
            let mut layout = self.layout.borrow_mut();
            let Some(layout) = layout.as_mut() else {
                return work;
            };
            let refs: Vec<&str> = self.sheets.iter().map(String::as_str).collect();
            let mut muts = Vec::new();
            host.dom.drain_mutations(&mut muts);
            let mut restyled = 0usize;
            if !muts.is_empty() {
                layout.apply(&host.dom, &refs, &muts);
                restyled += 1;
            }
            if layout.tick_animations(&host.dom, now_ms / 1000.0) != Applied::Unchanged {
                restyled += 1;
            }
            (
                layout.take_transition_events(&host.dom),
                layout.take_animation_events(&host.dom),
                restyled,
            )
        };
        // Dispatch off the borrow: listeners can mutate the DOM; the next turn
        // picks those mutations up.
        work += restyled + t_events.len() + a_events.len();
        for ev in t_events {
            let _ = rt.dispatch_transition_event(
                ev.node.raw(),
                ev.kind.event_type(),
                &ev.property_name,
                ev.elapsed_time,
            );
        }
        for ev in a_events {
            let _ = rt.dispatch_animation_event(
                ev.node.raw(),
                ev.kind.event_type(),
                &ev.animation_name,
                ev.elapsed_time,
            );
        }
        work
    }
}

fn run_loaded_with<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    test_src: &str,
    completion: Option<&dyn CompletionSource>,
) -> HarnessOutcome {
    // H7a: every testharness run gets a rendering session. Built before the test
    // body runs so early `getComputedStyle` reads see the parse-time cascade.
    let render = RenderSession::new(rt);
    if let Err(e) = rt.begin_loaded_testharness(test_src) {
        return HarnessOutcome::Threw(truncate(&format!("{e:?}"), 200));
    }
    rt.run_microtasks();
    match completion {
        None => drive_virtual(rt, &render),
        Some(cs) => drive_wall(rt, &render, cs),
    }
}

/// Disk-mode drive: all three clocks (timers, rAF, the animation clock) are
/// **virtual** — each turn advances `now_ms` to the next frame while anything
/// frame-paced pends, else jumps straight to the next timer's due time, and
/// never sleeps. Timer firing order matches the old eager path (due-time order),
/// so pure-timer tests score identically; what's new is that rAF-paced and
/// animating tests make progress at evaluation speed rather than never. The
/// wall-clock deadline still backstops runaway loops (a self-rearming 0ms timer,
/// an rAF loop that never exits).
fn drive_virtual<E: ScriptEngine>(rt: &mut Runtime<E>, render: &RenderSession) -> HarnessOutcome {
    let start = Instant::now();
    let mut now_ms = 0.0f64;
    loop {
        if start.elapsed() >= DRIVE_DEADLINE {
            rt.fail_all_pending("test timed out");
            break;
        }
        rt.run_microtasks();
        let fired = rt.run_timers(TIMER_BUDGET, now_ms);
        let rendered = render.turn(rt, now_ms);
        let has_raf = rt.has_animation_frame_callbacks();
        let animating = render.animating();
        let next_timer = rt.next_timer_delay();
        if fired == 0 && rendered == 0 && !has_raf && !animating && next_timer.is_none() {
            break; // quiescent: no timer, no frame consumer, nothing happened
        }
        // Advance the virtual clock: to the sooner of the next frame and the next
        // timer while frames are wanted, else straight to the next timer. A turn
        // that did work but scheduled nothing time-based loops once more at the
        // same instant and then quiesces.
        now_ms += if has_raf || animating {
            match next_timer {
                Some(d) if d < FRAME_MS => d.max(0.0),
                _ => FRAME_MS,
            }
        } else if let Some(d) = next_timer {
            d.max(0.0)
        } else {
            0.0
        };
    }
    HarnessOutcome::Ran(rt.results())
}

/// Server-mode drive: timers fire on a real-time gate (virtual clock = elapsed
/// wall ms), so a short abort timer fires at its delay while the far-future
/// testharness timeout stays pending, and fetch completions arrive out of band
/// on the channel. Each turn additionally runs a rendering turn; idle waits are
/// capped at one frame while rAF callbacks or animations pend.
fn drive_wall<E: ScriptEngine>(
    rt: &mut Runtime<E>,
    render: &RenderSession,
    cs: &dyn CompletionSource,
) -> HarnessOutcome {
    let start = Instant::now();
    let elapsed_ms =
        |start: Instant| (Instant::now().saturating_duration_since(start)).as_millis() as f64;
    loop {
        if elapsed_ms(start) >= DRIVE_DEADLINE.as_millis() as f64 {
            rt.fail_all_pending("test timed out");
            break;
        }
        rt.run_microtasks();
        let applied = cs.drain(&mut |c| match c {
            FetchCompletion::StartStream(id, o) => rt.start_stream(id, o),
            FetchCompletion::Chunk(id, b) => rt.push_chunk(id, &b),
            FetchCompletion::Close(id) => rt.close_stream(id),
            FetchCompletion::Error(id) => rt.error_stream(id),
            FetchCompletion::Fail(id, m) => rt.fail_fetch(id, &m),
        });
        let fired = rt.run_timers(TIMER_BUDGET, elapsed_ms(start));
        let rendered = render.turn(rt, elapsed_ms(start));
        let has_raf = rt.has_animation_frame_callbacks();
        let animating = render.animating();
        let pending = rt.pending_fetches();
        let next_timer = rt.next_timer_delay();
        if pending == 0
            && next_timer.is_none()
            && fired == 0
            && applied == 0
            && rendered == 0
            && !has_raf
            && !animating
        {
            break; // quiescent: no fetch, no timer, no frame consumer
        }
        if fired == 0 && applied == 0 && rendered == 0 {
            // Nothing ran this turn. Sleep until the next event: a fetch
            // completion, the next timer's due time, or — while frames are
            // wanted — at most one frame.
            let remaining = (DRIVE_DEADLINE.as_millis() as f64 - elapsed_ms(start)).max(0.0);
            let mut wait_ms = remaining;
            if let Some(d) = next_timer {
                wait_ms = wait_ms.min(d);
            }
            if has_raf || animating {
                wait_ms = wait_ms.min(FRAME_MS);
            }
            if pending > 0 {
                // Wake on a completion or after the slice (whichever first).
                cs.wait(
                    Duration::from_millis(wait_ms.ceil() as u64),
                    &mut |c| match c {
                        FetchCompletion::StartStream(id, o) => rt.start_stream(id, o),
                        FetchCompletion::Chunk(id, b) => rt.push_chunk(id, &b),
                        FetchCompletion::Error(id) => rt.error_stream(id),
                        FetchCompletion::Close(id) => rt.close_stream(id),
                        FetchCompletion::Fail(id, m) => rt.fail_fetch(id, &m),
                    },
                );
            } else if wait_ms > 0.0 {
                // Only timers/frames are outstanding: sleep until the soonest.
                std::thread::sleep(Duration::from_millis(wait_ms.ceil() as u64));
            }
        }
    }
    HarnessOutcome::Ran(rt.results())
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
    if dom
        .element_name(node)
        .is_some_and(|q| q.local.as_ref() == "script")
    {
        match dom.attribute(node, &Namespace::default(), &LocalName::from("src")) {
            Some(src) if !is_harness_src(src) => {
                if let Some(text) = loader.load_script(src) {
                    out.push(text);
                }
            },
            Some(_) => {}, // the harness / report hook: the host surface supplies these
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
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyLoader;

    impl ScriptSrcLoader for EmptyLoader {
        fn load_script(&self, _src: &str) -> Option<String> {
            None
        }
    }

    const MINI_TESTHARNESS: &str = r#"
var __tests = [];
var __completion_callbacks = [];
function setup() {}
function add_completion_callback(cb) { __completion_callbacks.push(cb); }
function assert_true(value, message) {
  if (!value) throw new Error(message || "assert_true failed");
}
function test(fn, name) {
  try {
    fn();
    __tests.push({ name: name, status: 0, message: null });
  } catch (e) {
    __tests.push({ name: name, status: 1, message: String((e && e.message) || e) });
  }
}
window.addEventListener("load", function() {
  var snapshot = __tests.slice();
  for (var i = 0; i < __completion_callbacks.length; i++) {
    __completion_callbacks[i](snapshot);
  }
});
"#;

    /// End to end through the H7a rendering session: the real WPT
    /// `animationevent-types.html` (negative delay, iteration-count 2, animates
    /// `left`) runs to completion without panicking. This test found the stylo
    /// f32 boundary hole (fork fix `56e70cacdb`) — the harness's silent panic
    /// hook had reduced it to an opaque `ERROR panic` in the corpus run, so it
    /// stays here where a panic gets a backtrace.
    #[test]
    fn animationevent_types_survives_the_rendering_session() {
        let wpt = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt");
        let testharness_js =
            fs::read_to_string(wpt.join("tests/resources/testharness.js")).expect("harness");
        let test_path = wpt.join("tests/css/css-animations/animationevent-types.html");
        let html = fs::read_to_string(&test_path).expect("test html");
        let base_dir = test_path.parent().unwrap().to_path_buf();
        let tests_root = wpt.join("tests");
        let loader = DiskLoader {
            base_dir: base_dir.as_path(),
            tests_root: tests_root.as_path(),
        };
        let outcome = run_test(
            &testharness_js,
            &html,
            &loader,
            None,
            None,
            None,
            Engine::Boa,
        );
        match outcome {
            // Subtest passes/failures are the corpus baseline's business; this
            // guard only pins "the session does not take the process down".
            HarnessOutcome::Ran(results) => {
                assert!(!results.is_empty(), "the animation events should reach testharness");
            },
            HarnessOutcome::Threw(m) => panic!("threw instead of reporting: {m}"),
        }
    }

    fn unwrap_ran(outcome: HarnessOutcome) -> Vec<TestResult> {
        match outcome {
            HarnessOutcome::Ran(results) => results,
            HarnessOutcome::Threw(message) => panic!("harness threw: {message}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn nova_harness_template_reuses_loaded_harness_without_result_leak() {
        let mut template = NovaHarnessTemplate::new(MINI_TESTHARNESS).expect("template");
        let loader = EmptyLoader;

        let first = unwrap_ran(template.run_test(
            r#"<script>test(function(){ assert_true(true); }, "first");</script>"#,
            &loader,
            Some("file:///first.html"),
            None,
            None,
        ));
        let second = unwrap_ran(template.run_test(
            r#"<script>test(function(){ assert_true(true); }, "second");</script>"#,
            &loader,
            Some("file:///second.html"),
            None,
            None,
        ));

        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!(first[0].name, "first");
        assert!(first[0].passed(), "{first:?}");
        assert_eq!(second.len(), 1, "{second:?}");
        assert_eq!(second[0].name, "second");
        assert!(second[0].passed(), "{second:?}");
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
        },
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
    let loader = DiskLoader {
        base_dir: root,
        tests_root: root,
    };
    let t = Instant::now();
    for _ in 0..n {
        let _ = run_test(
            &testharness_js,
            html,
            &loader,
            None,
            None,
            None,
            Engine::Boa,
        );
    }
    let run_ms = t.elapsed().as_secs_f64() * 1000.0 / n as f64;

    // (d) Isolation probe: can one Runtime run two harness evals back-to-back
    // without the `tests` singleton leaking results across them? If a re-eval
    // resets cleanly, a pooled-Runtime (re-eval harness per test) is safe.
    let mut rt = Runtime::<BoaEngine>::new().expect("new");
    let r1 = rt.run_testharness(
        &testharness_js,
        "test(function(){ assert_true(true); }, 'a');",
    );
    let r2 = rt.run_testharness(
        &testharness_js,
        "test(function(){ assert_true(true); }, 'b');",
    );
    let leak = match (&r1, &r2) {
        (Ok(a), Ok(b)) => format!(
            "run1={} subtests, run2={} subtests (want 1 and 1; >1 = leak)",
            a.len(),
            b.len()
        ),
        _ => "a run errored".to_string(),
    };

    println!("bench (Boa, {n} iters, ms/iter):");
    println!("  (a) Runtime::new()                  {new_ms:8.2}");
    println!(
        "  (b) new() + eval(testharness.js)    {new_harness_ms:8.2}  (harness eval = {:.2})",
        new_harness_ms - new_ms
    );
    println!("  (c) full run_test (trivial test)    {run_ms:8.2}");
    println!("  (d) reuse isolation: {leak}");
    println!(
        "\nFinding: the dominant per-test cost is the harness eval (~{:.0} ms), not\n\
         Runtime::new() (~{:.0} ms). Reusing a Runtime across tests LEAKS — testharness's\n\
         `tests` singleton accumulates across re-evals (see (d)) — so realm-reuse is\n\
         incorrect without a reset. Correct amortization needs a post-(harness-eval)\n\
         snapshot cloned per test (a fresh `tests` each time): the GcAgent::clone path.",
        new_harness_ms - new_ms,
        new_ms,
    );
}
