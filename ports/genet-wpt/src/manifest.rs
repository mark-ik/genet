//! MANIFEST.json reader (WPT manifest v8/v9).
//!
//! Replaces the runner's heuristic discovery (`collect`'s directory walk,
//! `synthesize_any_js`'s `.any.js` -> `.any.html`/`.any.worker.html` expansion,
//! and `parse_fuzzy`'s reftest-metadata reconstruction) with the
//! upstream-generated, authoritative enumeration: every test's URL(s), kind,
//! reftest references, fuzzy tolerance, and timeout, exactly as `wpt run` sees
//! them. This is grand-audit lever 1 / harness-exactness H1.
//!
//! ## Manifest shape
//!
//! `{"version": 9, "url_base": "/", "items": {<kind>: <tree>}}` where each
//! `<tree>` is nested by path component (`{"FileAPI": {"Blob": {"x.html":
//! <leaf>}}}`). A node whose value is an **object** is a directory; a node whose
//! value is an **array** is a test **leaf**: `[hash, variant, variant, ...]`.
//! Each variant is `[url, extras]` for a testharness-family test (one entry per
//! global / `?query` variant; `.any.js` is pre-expanded here for discovery) or
//! `[url, references, extras]` for a reftest. A `null` url means "use the leaf's
//! own path".

use std::path::Path;

use serde_json::Value;

/// The kind of a manifest test (the `items.<kind>` bucket it came from).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    Testharness,
    Reftest,
    PrintReftest,
    Crashtest,
    Manual,
    Visual,
    Wdspec,
}

impl TestKind {
    /// Whether this kind carries reftest references (`[url, refs, extras]` leaf).
    fn is_reftest(self) -> bool {
        matches!(self, TestKind::Reftest | TestKind::PrintReftest)
    }

    /// A short label for listings (mirrors the runner's `Kind::label`).
    pub fn label(self) -> &'static str {
        match self {
            TestKind::Testharness => "testharness",
            TestKind::Reftest => "reftest",
            TestKind::PrintReftest => "print-ref",
            TestKind::Crashtest => "crashtest",
            TestKind::Manual => "manual",
            TestKind::Visual => "visual",
            TestKind::Wdspec => "wdspec",
        }
    }
}

/// A reftest reference comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefMatch {
    Equal,
    NotEqual,
}

/// One runnable test enumerated from the manifest. A `.any.js` test or a
/// multi-`?query` test expands to several of these (one per variant URL).
#[derive(Clone, Debug, PartialEq)]
pub struct ManifestTest {
    /// The backing file path inside the tests root. For generated variants such
    /// as `/dom/foo.any.html`, this remains the source `dom/foo.any.js`.
    pub source_path: String,
    /// The test URL, `url_base`-relative (e.g. `/FileAPI/Blob/x.any.worker.html`).
    pub url: String,
    pub kind: TestKind,
    /// Reftest references `(ref_url, comparison)`; empty for testharness tests.
    pub refs: Vec<(String, RefMatch)>,
    /// Fuzzy tolerance, if declared: `(max_pixel_diff_range, total_pixels_range)`.
    pub fuzzy: Option<((u32, u32), (u32, u32))>,
    /// Whether the manifest marks the test `timeout: long`.
    pub long_timeout: bool,
}

impl ManifestTest {
    /// Whether this variant runs in a worker (`.worker.html` / `.any.worker.html`).
    /// The window-shaped runner cannot host workers, so callers skip these.
    pub fn is_worker(&self) -> bool {
        self.url.contains(".worker.")
            || self.url.contains(".worker?")
            || self.url.contains(".sharedworker.")
            || self.url.contains(".serviceworker.")
            || self.url.contains(".shadowrealm-")
    }
}

/// A parsed WPT MANIFEST.json.
pub struct Manifest {
    items: Value,
    url_base: String,
}

impl Manifest {
    /// Load + parse `MANIFEST.json` (the upstream-generated tree, usually at
    /// `<tests-root>/../meta/MANIFEST.json`). The file is large (tens of MB); this
    /// parses it once at startup.
    pub fn load(path: &Path) -> std::io::Result<Manifest> {
        let file = std::fs::File::open(path)?;
        let root: Value = serde_json::from_reader(std::io::BufReader::new(file))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Manifest::from_value(root)
    }

    fn from_value(root: Value) -> std::io::Result<Manifest> {
        let version = root.get("version").and_then(Value::as_u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "manifest has no numeric version",
            )
        })?;
        if !matches!(version, 8 | 9) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unsupported WPT manifest version {version}; expected 8 or 9"),
            ));
        }
        let url_base = root
            .get("url_base")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "manifest has no string url_base",
                )
            })?
            .to_string();
        let items = root
            .get("items")
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "no items"))?;
        if !items.is_object() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "manifest items is not an object",
            ));
        }
        let manifest = Manifest { items, url_base };
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> std::io::Result<()> {
        for (kind, key) in [
            (TestKind::Testharness, "testharness"),
            (TestKind::Reftest, "reftest"),
            (TestKind::PrintReftest, "print-reftest"),
            (TestKind::Crashtest, "crashtest"),
            (TestKind::Visual, "visual"),
            (TestKind::Wdspec, "wdspec"),
            (TestKind::Manual, "manual"),
        ] {
            if let Some(tree) = self.items.get(key) {
                self.validate_node(tree, kind, &mut String::new())?;
            }
        }
        Ok(())
    }

    fn validate_node(
        &self,
        node: &Value,
        kind: TestKind,
        prefix: &mut String,
    ) -> std::io::Result<()> {
        match node {
            Value::Object(map) => {
                for (component, child) in map {
                    let restore = prefix.len();
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(component);
                    self.validate_node(child, kind, prefix)?;
                    prefix.truncate(restore);
                }
                Ok(())
            },
            Value::Array(leaf) => {
                if leaf.is_empty() || !matches!(leaf.first(), Some(Value::String(_) | Value::Null))
                {
                    return Err(invalid_manifest(
                        kind,
                        prefix,
                        "leaf lacks a string or null source hash",
                    ));
                }
                if leaf.len() == 1 {
                    return Err(invalid_manifest(kind, prefix, "leaf has no test variants"));
                }
                for (index, variant) in leaf.iter().enumerate().skip(1) {
                    let Some(fields) = variant.as_array() else {
                        return Err(invalid_manifest(
                            kind,
                            prefix,
                            &format!("variant {index} is not an array"),
                        ));
                    };
                    let expected_fields = if kind.is_reftest() { 3 } else { 2 };
                    if fields.len() != expected_fields {
                        return Err(invalid_manifest(
                            kind,
                            prefix,
                            &format!(
                                "variant {index} has {} fields, expected {expected_fields}",
                                fields.len()
                            ),
                        ));
                    }
                    if !matches!(fields.first(), Some(Value::String(_) | Value::Null)) {
                        return Err(invalid_manifest(
                            kind,
                            prefix,
                            &format!("variant {index} lacks a string or null URL"),
                        ));
                    }
                    if kind.is_reftest() {
                        let Some(refs) = fields.get(1).and_then(Value::as_array) else {
                            return Err(invalid_manifest(
                                kind,
                                prefix,
                                &format!("variant {index} lacks a reference list"),
                            ));
                        };
                        for reference in refs {
                            let Some(pair) = reference.as_array() else {
                                return Err(invalid_manifest(
                                    kind,
                                    prefix,
                                    &format!("variant {index} has a malformed reference"),
                                ));
                            };
                            if pair.len() != 2
                                || pair.first().and_then(Value::as_str).is_none()
                                || !matches!(pair.get(1).and_then(Value::as_str), Some("==" | "!="))
                            {
                                return Err(invalid_manifest(
                                    kind,
                                    prefix,
                                    &format!("variant {index} has a malformed reference"),
                                ));
                            }
                        }
                    }
                    if fields.last().and_then(Value::as_object).is_none() {
                        return Err(invalid_manifest(
                            kind,
                            prefix,
                            &format!("variant {index} lacks an extras object"),
                        ));
                    }
                    if self.parse_variant(variant, kind, prefix).is_none() {
                        return Err(invalid_manifest(
                            kind,
                            prefix,
                            &format!("variant {index} has an unsupported shape"),
                        ));
                    }
                }
                Ok(())
            },
            _ => Err(invalid_manifest(
                kind,
                prefix,
                "tree node is neither a directory object nor a test leaf",
            )),
        }
    }

    /// Enumerate the runnable tests of the kinds this runner can host, with
    /// variants expanded. Worker-only variants are included (flagged by
    /// [`ManifestTest::is_worker`]); the caller decides whether to skip them.
    pub fn tests(&self) -> Vec<ManifestTest> {
        let mut out = Vec::new();
        for (kind, key) in [
            (TestKind::Testharness, "testharness"),
            (TestKind::Reftest, "reftest"),
            (TestKind::PrintReftest, "print-reftest"),
            (TestKind::Crashtest, "crashtest"),
            (TestKind::Visual, "visual"),
            (TestKind::Wdspec, "wdspec"),
            (TestKind::Manual, "manual"),
        ] {
            if let Some(tree) = self.items.get(key) {
                self.walk(tree, kind, &mut String::new(), &mut out);
            }
        }
        out
    }

    /// The tests whose URL falls under `subset` (a tests-root-relative dir like
    /// `dom` or `css/CSS2/floats`, or `""` for the whole tree) — for spot-checking
    /// the manifest enumeration against the directory walk.
    pub fn tests_under(&self, subset: &str) -> Vec<ManifestTest> {
        let subset = subset.trim_matches('/');
        if subset.is_empty() {
            return self.tests();
        }
        let want = subset.trim_start_matches('/');
        self.tests()
            .into_iter()
            .filter(|t| {
                let matches = |candidate: &str| {
                    let candidate = candidate.trim_start_matches('/');
                    candidate
                        .strip_prefix(&format!(
                            "{}{}",
                            self.url_base.trim_start_matches('/'),
                            want
                        ))
                        .or_else(|| candidate.strip_prefix(want))
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
                };
                matches(&t.url) || matches(&t.source_path)
            })
            .collect()
    }

    fn walk(&self, node: &Value, kind: TestKind, prefix: &mut String, out: &mut Vec<ManifestTest>) {
        match node {
            // Directory node: recurse, extending the path prefix by the component.
            Value::Object(map) => {
                for (component, child) in map {
                    let restore = prefix.len();
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    prefix.push_str(component);
                    self.walk(child, kind, prefix, out);
                    prefix.truncate(restore);
                }
            },
            // Test leaf: `[hash, variant, variant, ...]`.
            Value::Array(leaf) => {
                for variant in leaf.iter().skip(1) {
                    if let Some(test) = self.parse_variant(variant, kind, prefix) {
                        out.push(test);
                    }
                }
            },
            _ => {},
        }
    }

    /// Parse one variant of a leaf: `[url, extras]` (testharness family) or
    /// `[url, references, extras]` (reftest). `url == null` -> the leaf's path.
    fn parse_variant(&self, variant: &Value, kind: TestKind, prefix: &str) -> Option<ManifestTest> {
        let arr = variant.as_array()?;
        let url = match arr.first() {
            Some(Value::String(s)) => s.clone(),
            // null url -> the file path itself, under url_base.
            Some(Value::Null) | None => format!("{}{}", self.url_base, prefix),
            _ => return None,
        };
        let (refs, extras) = if kind.is_reftest() {
            (parse_refs(arr.get(1)), arr.get(2))
        } else {
            (Vec::new(), arr.get(1))
        };
        let long_timeout = extras
            .and_then(|e| e.get("timeout"))
            .and_then(Value::as_str)
            == Some("long");
        let fuzzy = extras.and_then(|e| e.get("fuzzy")).and_then(parse_fuzzy);
        Some(ManifestTest {
            source_path: prefix.to_string(),
            url,
            kind,
            refs,
            fuzzy,
            long_timeout,
        })
    }
}

fn invalid_manifest(kind: TestKind, path: &str, detail: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "malformed {} manifest entry `{}`: {detail}",
            kind.label(),
            if path.is_empty() { "<root>" } else { path }
        ),
    )
}

/// Parse a reftest references value: `[[ref_url, "=="|"!="], ...]`.
fn parse_refs(value: Option<&Value>) -> Vec<(String, RefMatch)> {
    let Some(Value::Array(list)) = value else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|pair| {
            let p = pair.as_array()?;
            let url = p.first()?.as_str()?.to_string();
            let cmp = match p.get(1).and_then(Value::as_str) {
                Some("!=") => RefMatch::NotEqual,
                _ => RefMatch::Equal,
            };
            Some((url, cmp))
        })
        .collect()
}

/// Parse the `fuzzy` extras into `((max_diff_lo, max_diff_hi), (total_lo, total_hi))`.
/// The manifest shape is `[[ [maxDiff_lo, maxDiff_hi], [total_lo, total_hi] ]]` (or
/// the same keyed by a ref pair); take the first range pair, a best-effort first cut.
fn parse_fuzzy(value: &Value) -> Option<((u32, u32), (u32, u32))> {
    fn range_pair(v: &Value) -> Option<((u32, u32), (u32, u32))> {
        let a = v.as_array()?;
        // `a` is `[[lo,hi],[lo,hi]]`: the maxDifference range and totalPixels range.
        let diff = a.first()?.as_array()?;
        let total = a.get(1)?.as_array()?;
        let n = |x: Option<&Value>| x.and_then(Value::as_u64).map(|v| v as u32);
        Some((
            (n(diff.first())?, n(diff.get(1))?),
            (n(total.first())?, n(total.get(1))?),
        ))
    }
    // `value` is a list; an entry is either the range pair directly, or
    // `[ref_pair, range_pair]` when keyed by reference. Probe both.
    let list = value.as_array()?;
    let first = list.first()?;
    range_pair(first).or_else(|| range_pair(first.as_array()?.get(1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Manifest {
        // A tiny hand-built manifest exercising: a directory, a plain testharness
        // test (null url -> path), a `.any.js` expanded to two global variants,
        // and a reftest with a `==` ref + fuzzy.
        let json = serde_json::json!({
            "version": 9,
            "url_base": "/",
            "items": {
                "testharness": {
                    "FileAPI": {
                        "Blob": {
                            "size.html": ["hash", [serde_json::Value::Null, {}]],
                            "x.any.js": [
                                "hash",
                                ["/FileAPI/Blob/x.any.html", { "timeout": "long" }],
                                ["/FileAPI/Blob/x.any.worker.html", {}]
                            ],
                            "y.any.js": [
                                "hash",
                                ["FileAPI/Blob/y.any.html", {}]
                            ]
                        }
                    }
                },
                "reftest": {
                    "css": {
                        "ref.html": [
                            "hash",
                            ["/css/ref.html", [["/css/ref-ref.html", "=="]], { "fuzzy": [[[0, 2], [0, 40]]] }]
                        ]
                    }
                }
            }
        });
        Manifest::from_value(json).expect("fixture parses")
    }

    #[test]
    fn enumerates_testharness_variants_and_paths() {
        let tests = fixture().tests();
        // Plain test: null url -> the path under url_base.
        let size = tests
            .iter()
            .find(|t| t.url == "/FileAPI/Blob/size.html")
            .expect("size.html");
        assert_eq!(size.kind, TestKind::Testharness);
        assert!(!size.long_timeout);
        // `.any.js` is pre-expanded: window + worker variants, no synthesize needed.
        assert!(
            tests
                .iter()
                .any(|t| t.url == "/FileAPI/Blob/x.any.html" && t.long_timeout)
        );
        let worker = tests
            .iter()
            .find(|t| t.url == "/FileAPI/Blob/x.any.worker.html")
            .expect("worker variant");
        assert!(
            worker.is_worker(),
            "worker variant flagged so the runner can skip it"
        );
        // Real manifests mix leading-slash and slashless explicit variant URLs.
        // Subset filtering must treat both as the same URL space.
        assert!(
            fixture()
                .tests_under("FileAPI/Blob")
                .iter()
                .any(|t| t.url == "FileAPI/Blob/y.any.html")
        );
        assert!(
            fixture()
                .tests_under("FileAPI/Blob/x.any.js")
                .iter()
                .any(|t| t.url == "/FileAPI/Blob/x.any.html")
        );
    }

    #[test]
    fn parses_reftest_refs_and_fuzzy() {
        let tests = fixture().tests();
        let r = tests
            .iter()
            .find(|t| t.url == "/css/ref.html")
            .expect("reftest");
        assert_eq!(r.kind, TestKind::Reftest);
        assert_eq!(
            r.refs,
            vec![("/css/ref-ref.html".to_string(), RefMatch::Equal)]
        );
        assert_eq!(r.fuzzy, Some(((0, 2), (0, 40))));
    }

    #[test]
    fn rejects_unknown_manifest_versions() {
        let error = Manifest::from_value(serde_json::json!({
            "version": 10,
            "url_base": "/",
            "items": {}
        }))
        .err()
        .expect("an unknown shape cannot define the denominator");
        assert!(
            error
                .to_string()
                .contains("unsupported WPT manifest version")
        );
    }

    #[test]
    fn rejects_malformed_variants_in_supported_buckets() {
        let error = Manifest::from_value(serde_json::json!({
            "version": 9,
            "url_base": "/",
            "items": {
                "testharness": {
                    "css": {
                        "broken.html": ["hash", 42]
                    }
                }
            }
        }))
        .err()
        .expect("a silently dropped variant would shrink the denominator");
        assert!(error.to_string().contains("variant 1"));
    }

    /// Opt-in integration test against the real ~39MB manifest (heavy; run with
    /// `cargo test -p genet-wpt -- --ignored real_manifest`).
    #[test]
    #[ignore = "loads the real ~39MB MANIFEST.json"]
    fn real_manifest_enumerates_sanely() {
        // CWD under `cargo test` is the package dir; the manifest is at the genet root.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/wpt/meta/MANIFEST.json");
        let m = Manifest::load(&path).expect("load real manifest");
        let tests = m.tests();
        assert!(
            tests.len() > 10_000,
            "expected a large corpus, got {}",
            tests.len()
        );
        assert!(
            tests.iter().any(|t| t.kind == TestKind::Testharness),
            "has testharness tests",
        );
        assert!(
            tests
                .iter()
                .any(|t| t.kind == TestKind::Reftest && !t.refs.is_empty()),
            "reftests carry refs"
        );
    }
}
