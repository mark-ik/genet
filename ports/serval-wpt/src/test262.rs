//! test262 runner core: frontmatter parsing + test-script assembly.
//!
//! test262 (the ECMAScript conformance suite, vendored at
//! `tests/wpt/tests/third_party/test262`) is where the Nova-vs-Boa gap lives —
//! WPT excludes ECMAScript, and `compare` showed Boa/Nova at parity on WPT, so
//! Nova's actual worklist is here. A test262 test is a `.js` file with a
//! `/*--- … ---*/` YAML frontmatter declaring `includes:` (extra harness files),
//! `flags:` (raw / onlyStrict / noStrict / module / async), `negative:`
//! (expected error), and `features:`. Running it means assembling the harness
//! (`assert.js` + `sta.js`, unless `raw`) + the declared includes + the test,
//! optionally under `"use strict"`, then evaluating it: a positive test passes
//! iff it does not throw; a negative test passes iff it throws the declared error.
//!
//! This module is the engine-independent core (parse + assemble); the run path
//! (eval + pass/fail, per-engine) wires it in [`crate::main`].

#![allow(dead_code)] // run path + `test262` command land in the next slice.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The `flags: [...]` set that shapes how a test is run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// No harness, no strict-wrapping: run the source exactly as-is.
    pub raw: bool,
    /// Run only in strict mode (a `"use strict"` prefix).
    pub only_strict: bool,
    /// Run only in sloppy mode.
    pub no_strict: bool,
    /// An ES module (different eval entry); deferred by the v1 run path.
    pub module: bool,
    /// Async: completion is signalled via `$DONE` (needs `doneprintHandle.js`).
    pub r#async: bool,
}

/// A `negative:` block: the test is expected to throw `error_type` at `phase`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negative {
    /// `parse` | `resolution` | `runtime`.
    pub phase: String,
    /// The expected error constructor name (e.g. `SyntaxError`, `TypeError`).
    pub error_type: String,
}

/// Parsed test262 frontmatter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Test262Meta {
    pub includes: Vec<String>,
    pub flags: Flags,
    pub negative: Option<Negative>,
    pub features: Vec<String>,
}

/// Parse the `/*--- … ---*/` frontmatter. Absent frontmatter yields the default
/// (positive, sloppy+strict, no includes). A line-based parse of the subset of
/// YAML test262 uses (flow lists for `includes`/`flags`/`features`, a small block
/// mapping for `negative`); not a general YAML reader.
pub fn parse_meta(src: &str) -> Test262Meta {
    let Some(block) = frontmatter(src) else {
        return Test262Meta::default();
    };
    let mut meta = Test262Meta::default();
    let lines: Vec<&str> = block.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("includes:") {
            meta.includes = parse_list(rest, &lines, &mut i);
        } else if let Some(rest) = trimmed.strip_prefix("features:") {
            meta.features = parse_list(rest, &lines, &mut i);
        } else if let Some(rest) = trimmed.strip_prefix("flags:") {
            for flag in parse_list(rest, &lines, &mut i) {
                match flag.as_str() {
                    "raw" => meta.flags.raw = true,
                    "onlyStrict" => meta.flags.only_strict = true,
                    "noStrict" => meta.flags.no_strict = true,
                    "module" => meta.flags.module = true,
                    "async" => meta.flags.r#async = true,
                    _ => {},
                }
            }
        } else if trimmed.starts_with("negative:") {
            // The following more-indented lines carry `phase:` and `type:`.
            let base_indent = indent_of(line);
            let (mut phase, mut error_type) = (String::new(), String::new());
            while i + 1 < lines.len() && indent_of(lines[i + 1]) > base_indent {
                i += 1;
                let t = lines[i].trim();
                if let Some(v) = t.strip_prefix("phase:") {
                    phase = v.trim().to_string();
                } else if let Some(v) = t.strip_prefix("type:") {
                    error_type = v.trim().to_string();
                }
            }
            if !error_type.is_empty() {
                meta.negative = Some(Negative { phase, error_type });
            }
        }
        i += 1;
    }
    meta
}

/// Extract the text between `/*---` and `---*/`.
fn frontmatter(src: &str) -> Option<&str> {
    let start = src.find("/*---")? + "/*---".len();
    let end = src[start..].find("---*/")? + start;
    Some(&src[start..end])
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Parse a YAML list that is either an inline flow list (`[a, b]` on `rest`, the
/// text after the key) or a block list (`- a` / `- b` on the following lines).
fn parse_list(rest: &str, lines: &[&str], i: &mut usize) -> Vec<String> {
    let rest = rest.trim();
    if rest.starts_with('[') {
        return rest
            .trim_matches(|c| c == '[' || c == ']')
            .split(',')
            .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    // Block list: consume following `- item` lines (more-indented).
    let base = indent_of(lines[*i]);
    let mut out = Vec::new();
    while *i + 1 < lines.len() {
        let next = lines[*i + 1];
        if indent_of(next) <= base {
            break;
        }
        let t = next.trim();
        if let Some(item) = t.strip_prefix("- ") {
            out.push(item.trim().trim_matches(['"', '\'']).to_string());
            *i += 1;
        } else {
            break;
        }
    }
    out
}

/// The test262 harness directory (`sta.js` + `assert.js` are loaded eagerly; the
/// `includes:` files are read on demand).
pub struct Harness {
    dir: PathBuf,
    assert_js: String,
    sta_js: String,
}

impl Harness {
    pub fn load(harness_dir: &Path) -> io::Result<Harness> {
        Ok(Harness {
            dir: harness_dir.to_path_buf(),
            assert_js: fs::read_to_string(harness_dir.join("assert.js"))?,
            sta_js: fs::read_to_string(harness_dir.join("sta.js"))?,
        })
    }

    fn include(&self, name: &str) -> io::Result<String> {
        fs::read_to_string(self.dir.join(name))
    }

    /// Assemble the runnable script: (strict prefix) + `assert.js` + `sta.js` +
    /// (`doneprintHandle.js` if async) + each include + the test. A `raw` test is
    /// returned verbatim (no harness, no strict-wrapping).
    pub fn assemble(&self, test_src: &str, meta: &Test262Meta, strict: bool) -> io::Result<String> {
        if meta.flags.raw {
            return Ok(test_src.to_string());
        }
        let mut out = String::new();
        if strict {
            out.push_str("\"use strict\";\n");
        }
        out.push_str(&self.assert_js);
        out.push('\n');
        out.push_str(&self.sta_js);
        out.push('\n');
        if meta.flags.r#async {
            out.push_str(&self.include("doneprintHandle.js")?);
            out.push('\n');
        }
        for inc in &meta.includes {
            out.push_str(&self.include(inc)?);
            out.push('\n');
        }
        out.push_str(test_src);
        Ok(out)
    }

    /// The harness preamble *without* the test body (`assert.js` + `sta.js` +
    /// `doneprintHandle.js` if async + the includes). Used by the module + async run
    /// paths, which evaluate this as a sloppy script to install the harness globals
    /// (`assert`, `Test262Error`, …) and then run the test separately (a module, or a
    /// script whose `$DONE` the harness reports). No strict prefix: the globals must
    /// land on `globalThis` so the test (module or otherwise) can see them.
    pub fn preamble(&self, meta: &Test262Meta) -> io::Result<String> {
        let mut out = String::new();
        out.push_str(&self.assert_js);
        out.push('\n');
        out.push_str(&self.sta_js);
        out.push('\n');
        if meta.flags.r#async {
            out.push_str(&self.include("doneprintHandle.js")?);
            out.push('\n');
        }
        for inc in &meta.includes {
            out.push_str(&self.include(inc)?);
            out.push('\n');
        }
        Ok(out)
    }
}

/// Which strict-mode variants to run for a test: `raw`/`module` once
/// (non-strict-wrapped), `onlyStrict`/`noStrict` once in that mode, otherwise
/// both. `true` = run under `"use strict"`.
pub fn strict_variants(flags: &Flags) -> Vec<bool> {
    if flags.raw || flags.module {
        return vec![false];
    }
    match (flags.only_strict, flags.no_strict) {
        (true, _) => vec![true],
        (_, true) => vec![false],
        _ => vec![false, true],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_includes_and_features() {
        let src = "/*---\nesid: sec-array.prototype.at\ndescription: >\n  multi\n  line\nincludes: [resizableArrayBufferUtils.js]\nfeatures: [resizable-arraybuffer]\n---*/\nvar x = 1;";
        let m = parse_meta(src);
        assert_eq!(m.includes, vec!["resizableArrayBufferUtils.js"]);
        assert_eq!(m.features, vec!["resizable-arraybuffer"]);
        assert!(m.negative.is_none());
        assert_eq!(
            strict_variants(&m.flags),
            vec![false, true],
            "no flag -> both modes"
        );
    }

    #[test]
    fn parses_negative_and_only_strict() {
        let src = "/*---\nes5id: 10.5-1gs\nnegative:\n  phase: parse\n  type: SyntaxError\nflags: [onlyStrict]\n---*/\nwith({}){}";
        let m = parse_meta(src);
        assert_eq!(
            m.negative,
            Some(Negative {
                phase: "parse".into(),
                error_type: "SyntaxError".into()
            })
        );
        assert!(m.flags.only_strict);
        assert_eq!(strict_variants(&m.flags), vec![true]);
    }

    #[test]
    fn raw_runs_once_unwrapped() {
        let src = "/*---\nflags: [raw]\n---*/\nthrow 0;";
        let m = parse_meta(src);
        assert!(m.flags.raw);
        assert_eq!(strict_variants(&m.flags), vec![false]);
    }

    #[test]
    fn no_frontmatter_is_default() {
        let m = parse_meta("var x = 1;");
        assert_eq!(m, Test262Meta::default());
    }

    #[test]
    fn assemble_wraps_strict_and_skips_for_raw() {
        // A stub harness via a temp dir.
        let dir = std::env::temp_dir().join("serval-test262-asm");
        let _ = fs::create_dir_all(&dir);
        fs::write(dir.join("assert.js"), "/*assert*/").unwrap();
        fs::write(dir.join("sta.js"), "/*sta*/").unwrap();
        let h = Harness::load(&dir).unwrap();

        let meta = parse_meta("/*---\n---*/\ncode;");
        let strict = h.assemble("code;", &meta, true).unwrap();
        assert!(strict.starts_with("\"use strict\";\n"));
        assert!(
            strict.contains("/*assert*/")
                && strict.contains("/*sta*/")
                && strict.ends_with("code;")
        );

        let raw_meta = parse_meta("/*---\nflags: [raw]\n---*/\ncode;");
        assert_eq!(h.assemble("code;", &raw_meta, false).unwrap(), "code;");
    }

    /// Opt-in: parse a real test262 test (run with `--ignored real_test262`).
    #[test]
    #[ignore = "reads a real test262 file"]
    fn real_test262_parses() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/wpt/tests/third_party/test262/test/language/types/boolean");
        let file = fs::read_dir(&p)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "js"))
            .expect("a boolean test");
        let src = fs::read_to_string(&file).unwrap();
        let _ = parse_meta(&src); // must not panic
        let harness = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/wpt/tests/third_party/test262/harness");
        assert!(Harness::load(&harness).is_ok(), "harness loads");
    }
}
