# Nova WTF-8 / UTF-16 string-indexing fixes

Status: **landed in the fork (`crates/nova`, branch `serval-embedder`),
2026-06-02.** Six numbered bugs plus an audit pass that closed the rest of the
family (the position-taking search methods), all upstream candidates (we are a
thin fork; clean to contribute). Verified by a serval-side end-to-end regression
test (`components/script-engine-nova/tests/wtf8_indexing_regression.rs`), nova_vm
and `small_string` unit tests, the dom/nodes WPT sweep, and a test262
String/RegExp pass (results below).

## Where this came from

The Boa<->Nova divergence sweep
([pluggable-engines plan](./2026-05-26_pluggable_engines_testharness_plan.md))
left a residual of 6 dom/nodes tests that **panicked** on Nova where Boa
completed. The shorthand at the time was "one lone-surrogate string panic." That
was wrong on two counts: there were six distinct bugs (five found via the WPT
sweep, a sixth via the test262 pass below), and only one is about surrogates. The
common thread is that Nova stores strings as WTF-8 (bytes) but
JS addresses them in UTF-16 code units, and several places confused the two.

The runner swallows per-test panics (`ERROR panic`), so the panic site was found
by re-running the 6 files with the panic hook capturing location + backtrace, and
by a JS-level probe through the engine. Method: runtime first, not static
tracing.

## Result

- dom/nodes panics: **6 -> 0**.
- Boa<->Nova behavioural divergence (status + subtest count): **10 -> 3**.
- Nova dom/nodes subtests: **1640/5280** (Boa 1646/5325). The 6 ex-panicking
  files now match Boa exactly (e.g. CharacterData-surrogates FAIL 2/8,
  createElement FAIL 39/147, getElementsByClassName-24 PASS).
- The 3 remaining divergences (Document-createElement-namespace, Node-parentNode,
  ParentNode-replaceChildren) are subtest-**count** differences, not panics and
  not string-indexing related. They are the engine-neutral binding-gap tail and
  out of scope here.

Bug 5 is the broadest: it silently corrupted `.index` / `search` / regex
`replace` / `split` results for *any* string with a non-ASCII character at or
before a match, independent of the WPT harness.

## The five bugs

Severity reflects blast radius (how much normal JS it breaks), not just whether
it panics.

### 1. `SmallString::utf8_index` returned the code-point ordinal as the byte index

`small_string/lib.rs`. The loop was
`for (idx, ch) in self.as_wtf8().code_points().enumerate() { ... return Some(idx) }`
— `enumerate()` yields the code-point ordinal (0, 1, 2, ...), returned as if it
were a WTF-8 byte offset. Correct only for ASCII (where the two coincide), so the
ASCII fast-path masked it. For any non-ASCII inline string (<= 7 bytes) the
returned byte index was short by the multi-byte slack, landing mid-character and
panicking in `Wtf8::slice` downstream.

Fix: track the running byte offset alongside the running UTF-16 index, adding
each code point's WTF-8 length (`code_point_wtf8_len`, a small helper; lone
surrogates are 3 bytes). Returns `None` on a surrogate-pair split, as before.
Unit test: `utf8_index_maps_utf16_units_to_byte_offsets`.

### 2. `RegExp.prototype[@@split]` mixed a byte index with a UTF-16 length

`regexp_prototype.rs`, the final-segment slice. It computed
`s.as_wtf8_(agent).slice(p_utf8, size)` where `p_utf8` is a WTF-8 byte offset but
`size` is the string's UTF-16 length. The pair runs to the end, so the slice end
was simply wrong (and panicked on a multi-byte tail). Fix: `slice_from(p_utf8)`.
The per-segment slice earlier in the function already converts both ends
correctly and was left alone.

### 3. `slice` / `substring` / `substr` panicked on surrogate-splitting bounds

`string_prototype.rs`. `String::utf8_index` returns `None` when a UTF-16 index is
the latter half of a surrogate pair (the astral scalar is one 4-byte sequence,
not two 3-byte surrogate sequences, so no byte boundary exists). The three
slicing builtins `.unwrap()`-ed it. (Separately, `substr` indexed the lossy UTF-8
string with raw UTF-16 offsets, broken for all non-ASCII.) Per spec, slicing
through a pair yields a **lone surrogate** at the edge.

Fix: a shared `wtf8_substring(agent, s, from, to, gc)` helper. It byte-slices the
clean interior and synthesizes the at-most-one lone surrogate at each split edge
(low surrogate when `from` splits, high surrogate when `to` splits). `slice`,
`substring`, and `substr` now route through it. Verified:
`"\u{1F320} test \u{1F320} TEST".substring(1,9)` returns the 8-unit
`"\uDF20 test \uD83C"`.

### 4. `char_code_at` / `code_point_at` panicked on a string starting with a non-BMP char

`string/data.rs`. The UTF-16->byte map stores byte offsets as `Option<NonZeroUsize>`,
using `None` both for a surrogate-pair latter half and (unavoidably) for byte
offset 0. The latter-half branch did `mapping[idx - 1].unwrap()` to find the
former half's byte offset; for a string *beginning* with a surrogate pair that
former half is at byte 0, stored as `None`, so `.unwrap()` panicked.
`"\u{1F320}X".charCodeAt(1)` is enough to trigger it. Fix: map `None` back to 0
(`map_or(0, NonZeroUsize::get)`) in both `char_code_at` and `code_point_at`. The
`small_string` equivalents iterate and were already correct.

### 5. (root cause) regex match `.index` was a WTF-8 byte offset, not a UTF-16 index

`regexp/abstract_operations.rs`, `RegExpBuiltinExec`. The matcher (`regress`)
runs over the string's WTF-8 bytes and returns byte offsets. The code converted
the match **end** to UTF-16 (`utf16_index_`) for `lastIndex`, but stored the
match **start** (`full_match.start()`) directly as the result's `.index`
property. So `.index`, `String.prototype.search`, and every position the
`@@replace` / `@@split` algorithms derive from `.index` were byte-based: correct
for ASCII, wrong by the multi-byte slack otherwise.

This is what actually made `@@replace` mis-slice (e.g. `"a\u{394}o".replace(/o/,"x")`
produced `"a\u{394}ox"`); the byte-typed position was then fed back through
`utf8_index_`, double-counting. It also panicked when the byte offset hit
mid-character (before fix 1). Fix: one line, convert the start with
`s.utf16_index_(...)` exactly as the end already was. Tests:
`regex_match_index_is_utf16_not_byte_offset`, `regex_replace_on_non_ascii`,
`regex_split_on_non_ascii`.

### 6. `reg_exp_builtin_exec` bounded a UTF-16 `lastIndex` by the byte length

`regexp/abstract_operations.rs`. Found by the test262 pass, not the WPT sweep:
fixing bug 5 let `matchAll-v-u-flag` get past its early assertions and reach an
empty-match case that exposed this. To map the regex's `lastIndex` (a UTF-16
index) to a byte offset for the matcher, the guard was
`if last_index > s.len_(agent)` — `len_` is the WTF-8 **byte** length. A
fullUnicode empty-match `matchAll` advances `lastIndex` one past the end, so a
UTF-16 index in `(utf16_len, byte_len]` slipped through and indexed the
UTF-16->byte map out of bounds (panic in `String::utf8_index`). A non-BMP string
is needed for the two lengths to differ. Fix: bound by `s.utf16_len_(agent)`, and
when past the end push the value past the byte length so the existing length
guard fails the match. Test: `regex_exec_lastindex_past_end_does_not_oob`.

## Closing the family (position-taking search methods)

The six bugs above were the sites the WPT and test262 corpora *reach*. An audit
of every `utf8_index_` caller (and every UTF-16-index-used-as-byte-offset) across
the text-processing builtins found a tail of the same defect class that the test
corpora do not exercise, but ordinary JS does. `"\u{1F320}x".indexOf("x", 1)`
(and `lastIndexOf` / `includes` / `startsWith` / `endsWith` with a position that
bisects a surrogate pair) panicked live; `includes` and `startsWith` also indexed
the lossy UTF-8 string with a raw UTF-16 position, panicking on any non-ASCII
before the search position (e.g. `"caf\u{e9}".includes("x", 3)`).

These five methods only need a UTF-16 position mapped to a byte offset to bound a
search region, so they do not need the full `wtf8_substring`. Two helpers cover
them: `utf8_index_ceil` (clamp to the string, round a surrogate split *up* to the
boundary after the pair) for the forward-search start of `indexOf`/`includes`/
`startsWith` and the prefix end of `endsWith`; `utf8_index_floor` (round *down*)
for `lastIndexOf`, where a match start must stay `<= pos`. `lastIndexOf` also
byte-slices its upper bound instead of `Wtf8::slice_to`, which panicked when
`pos + searchLen` landed inside a multi-byte char (`"\u{394}\u{394}".lastIndexOf("x", 1)`).

Rounding is exactly correct for every non-split position (including all
non-ASCII, the common case) because a search at a clean boundary is unaffected,
and for a pair-bisecting position because the lone surrogate at the split is not
byte-addressable in the fused-pair storage, so no normal-text match can begin
there anyway. The audit also confirmed the remaining `utf8_index_` call sites are
safe: the regexp `@@replace`/`@@split` spans take positions from regex matches,
which (after bug 5) are always code-point boundaries; `string_pad` already uses
`unwrap_or`; `get_substitution` takes a match position.

This pass is verified by direct probe (19 cases: split positions, non-ASCII
positions, out-of-range, the `slice_to` mid-char case — all correct, none panic)
and the regression test `search_methods_with_position_args_on_non_ascii`. The
test262 numbers below are unchanged by it, because the String/RegExp corpus does
not pass surrogate-bisecting or non-ASCII position arguments to these methods;
the value here is a closed ordinary-JS panic surface, not a test delta.

## test262 (built-ins/String + built-ins/RegExp)

Ran the full `built-ins/String` and `built-ins/RegExp` trees with the release
`nova_cli`, against the committed `expectations.json` (the pre-fix baseline; no
`--update`). Result: **6 improvements, 0 regressions** — every delta is a test
the baseline expected to Fail/Crash now Passing, and nothing the baseline passed
regressed:

- `RegExp/S15.10.2.7_A2_T1` (Fail->Pass) — `/\w{3}\d?/.exec("CE￿L￝box127")`
  expects `.index === 5`; pre-fix it returned the byte offset 9. The cleanest
  confirmation of bug 5.
- `RegExp/prototype/exec/regexp-builtin-exec-v-u-flag`, plus String.prototype
  `search/regexp-prototype-search-v-flag`, `search/regexp-prototype-search-v-u-flag`,
  `matchAll/regexp-prototype-matchAll-v-u-flag` (all Fail->Pass), and
  `replace/regexp-prototype-replace-v-u-flag` (Crash->Pass). The matchAll one is
  the test that drove out bug 6 (Fail -> Crash under fix 5 alone -> Pass with 6).

The String/prototype + RegExp/prototype subset alone: 1551 tests, 1436 pass,
0 regressions.

## Upstreaming

All six are upstream-Nova bugs (the binding layer in
`components/script-engine-nova` is clean; `value_to_string` is
`to_string_lossy`-safe). We are not gating on upstream: the fixes live in the
fork now. Each is self-contained and carries a comment explaining the WTF-8 /
UTF-16 confusion, so each can be lifted to a PR against upstream Nova
independently. Files touched: `small_string/lib.rs`,
`nova_vm/.../string/data.rs`, `nova_vm/.../string_prototype.rs`,
`nova_vm/.../regexp_prototype.rs`, `nova_vm/.../regexp/abstract_operations.rs`
(bugs 5 and 6).

## Not done (deliberate)

- The 3 residual dom/nodes divergences are subtest-count differences in the
  binding-gap tail, unrelated to string indexing.
- The regex `d` flag (`hasIndices`) capture start/end conversion is still
  unimplemented upstream (comments only in `RegExpBuiltinExec`); untouched.
- The test262 pass covered `built-ins/String` + `built-ins/RegExp`. The wider
  `language/` and other trees that lean on these methods were not run; the
  regression test, nova_vm/small_string units, and the dom/nodes sweep cover the
  rest.
