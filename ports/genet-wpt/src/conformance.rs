//! Absolute WPT conformance accounting.
//!
//! This is deliberately separate from the Stylo-versus-Livery differential.
//! It joins exact per-test result files to the authoritative WPT manifest, so
//! tests that did not run remain visible instead of disappearing from a pass
//! count.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{ManifestTest, TestKind};

const REPORT_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct StatusCounts {
    pub pass: usize,
    pub fail: usize,
    pub skip: usize,
    pub error: usize,
    pub no_results: usize,
}

impl StatusCounts {
    fn record(&mut self, status: &str) -> Result<(), String> {
        match status {
            "pass" => self.pass += 1,
            "fail" => self.fail += 1,
            "skip" => self.skip += 1,
            "error" => self.error += 1,
            "no-results" => self.no_results += 1,
            other => return Err(format!("unknown WPT result status `{other}`")),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct SubtestSummary {
    pub passed: usize,
    pub total: usize,
    pub files_with_counts: usize,
    pub observed_files_without_counts: usize,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct LaneSummary {
    pub manifest_tests: usize,
    pub hostable_manifest_tests: usize,
    pub unhostable_manifest_tests: usize,
    pub observed_tests: usize,
    pub missing_hostable_tests: usize,
    pub statuses: StatusCounts,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtests: Option<SubtestSummary>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct ManifestSummary {
    pub total: usize,
    pub testharness: usize,
    pub reftest: usize,
    pub print_reftest: usize,
    pub crashtest: usize,
    pub manual: usize,
    pub visual: usize,
    pub wdspec: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ConformanceReport {
    pub version: u32,
    pub subset: String,
    pub testharness_cssom: String,
    pub testharness_geometry: String,
    pub reftest_renderer: String,
    pub testharness_engine: String,
    pub complete: bool,
    pub provenance: ProvenanceSummary,
    pub manifest: ManifestSummary,
    pub testharness: LaneSummary,
    pub reftest: LaneSummary,
    pub unsupported_manifest_tests: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ProvenanceSummary {
    pub manifest_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub testharness_runner_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reftest_runner_sha256: Option<String>,
    pub inputs_pinned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    Testharness,
    Reftest,
}

impl Lane {
    fn command(self) -> &'static str {
        match self {
            Lane::Testharness => "testharness",
            Lane::Reftest => "reftest",
        }
    }

    fn accepts(self, kind: TestKind) -> bool {
        match self {
            Lane::Testharness => kind == TestKind::Testharness,
            Lane::Reftest => kind == TestKind::Reftest,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResultRecord {
    status: String,
    reason: Option<String>,
    subtests: Option<(usize, usize)>,
}

#[derive(Deserialize)]
struct ResultFile {
    version: u32,
    command: String,
    engine: String,
    renderer: String,
    subset: Option<String>,
    manifest_sha256: Option<String>,
    runner_sha256: Option<String>,
    tests: BTreeMap<String, serde_json::Value>,
}

pub struct ReportInputs<'a> {
    pub subset: &'a str,
    pub renderer: &'a str,
    pub testharness_engine: &'a str,
    pub manifest_sha256: &'a str,
    pub reftest_results: &'a [PathBuf],
    pub testharness_results: &'a [PathBuf],
    pub allow_incomplete: bool,
    pub allow_unpinned: bool,
}

struct LoadedLane {
    records: BTreeMap<String, ResultRecord>,
    runner_sha256: Option<String>,
    inputs_pinned: bool,
}

pub fn build_report(
    manifest_tests: &[ManifestTest],
    inputs: ReportInputs<'_>,
) -> Result<ConformanceReport, String> {
    let manifest_by_url = index_manifest(manifest_tests)?;
    let manifest = summarize_manifest(manifest_tests);
    let testharness_loaded = load_lane_results(
        Lane::Testharness,
        inputs.testharness_results,
        &inputs,
        &manifest_by_url,
    )?;
    let reftest_loaded = load_lane_results(
        Lane::Reftest,
        inputs.reftest_results,
        &inputs,
        &manifest_by_url,
    )?;
    let testharness = summarize_lane(
        Lane::Testharness,
        manifest_tests,
        &testharness_loaded.records,
        true,
    )?;
    let reftest = summarize_lane(
        Lane::Reftest,
        manifest_tests,
        &reftest_loaded.records,
        false,
    )?;
    let complete = testharness.missing_hostable_tests == 0 && reftest.missing_hostable_tests == 0;

    if !inputs.allow_incomplete && !complete {
        let mut missing = Vec::new();
        if testharness.missing_hostable_tests != 0 {
            missing.push(format!(
                "testharness is missing {} hostable manifest tests",
                testharness.missing_hostable_tests
            ));
        }
        if reftest.missing_hostable_tests != 0 {
            missing.push(format!(
                "reftest is missing {} hostable manifest tests",
                reftest.missing_hostable_tests
            ));
        }
        if !missing.is_empty() {
            return Err(format!(
                "incomplete conformance input: {}; pass --allow-incomplete-conformance only for \
                 a diagnostic report",
                missing.join("; ")
            ));
        }
    }

    let unsupported_manifest_tests = manifest.print_reftest
        + manifest.crashtest
        + manifest.manual
        + manifest.visual
        + manifest.wdspec;
    let inputs_pinned = testharness_loaded.inputs_pinned && reftest_loaded.inputs_pinned;
    Ok(ConformanceReport {
        version: REPORT_VERSION,
        subset: inputs.subset.trim_matches('/').to_string(),
        testharness_cssom: inputs.renderer.to_string(),
        // The current scripted Livery route owns CSSOM and computed style, but
        // retains Stylo as its geometry and animation driver.
        testharness_geometry: "stylo".to_string(),
        reftest_renderer: inputs.renderer.to_string(),
        testharness_engine: inputs.testharness_engine.to_string(),
        complete,
        provenance: ProvenanceSummary {
            manifest_sha256: inputs.manifest_sha256.to_string(),
            testharness_runner_sha256: testharness_loaded.runner_sha256,
            reftest_runner_sha256: reftest_loaded.runner_sha256,
            inputs_pinned,
        },
        manifest,
        testharness,
        reftest,
        unsupported_manifest_tests,
    })
}

fn index_manifest(
    manifest_tests: &[ManifestTest],
) -> Result<BTreeMap<String, &ManifestTest>, String> {
    let mut by_url = BTreeMap::new();
    for test in manifest_tests {
        let url = normalize_url(&test.url);
        if let Some(previous) = by_url.insert(url.clone(), test) {
            return Err(format!(
                "manifest URL `{url}` occurs more than once ({:?} and {:?})",
                previous.kind, test.kind
            ));
        }
    }
    Ok(by_url)
}

fn summarize_manifest(tests: &[ManifestTest]) -> ManifestSummary {
    let mut summary = ManifestSummary {
        total: tests.len(),
        ..ManifestSummary::default()
    };
    for test in tests {
        match test.kind {
            TestKind::Testharness => summary.testharness += 1,
            TestKind::Reftest => summary.reftest += 1,
            TestKind::PrintReftest => summary.print_reftest += 1,
            TestKind::Crashtest => summary.crashtest += 1,
            TestKind::Manual => summary.manual += 1,
            TestKind::Visual => summary.visual += 1,
            TestKind::Wdspec => summary.wdspec += 1,
        }
    }
    summary
}

fn load_lane_results(
    lane: Lane,
    paths: &[PathBuf],
    inputs: &ReportInputs<'_>,
    manifest: &BTreeMap<String, &ManifestTest>,
) -> Result<LoadedLane, String> {
    if paths.is_empty() {
        return Err(format!(
            "conformance needs at least one --{}-results file",
            lane.command()
        ));
    }
    let mut records = BTreeMap::new();
    let mut runner_sha256: Option<String> = None;
    let mut inputs_pinned = true;
    for path in paths {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let file: ResultFile = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        if file.version != 1 {
            return Err(format!(
                "{} has result format version {}, expected 1",
                path.display(),
                file.version
            ));
        }
        if file.command != lane.command() {
            return Err(format!(
                "{} records command `{}`, expected `{}`",
                path.display(),
                file.command,
                lane.command()
            ));
        }
        if !file.renderer.eq_ignore_ascii_case(inputs.renderer) {
            return Err(format!(
                "{} records renderer `{}`, expected `{}`",
                path.display(),
                file.renderer,
                inputs.renderer,
            ));
        }
        let file_pinned =
            file.subset.is_some() && file.manifest_sha256.is_some() && file.runner_sha256.is_some();
        let expected_engine = match lane {
            Lane::Testharness => inputs.testharness_engine,
            Lane::Reftest => "none",
        };
        let legacy_reftest_engine = lane == Lane::Reftest
            && !file_pinned
            && inputs.allow_unpinned
            && file.engine.eq_ignore_ascii_case("boa");
        if !file.engine.eq_ignore_ascii_case(expected_engine) && !legacy_reftest_engine {
            return Err(format!(
                "{} records engine `{}`, expected `{}`",
                path.display(),
                file.engine,
                expected_engine,
            ));
        }
        if let Some(file_subset) = file.subset.as_deref()
            && !scope_contains(inputs.subset, file_subset)
        {
            return Err(format!(
                "{} records subset `{file_subset}`, outside selected subset `{}`",
                path.display(),
                normalize_scope(inputs.subset)
            ));
        }
        if let Some(file_manifest) = file.manifest_sha256.as_deref()
            && file_manifest != inputs.manifest_sha256
        {
            return Err(format!(
                "{} was produced from WPT manifest {}, current manifest is {}",
                path.display(),
                file_manifest,
                inputs.manifest_sha256
            ));
        }
        if let Some(file_runner) = file.runner_sha256.as_deref() {
            validate_sha256("runner_sha256", file_runner, path)?;
            match runner_sha256.as_deref() {
                Some(previous) if previous != file_runner => {
                    return Err(format!(
                        "{} records runner {}, but another {} input records {}",
                        path.display(),
                        file_runner,
                        lane.command(),
                        previous
                    ));
                },
                None => runner_sha256 = Some(file_runner.to_string()),
                _ => {},
            }
        }
        if !file_pinned && !inputs.allow_unpinned {
            return Err(format!(
                "{} lacks pinned subset, manifest, or runner provenance; regenerate it with \
                 this runner, or pass --allow-unpinned-conformance-inputs only for a \
                 diagnostic report",
                path.display()
            ));
        }
        inputs_pinned &= file_pinned;
        for (raw_url, value) in file.tests {
            let url = normalize_url(&raw_url);
            let Some(test) = manifest.get(&url) else {
                return Err(format!(
                    "{} contains `{url}`, which is outside the selected manifest scope",
                    path.display()
                ));
            };
            if !lane.accepts(test.kind) {
                continue;
            }
            let record = parse_result_record(&url, value)?;
            if test.is_worker() {
                if record.status != "skip" {
                    return Err(format!(
                        "{} credits unhostable worker `{url}` as `{}`",
                        path.display(),
                        record.status
                    ));
                }
                continue;
            }
            match records.entry(url) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(record);
                },
                std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &record => {},
                std::collections::btree_map::Entry::Occupied(entry) => {
                    return Err(format!(
                        "conflicting duplicate result for `{}`: {:?} versus {:?}",
                        entry.key(),
                        entry.get(),
                        record
                    ));
                },
            }
        }
    }
    Ok(LoadedLane {
        records,
        runner_sha256,
        inputs_pinned,
    })
}

fn normalize_scope(scope: &str) -> String {
    scope.trim_matches('/').replace('\\', "/")
}

fn scope_contains(selected: &str, file_scope: &str) -> bool {
    let selected = normalize_scope(selected);
    let file_scope = normalize_scope(file_scope);
    selected.is_empty()
        || file_scope == selected
        || file_scope
            .strip_prefix(&selected)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn validate_sha256(field: &str, digest: &str, path: &Path) -> Result<(), String> {
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!(
            "{} has invalid `{field}` `{digest}`; expected 64 hexadecimal digits",
            path.display()
        ))
    }
}

fn parse_result_record(url: &str, value: serde_json::Value) -> Result<ResultRecord, String> {
    match value {
        serde_json::Value::String(status) => Ok(ResultRecord {
            status: normalize_status(&status)?,
            reason: None,
            subtests: None,
        }),
        serde_json::Value::Object(fields) => {
            let status = fields
                .get("status")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("result for `{url}` lacks a string `status`"))?;
            let reason = match fields.get("reason") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(reason)) => Some(reason.clone()),
                Some(_) => return Err(format!("result for `{url}` has a non-string `reason`")),
            };
            let count = |field: &str| -> Result<Option<usize>, String> {
                match fields.get(field) {
                    None | Some(serde_json::Value::Null) => Ok(None),
                    Some(value) => value
                        .as_u64()
                        .and_then(|count| usize::try_from(count).ok())
                        .map(Some)
                        .ok_or_else(|| {
                            format!("result for `{url}` has an invalid `{field}` count")
                        }),
                }
            };
            let subtests = match (count("subtests_passed")?, count("subtests_total")?) {
                (Some(passed), Some(total)) if passed <= total => Some((passed, total)),
                (Some(passed), Some(total)) => {
                    return Err(format!(
                        "result for `{url}` passes {passed} of only {total} subtests"
                    ));
                },
                (None, None) => None,
                _ => {
                    return Err(format!(
                        "result for `{url}` must carry both subtest counts or neither"
                    ));
                },
            };
            Ok(ResultRecord {
                status: normalize_status(status)?,
                reason,
                subtests,
            })
        },
        _ => Err(format!(
            "result for `{url}` must be a status string or an object"
        )),
    }
}

fn normalize_status(status: &str) -> Result<String, String> {
    let status = status.to_ascii_lowercase();
    let mut validation = StatusCounts::default();
    validation.record(&status)?;
    Ok(status)
}

fn summarize_lane(
    lane: Lane,
    manifest_tests: &[ManifestTest],
    records: &BTreeMap<String, ResultRecord>,
    include_subtests: bool,
) -> Result<LaneSummary, String> {
    let mut summary = LaneSummary {
        subtests: include_subtests.then(SubtestSummary::default),
        ..LaneSummary::default()
    };
    let mut expected_urls = BTreeSet::new();
    for test in manifest_tests.iter().filter(|test| lane.accepts(test.kind)) {
        let url = normalize_url(&test.url);
        expected_urls.insert(url.clone());
        summary.manifest_tests += 1;
        if test.is_worker() {
            summary.unhostable_manifest_tests += 1;
        } else {
            summary.hostable_manifest_tests += 1;
        }
        if let Some(record) = records.get(&url) {
            summary.observed_tests += 1;
            summary.statuses.record(&record.status)?;
            if let Some(subtests) = summary.subtests.as_mut() {
                if let Some((passed, total)) = record.subtests {
                    subtests.passed += passed;
                    subtests.total += total;
                    subtests.files_with_counts += 1;
                } else {
                    subtests.observed_files_without_counts += 1;
                }
            } else if record.subtests.is_some() {
                return Err(format!(
                    "reftest result `{url}` unexpectedly carries subtest counts"
                ));
            }
        } else if !test.is_worker() {
            summary.missing_hostable_tests += 1;
        }
    }
    if let Some(extra) = records.keys().find(|url| !expected_urls.contains(*url)) {
        return Err(format!(
            "{} result `{extra}` has no matching manifest test",
            lane.command()
        ));
    }
    Ok(summary)
}

fn normalize_url(url: &str) -> String {
    url.trim_start_matches('/').replace('\\', "/")
}

pub fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl ConformanceReport {
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("cannot serialize conformance report: {error}"))?;
        fs::write(path, format!("{json}\n"))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    pub fn read(path: &Path) -> Result<ConformanceReport, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let report: ConformanceReport = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        if report.version != REPORT_VERSION {
            return Err(format!(
                "{} has conformance report version {}, expected {REPORT_VERSION}",
                path.display(),
                report.version
            ));
        }
        let actually_complete = report.testharness.missing_hostable_tests == 0
            && report.reftest.missing_hostable_tests == 0;
        if report.complete != actually_complete {
            return Err(format!(
                "{} has inconsistent completeness metadata",
                path.display()
            ));
        }
        Ok(report)
    }

    pub fn human_summary(&self) -> String {
        let testharness = &self.testharness;
        let reftest = &self.reftest;
        let subtests = testharness
            .subtests
            .as_ref()
            .expect("testharness summary always carries subtests");
        format!(
            "absolute WPT conformance\n\
             subset: {}\n\
             testharness: CSSOM {}  geometry {}  script engine {}\n\
             reftest renderer: {}\n\
             input provenance: {}  complete: {}  baseline eligible: {}\n\
             manifest: {} variants\n\
             testharness: observed {}/{} hostable, unhostable {}, missing {}\n\
               files: pass {}  fail {}  skip {}  error {}  no-results {}\n\
               subtests: {}/{} passed; {} observed files had no subtest counts\n\
             reftest: observed {}/{} hostable, unhostable {}, missing {}\n\
               files: pass {}  fail {}  skip {}  error {}\n\
             unsupported manifest kinds: {}",
            self.subset,
            self.testharness_cssom,
            self.testharness_geometry,
            self.testharness_engine,
            self.reftest_renderer,
            if self.provenance.inputs_pinned {
                "pinned"
            } else {
                "unpinned"
            },
            if self.complete { "yes" } else { "no" },
            if self.baseline_eligible() {
                "yes"
            } else {
                "no"
            },
            self.manifest.total,
            testharness.observed_tests,
            testharness.hostable_manifest_tests,
            testharness.unhostable_manifest_tests,
            testharness.missing_hostable_tests,
            testharness.statuses.pass,
            testharness.statuses.fail,
            testharness.statuses.skip,
            testharness.statuses.error,
            testharness.statuses.no_results,
            subtests.passed,
            subtests.total,
            subtests.observed_files_without_counts,
            reftest.observed_tests,
            reftest.hostable_manifest_tests,
            reftest.unhostable_manifest_tests,
            reftest.missing_hostable_tests,
            reftest.statuses.pass,
            reftest.statuses.fail,
            reftest.statuses.skip,
            reftest.statuses.error,
            self.unsupported_manifest_tests,
        )
    }

    pub fn delta_from(&self, baseline: &ConformanceReport) -> Result<String, String> {
        if !self.baseline_eligible() {
            return Err(
                "current conformance report is diagnostic: it is incomplete or has unpinned inputs"
                    .to_string(),
            );
        }
        if !baseline.baseline_eligible() {
            return Err(
                "baseline conformance report is diagnostic: it is incomplete or has unpinned inputs"
                    .to_string(),
            );
        }
        if self.reftest_renderer != baseline.reftest_renderer {
            return Err(format!(
                "reftest renderer changed from `{}` to `{}`",
                baseline.reftest_renderer, self.reftest_renderer
            ));
        }
        if self.testharness_cssom != baseline.testharness_cssom
            || self.testharness_geometry != baseline.testharness_geometry
        {
            return Err(format!(
                "testharness route changed from `{} CSSOM + {} geometry` to `{} CSSOM + {} \
                 geometry`",
                baseline.testharness_cssom,
                baseline.testharness_geometry,
                self.testharness_cssom,
                self.testharness_geometry
            ));
        }
        if self.testharness_engine != baseline.testharness_engine {
            return Err(format!(
                "testharness engine changed from `{}` to `{}`",
                baseline.testharness_engine, self.testharness_engine
            ));
        }
        if self.subset != baseline.subset {
            return Err(format!(
                "conformance subset changed from `{}` to `{}`",
                baseline.subset, self.subset
            ));
        }
        let current_subtests = self
            .testharness
            .subtests
            .as_ref()
            .expect("testharness summary always carries subtests");
        let baseline_subtests = baseline
            .testharness
            .subtests
            .as_ref()
            .ok_or("baseline testharness summary lacks subtest counts")?;
        Ok(format!(
            "delta from conformance baseline\n\
             WPT manifest changed: {}\n\
             testharness runner changed: {}\n\
             reftest runner changed: {}\n\
             manifest variants: {:+}\n\
             testharness passing subtests: {:+}\n\
             testharness total subtests observed: {:+}\n\
             testharness passing files: {:+}\n\
             testharness missing hostable files: {:+}\n\
             reftest passing files: {:+}\n\
             reftest missing hostable files: {:+}\n\
             unsupported manifest kinds: {:+}",
            yes_no(self.provenance.manifest_sha256 != baseline.provenance.manifest_sha256),
            yes_no(
                self.provenance.testharness_runner_sha256
                    != baseline.provenance.testharness_runner_sha256
            ),
            yes_no(
                self.provenance.reftest_runner_sha256 != baseline.provenance.reftest_runner_sha256
            ),
            signed_delta(self.manifest.total, baseline.manifest.total),
            signed_delta(current_subtests.passed, baseline_subtests.passed),
            signed_delta(current_subtests.total, baseline_subtests.total),
            signed_delta(
                self.testharness.statuses.pass,
                baseline.testharness.statuses.pass,
            ),
            signed_delta(
                self.testharness.missing_hostable_tests,
                baseline.testharness.missing_hostable_tests,
            ),
            signed_delta(self.reftest.statuses.pass, baseline.reftest.statuses.pass),
            signed_delta(
                self.reftest.missing_hostable_tests,
                baseline.reftest.missing_hostable_tests,
            ),
            signed_delta(
                self.unsupported_manifest_tests,
                baseline.unsupported_manifest_tests,
            ),
        ))
    }

    fn baseline_eligible(&self) -> bool {
        self.complete && self.provenance.inputs_pinned
    }
}

fn signed_delta(current: usize, baseline: usize) -> i128 {
    current as i128 - baseline as i128
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_SHA256: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const RUNNER_SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn manifest_test(url: &str, source_path: &str, kind: TestKind) -> ManifestTest {
        ManifestTest {
            source_path: source_path.to_string(),
            url: url.to_string(),
            kind,
            refs: Vec::new(),
            fuzzy: None,
            long_timeout: false,
        }
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "genet-conformance-{name}-{}.json",
            std::process::id()
        ))
    }

    fn write_results(path: &Path, command: &str, tests: serde_json::Value) -> PathBuf {
        fs::write(
            path,
            serde_json::to_string(&serde_json::json!({
                "version": 1,
                "command": command,
                "engine": if command == "reftest" { "none" } else { "boa" },
                "renderer": "livery",
                "subset": "css",
                "manifest_sha256": MANIFEST_SHA256,
                "runner_sha256": RUNNER_SHA256,
                "tests": tests,
            }))
            .expect("serialize fixture"),
        )
        .expect("write fixture");
        path.to_path_buf()
    }

    fn inputs<'a>(
        reftest_results: &'a [PathBuf],
        testharness_results: &'a [PathBuf],
    ) -> ReportInputs<'a> {
        ReportInputs {
            subset: "css",
            renderer: "livery",
            testharness_engine: "boa",
            manifest_sha256: MANIFEST_SHA256,
            reftest_results,
            testharness_results,
            allow_incomplete: false,
            allow_unpinned: false,
        }
    }

    #[test]
    fn report_counts_missing_and_unhostable_manifest_tests() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/a.worker.html", "css/a.any.js", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
            manifest_test("/css/manual.html", "css/manual.html", TestKind::Manual),
        ];
        let harness_path = write_results(
            &temp_path("harness"),
            "testharness",
            serde_json::json!({
                "css/a.html": {
                    "status": "fail",
                    "subtests_passed": 7,
                    "subtests_total": 9
                },
                "css/b.html": {"status": "skip", "reason": "non-testharness"}
            }),
        );
        let reftest_path = write_results(
            &temp_path("reftest"),
            "reftest",
            serde_json::json!({
                "css/a.html": {"status": "skip", "reason": "non-reftest"}
            }),
        );
        let report = build_report(
            &tests,
            ReportInputs {
                subset: "css",
                renderer: "livery",
                testharness_engine: "boa",
                manifest_sha256: MANIFEST_SHA256,
                reftest_results: &[reftest_path.clone()],
                testharness_results: &[harness_path.clone()],
                allow_incomplete: true,
                allow_unpinned: false,
            },
        )
        .expect("partial report");

        assert_eq!(report.manifest.total, 4);
        assert_eq!(report.testharness.observed_tests, 1);
        assert_eq!(report.testharness.unhostable_manifest_tests, 1);
        assert_eq!(report.testharness.missing_hostable_tests, 0);
        assert_eq!(report.testharness.statuses.fail, 1);
        assert_eq!(
            report.testharness.subtests,
            Some(SubtestSummary {
                passed: 7,
                total: 9,
                files_with_counts: 1,
                observed_files_without_counts: 0,
            })
        );
        assert_eq!(report.reftest.missing_hostable_tests, 1);
        assert_eq!(report.unsupported_manifest_tests, 1);
        let _ = fs::remove_file(harness_path);
        let _ = fs::remove_file(reftest_path);
    }

    #[test]
    fn complete_report_rejects_a_missing_hostable_result() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
        ];
        let harness_path = write_results(
            &temp_path("complete-harness"),
            "testharness",
            serde_json::json!({"css/a.html": "pass"}),
        );
        let reftest_path = write_results(
            &temp_path("complete-reftest"),
            "reftest",
            serde_json::json!({}),
        );
        let error = build_report(
            &tests,
            ReportInputs {
                subset: "css",
                renderer: "livery",
                testharness_engine: "boa",
                manifest_sha256: MANIFEST_SHA256,
                reftest_results: &[reftest_path.clone()],
                testharness_results: &[harness_path.clone()],
                allow_incomplete: false,
                allow_unpinned: false,
            },
        )
        .expect_err("missing reftest must reject a baseline");
        assert!(error.contains("reftest is missing 1"), "{error}");
        let _ = fs::remove_file(harness_path);
        let _ = fs::remove_file(reftest_path);
    }

    #[test]
    fn overlapping_files_deduplicate_exact_records_and_reject_conflicts() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
        ];
        let harness_a = write_results(
            &temp_path("overlap-a"),
            "testharness",
            serde_json::json!({"css/a.html": "pass"}),
        );
        let harness_b = write_results(
            &temp_path("overlap-b"),
            "testharness",
            serde_json::json!({"css/a.html": "pass"}),
        );
        let reftest = write_results(
            &temp_path("overlap-ref"),
            "reftest",
            serde_json::json!({"css/b.html": "pass"}),
        );
        let report = build_report(
            &tests,
            ReportInputs {
                subset: "css",
                renderer: "livery",
                testharness_engine: "boa",
                manifest_sha256: MANIFEST_SHA256,
                reftest_results: &[reftest.clone()],
                testharness_results: &[harness_a.clone(), harness_b.clone()],
                allow_incomplete: false,
                allow_unpinned: false,
            },
        )
        .expect("identical overlap is safe");
        assert_eq!(report.testharness.observed_tests, 1);

        write_results(
            &harness_b,
            "testharness",
            serde_json::json!({"css/a.html": "fail"}),
        );
        let error = build_report(
            &tests,
            ReportInputs {
                subset: "css",
                renderer: "livery",
                testharness_engine: "boa",
                manifest_sha256: MANIFEST_SHA256,
                reftest_results: &[reftest.clone()],
                testharness_results: &[harness_a.clone(), harness_b.clone()],
                allow_incomplete: false,
                allow_unpinned: false,
            },
        )
        .expect_err("conflicting overlap");
        assert!(error.contains("conflicting duplicate result"), "{error}");
        let _ = fs::remove_file(harness_a);
        let _ = fs::remove_file(harness_b);
        let _ = fs::remove_file(reftest);
    }

    #[test]
    fn report_round_trip_and_delta_keep_the_instrument_absolute() {
        let report = ConformanceReport {
            version: REPORT_VERSION,
            subset: "css".into(),
            testharness_cssom: "livery".into(),
            testharness_geometry: "stylo".into(),
            reftest_renderer: "livery".into(),
            testharness_engine: "boa".into(),
            complete: true,
            provenance: ProvenanceSummary {
                manifest_sha256: MANIFEST_SHA256.into(),
                testharness_runner_sha256: Some(RUNNER_SHA256.into()),
                reftest_runner_sha256: Some(RUNNER_SHA256.into()),
                inputs_pinned: true,
            },
            manifest: ManifestSummary {
                total: 20,
                ..ManifestSummary::default()
            },
            testharness: LaneSummary {
                statuses: StatusCounts {
                    pass: 3,
                    ..StatusCounts::default()
                },
                subtests: Some(SubtestSummary {
                    passed: 12,
                    total: 20,
                    ..SubtestSummary::default()
                }),
                ..LaneSummary::default()
            },
            reftest: LaneSummary {
                statuses: StatusCounts {
                    pass: 4,
                    ..StatusCounts::default()
                },
                ..LaneSummary::default()
            },
            unsupported_manifest_tests: 2,
        };
        let path = temp_path("round-trip");
        report.write(&path).expect("write report");
        assert_eq!(ConformanceReport::read(&path).expect("read report"), report);
        let delta = report.delta_from(&report).expect("same report compares");
        assert!(delta.contains("passing subtests: +0"), "{delta}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn print_reftests_are_unsupported_and_never_credited_as_screen_reftests() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
            manifest_test("/css/print.html", "css/print.html", TestKind::PrintReftest),
        ];
        let harness = write_results(
            &temp_path("print-harness"),
            "testharness",
            serde_json::json!({"css/a.html": "pass"}),
        );
        let reftest = write_results(
            &temp_path("print-reftest"),
            "reftest",
            serde_json::json!({
                "css/b.html": "pass",
                "css/print.html": "pass"
            }),
        );
        let report = build_report(
            &tests,
            inputs(
                std::slice::from_ref(&reftest),
                std::slice::from_ref(&harness),
            ),
        )
        .expect("screen-only report");
        assert_eq!(report.reftest.manifest_tests, 1);
        assert_eq!(report.reftest.statuses.pass, 1);
        assert_eq!(report.unsupported_manifest_tests, 1);
        let _ = fs::remove_file(harness);
        let _ = fs::remove_file(reftest);
    }

    #[test]
    fn worker_records_never_inflate_hostable_totals() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/a.worker.html", "css/a.any.js", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
        ];
        let harness = write_results(
            &temp_path("worker-harness"),
            "testharness",
            serde_json::json!({
                "css/a.html": "pass",
                "css/a.worker.html": "pass"
            }),
        );
        let reftest = write_results(
            &temp_path("worker-reftest"),
            "reftest",
            serde_json::json!({"css/b.html": "pass"}),
        );
        let error = build_report(
            &tests,
            inputs(
                std::slice::from_ref(&reftest),
                std::slice::from_ref(&harness),
            ),
        )
        .expect_err("an unhostable worker cannot pass");
        assert!(error.contains("credits unhostable worker"), "{error}");

        write_results(
            &harness,
            "testharness",
            serde_json::json!({
                "css/a.html": "pass",
                "css/a.worker.html": {"status": "skip", "reason": "worker-only"}
            }),
        );
        let report = build_report(
            &tests,
            inputs(
                std::slice::from_ref(&reftest),
                std::slice::from_ref(&harness),
            ),
        )
        .expect("worker skip is excluded");
        assert_eq!(report.testharness.observed_tests, 1);
        assert_eq!(report.testharness.statuses.pass, 1);
        assert_eq!(report.testharness.statuses.skip, 0);
        let _ = fs::remove_file(harness);
        let _ = fs::remove_file(reftest);
    }

    #[test]
    fn unpinned_inputs_are_diagnostic_and_cannot_be_baselines() {
        let tests = vec![
            manifest_test("/css/a.html", "css/a.html", TestKind::Testharness),
            manifest_test("/css/b.html", "css/b.html", TestKind::Reftest),
        ];
        let harness = temp_path("legacy-harness");
        let reftest = temp_path("legacy-reftest");
        for (path, command, test) in [
            (&harness, "testharness", "css/a.html"),
            (&reftest, "reftest", "css/b.html"),
        ] {
            fs::write(
                path,
                serde_json::to_string(&serde_json::json!({
                    "version": 1,
                    "command": command,
                    "engine": "boa",
                    "renderer": "livery",
                    "tests": {(test): "pass"}
                }))
                .expect("serialize legacy fixture"),
            )
            .expect("write legacy fixture");
        }
        let mut report_inputs = inputs(
            std::slice::from_ref(&reftest),
            std::slice::from_ref(&harness),
        );
        let error = build_report(&tests, report_inputs)
            .expect_err("unpinned input must fail closed by default");
        assert!(error.contains("lacks pinned"), "{error}");

        report_inputs = inputs(
            std::slice::from_ref(&reftest),
            std::slice::from_ref(&harness),
        );
        report_inputs.allow_unpinned = true;
        let report = build_report(&tests, report_inputs).expect("diagnostic legacy report");
        assert!(report.complete);
        assert!(!report.provenance.inputs_pinned);
        let error = report
            .delta_from(&report)
            .expect_err("an unpinned report cannot be a baseline");
        assert!(error.contains("diagnostic"), "{error}");
        let _ = fs::remove_file(harness);
        let _ = fs::remove_file(reftest);
    }
}
