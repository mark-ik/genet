# Nova WTF-8 / UTF-16 string-indexing fixes

Status: **landed in the fork (`crates/nova`, branch `serval-embedder`),
2026-06-02.** Five bugs, all upstream candidates (we are a thin fork; clean to
contribute). Verified by a serval-side end-to-end regression test
(`components/script-engine-nova/tests/wtf8_indexing_regression.rs`), nova_vm and
`small_string` unit tests, and the dom/nodes WPT sweep.

## Where this came from

The Boa<->Nova divergence sweep
([pluggable-engines plan](./2026-05-26_pluggable_engines_testharness_plan.md))
left a residual of 6 dom/nodes tests that **panicked** on Nova where Boa
completed. The shorthand at the time was "one lone-surrogate string panic." That
was wrong on two counts: there were five distinct bugs, and only one is about
surrogates. The common thread is that Nova stores strings as WTF-8 (bytes) but
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

## Upstreaming

All five are upstream-Nova bugs (the binding layer in
`components/script-engine-nova` is clean; `value_to_string` is
`to_string_lossy`-safe). We are not gating on upstream: the fixes live in the
fork now. Each is self-contained and carries a comment explaining the WTF-8 /
UTF-16 confusion, so each can be lifted to a PR against upstream Nova
independently. Files touched: `small_string/lib.rs`,
`nova_vm/.../string/data.rs`, `nova_vm/.../string_prototype.rs`,
`nova_vm/.../regexp_prototype.rs`, `nova_vm/.../regexp/abstract_operations.rs`.

## Not done (deliberate)

- The 3 residual dom/nodes divergences are subtest-count differences in the
  binding-gap tail, unrelated to string indexing.
- The regex `d` flag (`hasIndices`) capture start/end conversion is still
  unimplemented upstream (comments only in `RegExpBuiltinExec`); untouched.
- test262 is an uninitialized submodule locally, so the authoritative
  String/RegExp conformance suite was not run. Coverage rests on the regression
  test, nova_vm/small_string units, and the dom/nodes sweep.
