# Nova RegExp on regress (replacing the `regex` crate)

Status: **landed in the fork (`crates/nova`, branch `serval-embedder`),
2026-06-02.** Nova's `RegExp` matcher now uses **`regress`** (the ECMAScript-spec
backtracking engine, the one Boa uses) instead of the Rust **`regex`** crate.
Upstream candidate. Verified by test262 `built-ins/RegExp` + `built-ins/String`
(see numbers below), the serval regression test, nova_vm units, and the dom WPT
sweep.

## Why

`compile_pattern` fed the JS pattern **raw** into the `regex` crate, a
linear-time finite-automaton engine that by design **cannot** do lookahead,
lookbehind, or backreferences. So Nova's "RegExp" was "whatever the `regex` crate
accepts": `/a(?!b)/`, `/(?<=a)b/`, `/(a)\1/`, and Annex-B literal `{` all threw
`SyntaxError` on exec. The earlier `harness_regex_compat` shim papered over two
testharness.js regexes, but real JS hits this constantly. The premise behind
"patch regress instead of shimming" was wrong twice over: it was not regress, and
there was no translation layer to patch — the whole approach was a wrong engine.

`regress` is the right engine: it targets the ECMAScript spec, supports the
backtracking features, and is already in the serval workspace at 0.11.1 (via
Boa), so adopting it shares one regress across the ecosystem and aligns Nova's
regex with the conformance oracle.

## What changed

- **Dependency.** `regress = { version = "0.11.1", features = ["utf16"] }` in the
  nova workspace; `nova_vm`'s `regexp` feature switched from `dep:regex` to
  `dep:regress`. `regex` is no longer used.
- **`compile_pattern`** (`regexp/data.rs`). Maps JS flags (`imsuv`) to regress
  `Flags`. The pattern is compiled from **code points** in Unicode mode (`u`/`v`)
  and from **UTF-16 code units** otherwise (`from_unicode`), so a non-BMP literal
  in the pattern matches the surrogate-pair units of a ucs2 haystack.
- **`reg_exp_builtin_exec` / `reg_exp_builtin_test`** (`regexp/abstract_operations.rs`).
  Rewritten to drive regress: the subject string is fed as UTF-16 code units
  (`to_ill_formed_utf16`), via `find_from_utf16` (Unicode) or `find_from_ucs2`
  (non-Unicode). Match positions come back as **code-unit indices**, so `.index`,
  `lastIndex`, captures, and named groups need no WTF-8 byte conversion (this
  subsumes the regex-side of the earlier byte↔UTF-16 work). `lastIndex` stays in
  code units; sticky (`y`) is emulated (regress has no native sticky).
- **`d` flag (`hasIndices`).** Implemented (previously stubbed): the `indices`
  array of `[start, end]` code-unit pairs plus `indices.groups` for named
  captures, built directly from regress's capture ranges.
- **Shim retired.** `harness_regex_compat` (and its call) deleted from
  `components/script-runtime-api/lib.rs`; regress runs both former-problem
  testharness regexes (the surrogate lookahead and the arrow-body literal `{`)
  directly.

## Bugs the swap surfaced (and fixed)

regress made `RegExp` actually run, which exposed two latent bugs in code the old
engine never reached:

1. **`get_substitution` `$N` scan** (`string_prototype.rs`). The replacement-
   template loop defaulted `ref` to the *whole* remainder and only took a single
   character when `len == 1`, so a literal run before the next `$` swallowed the
   rest of the template: `"x".replace(/(x)/, "[$1]")` gave `"[$1]"`, and
   `"...".replace(..., "$1-$2")` dropped `$2`. The spec's catch-all (step h) takes
   the first code point; fixed to default to that.
2. **`@@replace` / `@@split` / `get_substitution` surrogate-split positions.**
   regress in non-Unicode mode can match at a code-unit position that splits a
   surrogate pair (e.g. an empty global match inside an astral char). The
   inter-match span extraction sliced the WTF-8 string via
   `utf8_index_(pos).unwrap()`, which has no byte boundary at a split and
   panicked. Now the spans are copied by code-unit range (`push_code_units` /
   `utf8_index_floor`), yielding the spec'd lone surrogate instead of crashing.

## test262 (built-ins/RegExp + built-ins/String, vs the pre-swap baseline)

Ran the full `built-ins/RegExp` + `built-ins/String` trees with the release
`nova_cli` against the committed `expectations.json` (no `--update`). Result:
**244 improvements, 0 regressions, 0 crashes** — every delta is a test the
baseline expected to Fail/Crash now Passing, and nothing the baseline passed
regressed. (The one transient `Fail→Crash` seen mid-refactor,
`Symbol.replace/coerce-unicode`, was the surrogate-split panic; it is fixed.)

The improvements span the features the `regex` crate could never do: lookbehind,
named groups (`groups-object`, duplicate names, unicode references), unicodeSets
(`v` flag) generated tests, regexp-modifiers, and the `Symbol.replace`/`match`/
`split` algorithms.

## Notes

- The pattern is compiled via the `&str` code-point / code-unit iterators; a
  pattern source containing a lone surrogate is lossy (rare). The haystack path
  is exact.
- Capture/segment substrings are rebuilt from code units, so lone surrogates
  survive (`String.prototype.replace` etc. on astral input).
- This is the engine half of the broader WTF-8/UTF-16 story in
  [the indexing-fixes doc](./2026-06-02_nova_wtf8_indexing_fixes.md); the String-
  method search/slice fixes there stand (they slice the WTF-8 store, independent
  of the regex engine).
