// Copyright 2026 the genet-probe authors.
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The shared scenario driver: a generic verb loop over an [`Automatable`] app.
//!
//! merecat proved the shape — a text scenario, pumped one step per rendered
//! frame, asserting against a typed observation and an event stream. This is
//! that loop, lifted off the app: it parses a small generic grammar, and each
//! [`tick`](Scenario::tick) drives the app through the [`Automatable`] trait and
//! the [`Driveable`] hooks. The frame pump stays the app's (it owns winit); the
//! app calls `tick` after each frame and `finish` at the end.
//!
//! Two things the generic loop cannot do are the app's: taking a screenshot, and
//! any verb specific to the app's own state (`assert pane roster`). Those go
//! behind [`Driveable`]. Everything else — `act`, `click` by selector, `settle`,
//! `assert text` / `event` / `snap`, `log`, `capture` — is shared.
//!
//! A `click` miss attributes itself into the event stream as
//! `interaction-missed <selector>`, so a scenario that drives a miss can assert
//! it rather than have it vanish — the "loud AND attributable" rule, generalized
//! off merecat into the loop every app inherits.

use crate::{Automatable, AutomatableExt, Match, Selector, text_present};

/// How an `assert snap` value compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Eq,
    Ge,
    Le,
    /// Substring containment (`~`).
    Contains,
}

/// One parsed step. Unrecognized verbs become [`Step::App`] for the host.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Act(String),
    Click(Selector),
    Settle(u32),
    Log(String),
    Capture(String),
    AssertText(String),
    AssertEvent(String),
    AssertSnap { field: String, cmp: Cmp, value: String },
    App(String),
}

/// The app-side surface a scenario drives: [`Automatable`] plus the two
/// fulfillments the generic loop cannot do itself — a screenshot, and any
/// app-specific verb the generic grammar did not recognize.
pub trait Driveable: Automatable {
    /// Fulfill a `capture <name>`. Default: a no-op success (headless runs that
    /// do not screenshot). Return `false` to fail the step.
    fn capture(&mut self, name: &str) -> bool {
        let _ = name;
        true
    }

    /// Handle an app-specific verb line the generic grammar passed through.
    /// `Ok(())` passes; `Err(msg)` fails the scenario with `msg`. Default: an
    /// unknown verb is a failure, so a typo is loud rather than silently skipped.
    fn app_step(&mut self, line: &str) -> Result<(), String> {
        Err(format!("unknown verb: {line}"))
    }
}

/// What one [`tick`](Scenario::tick) reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// More steps remain (or a settle is counting down).
    Running,
    /// Every step ran; call [`finish`](Scenario::finish).
    Done,
}

/// The end result: whether every assertion held, and the human log.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub log: Vec<String>,
}

/// A parsed scenario, pumped one step per frame against a [`Driveable`] app.
pub struct Scenario {
    steps: Vec<Step>,
    idx: usize,
    settle: u32,
    failed: bool,
    log: Vec<String>,
    /// The app's semantic events, accumulated across frames (drained each tick);
    /// `assert event` matches substrings against these describe-strings, plus the
    /// loop's own `interaction-missed` attributions.
    events: Vec<String>,
}

impl Scenario {
    /// Parse a scenario. Blank lines and `#` comments are skipped. Each other
    /// line is one verb; an unrecognized verb parses to [`Step::App`] and is
    /// handled by [`Driveable::app_step`] at run time (so an app's own verbs pass
    /// through this parser untouched).
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            steps.push(parse_step(line).map_err(|e| format!("line {}: {e}", n + 1))?);
        }
        Ok(Self {
            steps,
            idx: 0,
            settle: 0,
            failed: false,
            log: Vec::new(),
            events: Vec::new(),
        })
    }

    /// Advance one step against `app`, after a rendered frame. Drains the app's
    /// events first (so an assertion sees everything emitted up to now), counts
    /// down a pending `settle`, then runs the next step. Returns [`Progress::Done`]
    /// when the steps are exhausted.
    pub fn tick(&mut self, app: &mut impl Driveable) -> Progress {
        self.events.extend(app.drain_events());
        if self.settle > 0 {
            self.settle -= 1;
            return Progress::Running;
        }
        let Some(step) = self.steps.get(self.idx).cloned() else {
            return Progress::Done;
        };
        self.idx += 1;
        self.run(step, app);
        Progress::Running
    }

    /// The finished result: `ok` false if any assertion failed.
    pub fn finish(&self) -> Outcome {
        Outcome {
            ok: !self.failed,
            log: self.log.clone(),
        }
    }

    fn fail(&mut self, why: String) {
        self.failed = true;
        self.log.push(format!("FAIL: {why}"));
    }

    fn run(&mut self, step: Step, app: &mut impl Driveable) {
        match step {
            Step::Act(label) => {
                if !app.act(&label) {
                    self.fail(format!("act: no command '{label}'"));
                }
            }
            Step::Click(sel) => {
                if !app.click(&sel) {
                    // The miss attributes itself into the event stream, so a
                    // scenario that drives a miss can assert it (and one that did
                    // not mean to miss sees why it failed downstream).
                    self.events.push(format!("interaction-missed {}", describe(&sel)));
                }
            }
            Step::Settle(n) => self.settle = n,
            Step::Log(text) => self.log.push(text),
            Step::Capture(name) => {
                if app.capture(&name) {
                    self.log.push(format!("captured {name}"));
                } else {
                    self.fail(format!("capture '{name}' failed"));
                }
            }
            Step::AssertText(substr) => {
                let present = app.with_surfaces(|s| text_present(s, &substr));
                if !present {
                    self.fail(format!("assert text '{substr}': not on any surface"));
                }
            }
            Step::AssertEvent(substr) => {
                if !self.events.iter().any(|e| e.contains(&substr)) {
                    self.fail(format!(
                        "assert event '{substr}': not in the stream (last: {:?})",
                        self.events.iter().rev().take(6).collect::<Vec<_>>()
                    ));
                }
            }
            Step::AssertSnap { field, cmp, value } => {
                let snap = app.snapshot();
                let got = if field == "focused" {
                    snap.focused.clone()
                } else {
                    snap.field(&field).map(str::to_string)
                };
                match got {
                    Some(got) if compare(&got, cmp, &value) => {}
                    Some(got) => self.fail(format!(
                        "assert snap {field} {cmp:?} '{value}': got '{got}'"
                    )),
                    None => self.fail(format!("assert snap {field}: no such field")),
                }
            }
            Step::App(line) => {
                if let Err(msg) = app.app_step(&line) {
                    self.fail(msg);
                }
            }
        }
    }
}

/// Whether `got <cmp> want`. Numeric for `Ge`/`Le` (falling back to false when
/// either side is non-numeric); string-equal for `Eq`; substring for `Contains`.
fn compare(got: &str, cmp: Cmp, want: &str) -> bool {
    match cmp {
        Cmp::Eq => got == want,
        Cmp::Contains => got.contains(want),
        Cmp::Ge | Cmp::Le => match (got.parse::<f64>(), want.parse::<f64>()) {
            (Ok(a), Ok(b)) => {
                if matches!(cmp, Cmp::Ge) {
                    a >= b
                } else {
                    a <= b
                }
            }
            _ => false,
        },
    }
}

/// A grep-friendly rendering of a selector, for the `interaction-missed` event.
fn describe(sel: &Selector) -> String {
    let base = match &sel.matcher {
        Match::Class(c) => format!(".{c}"),
        Match::Role(r) => format!("role:{r}"),
    };
    let mut out = base;
    if let Some(t) = &sel.text {
        out.push_str(&format!(" '{t}'"));
    }
    if let Some((n, v)) = &sel.attr {
        out.push_str(&format!(" @{n}={v}"));
    }
    out
}

fn parse_step(line: &str) -> Result<Step, String> {
    let (verb, rest) = split_first(line);
    match verb {
        "act" => Ok(Step::Act(rest.to_string())),
        "settle" => Ok(Step::Settle(rest.trim().parse().unwrap_or(1))),
        "log" => Ok(Step::Log(rest.to_string())),
        "capture" => Ok(Step::Capture(rest.trim().to_string())),
        "click" => Ok(Step::Click(parse_selector(rest)?)),
        "assert" => {
            let (kind, arg) = split_first(rest);
            match kind {
                "text" => Ok(Step::AssertText(arg.to_string())),
                "event" => Ok(Step::AssertEvent(arg.to_string())),
                "snap" => {
                    let parts: Vec<&str> = arg.splitn(3, char::is_whitespace).collect();
                    match parts.as_slice() {
                        [field, op, value] => Ok(Step::AssertSnap {
                            field: field.to_string(),
                            cmp: parse_cmp(op)?,
                            value: value.trim().to_string(),
                        }),
                        _ => Err("assert snap wants '<field> <op> <value>'".into()),
                    }
                }
                // Any other `assert ...` is an app-specific verb (assert pane,
                // assert focused): pass the whole line through to the host.
                _ => Ok(Step::App(line.to_string())),
            }
        }
        // Unknown verb: the host's own vocabulary.
        _ => Ok(Step::App(line.to_string())),
    }
}

fn parse_cmp(op: &str) -> Result<Cmp, String> {
    match op {
        "==" => Ok(Cmp::Eq),
        ">=" => Ok(Cmp::Ge),
        "<=" => Ok(Cmp::Le),
        "~" => Ok(Cmp::Contains),
        _ => Err(format!("assert snap op wants ==|>=|<=|~, got '{op}'")),
    }
}

/// Parse a `click` selector: `.class` or `role:name`, then an optional
/// `@attr=value` or free text.
fn parse_selector(rest: &str) -> Result<Selector, String> {
    let (head, tail) = split_first(rest.trim());
    let mut sel = if let Some(class) = head.strip_prefix('.') {
        Selector::class(class)
    } else if let Some(role) = head.strip_prefix("role:") {
        Selector::role(role)
    } else {
        return Err(format!("click wants '.class' or 'role:name', got '{head}'"));
    };
    let tail = tail.trim();
    if let Some(attr) = tail.strip_prefix('@') {
        let (name, value) = attr
            .split_once('=')
            .ok_or_else(|| "click @attr wants 'name=value'".to_string())?;
        sel = sel.with_attr(name, value);
    } else if !tail.is_empty() {
        sel = sel.containing(tail);
    }
    Ok(sel)
}

/// Split a line into its first whitespace-delimited token and the rest.
fn split_first(line: &str) -> (&str, &str) {
    match line.trim().split_once(char::is_whitespace) {
        Some((a, b)) => (a, b.trim()),
        None => (line.trim(), ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProbeSnapshot, ProbeSurface};
    use genet_scripted_dom::ScriptedDom;
    use layout_dom_api::{LayoutDom, LayoutDomMut, LocalName, Namespace, QualName};

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    /// A mock app with one clickable `.tab` labelled "Nodes", a focus label, and
    /// a record of what was acted / captured.
    struct MockApp {
        dom: ScriptedDom,
        acted: Vec<String>,
        captured: Vec<String>,
        pressed: Vec<(f32, f32)>,
    }

    impl MockApp {
        fn new() -> Self {
            let mut dom = ScriptedDom::new();
            let root = dom.document();
            let tab = dom.create_element(qual("div"));
            dom.set_attribute(tab, qual("class"), "tab");
            dom.set_attribute(
                tab,
                qual("style"),
                "position:absolute;left:0px;top:0px;width:80px;height:24px;",
            );
            let t = dom.create_text("Nodes");
            dom.append_child(tab, t);
            dom.append_child(root, tab);
            Self {
                dom,
                acted: Vec::new(),
                captured: Vec::new(),
                pressed: Vec::new(),
            }
        }
    }

    impl Automatable for MockApp {
        fn with_surfaces<R>(&self, f: impl FnOnce(&[ProbeSurface<'_>]) -> R) -> R {
            f(&[ProbeSurface {
                name: "mock",
                dom: &self.dom,
                rect: [0.0, 0.0, 200.0, 100.0],
                sheet: "",
            }])
        }
        fn snapshot(&self) -> ProbeSnapshot {
            ProbeSnapshot::default()
                .with_focus("Example Domain")
                .with_field("node-count", "12")
        }
        fn drain_events(&mut self) -> Vec<String> {
            Vec::new()
        }
        fn act(&mut self, label: &str) -> bool {
            self.acted.push(label.to_string());
            label != "Nonexistent"
        }
        fn press(&mut self, x: f32, y: f32) {
            self.pressed.push((x, y));
        }
        fn moved(&mut self, _x: f32, _y: f32) {}
        fn release(&mut self, _x: f32, _y: f32) {}
    }

    impl Driveable for MockApp {
        fn capture(&mut self, name: &str) -> bool {
            self.captured.push(name.to_string());
            true
        }
    }

    fn run(text: &str, app: &mut MockApp) -> Outcome {
        let mut sc = Scenario::parse(text).expect("parse");
        // Pump to completion; a generous cap stands in for the frame loop.
        for _ in 0..1000 {
            if sc.tick(app) == Progress::Done {
                break;
            }
        }
        sc.finish()
    }

    #[test]
    fn a_generic_scenario_runs_green() {
        let mut app = MockApp::new();
        let out = run(
            "# a full generic pass\n\
             act Open something\n\
             settle 2\n\
             click .tab Nodes\n\
             assert text Nodes\n\
             assert snap focused ~ Example\n\
             assert snap node-count >= 10\n\
             capture 01_shot\n\
             log done",
            &mut app,
        );
        assert!(out.ok, "log: {:?}", out.log);
        assert_eq!(app.acted, ["Open something"]);
        assert_eq!(app.captured, ["01_shot"]);
        // The click resolved and pressed the tab's centre (0..80, 0..24).
        assert_eq!(app.pressed, [(40.0, 12.0)]);
    }

    #[test]
    fn a_failed_assert_fails_the_run() {
        let mut app = MockApp::new();
        let out = run("assert snap node-count >= 99", &mut app);
        assert!(!out.ok);
        assert!(out.log.iter().any(|l| l.contains("node-count")), "{:?}", out.log);
    }

    #[test]
    fn a_click_miss_attributes_itself_and_is_assertable() {
        let mut app = MockApp::new();
        // Drive a miss, then assert the loop recorded it — the divergence rule,
        // generalized: the miss is in the event stream, not just lost.
        let out = run(
            "click .tab Nope\n\
             assert event interaction-missed .tab 'Nope'",
            &mut app,
        );
        assert!(out.ok, "log: {:?}", out.log);
        assert!(app.pressed.is_empty(), "a miss must not press");
    }

    #[test]
    fn an_unknown_verb_reaches_app_step_and_fails_by_default() {
        let mut app = MockApp::new();
        let out = run("assert pane roster", &mut app);
        // MockApp does not override app_step, so the app-specific verb fails loudly.
        assert!(!out.ok);
        assert!(out.log.iter().any(|l| l.contains("unknown verb")), "{:?}", out.log);
    }
}
