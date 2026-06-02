// Regression guard for five nova_vm WTF-8/UTF-16 indexing bugs surfaced by the
// WPT testharness runner on Nova (a string with a non-ASCII or astral character
// reaching String.prototype.{slice,substring,substr}, RegExp match/split/replace/
// search, or charCodeAt/codePointAt). Each is diagnosed in
// docs/2026-06-02_nova_wtf8_indexing_fixes.md. These assert behaviour through the
// public engine surface, so they fail loudly if a fix regresses (e.g. on a Nova
// rebase). Surrogate halves are compared via charCodeAt so the terminal never has
// to render a lone surrogate.
#![cfg(not(target_arch = "wasm32"))]

use script_engine_api::ScriptEngine;
use script_engine_nova::NovaEngine;

/// Eval `src` and stringify the result (lossily; lone surrogates -> U+FFFD).
fn eval(src: &str) -> String {
    let mut e = NovaEngine::new().unwrap();
    let v = e.eval(src).unwrap_or_else(|e| panic!("eval {src:?} errored: {e}"));
    e.value_to_string(&v).unwrap_or_else(|e| panic!("value_to_string errored: {e}"))
}

#[test]
fn regex_match_index_is_utf16_not_byte_offset() {
    // RegExpBuiltinExec stored the matcher's WTF-8 byte offset as `.index`
    // instead of converting to a UTF-16 code-unit offset (the end index already
    // was). `.index`, `search`, and every slice position derived from them were
    // wrong for a string with a non-ASCII char at or before the match. This is
    // the root cause behind the replace/split corruption below.
    assert_eq!(eval(r#"String("aΔo".match(/o/).index)"#), "2"); // not 3 (byte)
    assert_eq!(eval(r#"String("￿o".match(/o/).index)"#), "1"); // not 3
    assert_eq!(eval(r#"String("abco".match(/o/).index)"#), "3"); // ASCII baseline
    assert_eq!(eval(r#"String("aΔo".search(/o/))"#), "2");
    assert_eq!(eval(r#"String("Δあ\u{10437}x".match(/x/).index)"#), "4"); // astral = 2 units
}

#[test]
fn regex_replace_on_non_ascii() {
    // With the byte offset fixed, @@replace slices the right spans (before the
    // SmallString::utf8_index fix it had also panicked outright in Wtf8::slice).
    assert_eq!(eval(r#""￿foo".replace(/o/g, "x")"#), "\u{FFFF}fxx");
    assert_eq!(eval(r#""aΔo".replace(/o/, "x")"#), "a\u{394}x");
    assert_eq!(eval(r#""￿foo".replace(/f/, "F")"#), "\u{FFFF}Foo");
    assert_eq!(eval(r#""Kabc".replace(/b/, "B")"#), "\u{212A}aBc");
    // Heap-sized (>7 byte) non-ASCII string through replace.
    assert_eq!(eval(r#""ΔЙあ叶葉xy".replace(/x/, "_")"#), "\u{394}\u{419}\u{3042}\u{53f6}\u{8449}_y");
}

#[test]
fn regex_split_on_non_ascii() {
    // RegExp[@@split]'s final segment sliced `(byte_start, utf16_len)` — a byte
    // index paired with a UTF-16 length — and panicked on a multi-byte tail.
    assert_eq!(eval(r#"JSON.stringify("ΔЙあ叶葉".split(/,/))"#), "[\"\u{394}\u{419}\u{3042}\u{53f6}\u{8449}\"]");
    assert_eq!(eval(r#"JSON.stringify("aΔbΔc".split(/Δ/))"#), r#"["a","b","c"]"#);
}

#[test]
fn substring_slice_substr_on_non_ascii() {
    // substr previously indexed the lossy UTF-8 string with UTF-16 offsets;
    // substring/slice used utf8_index (correct only for ASCII small strings).
    assert_eq!(eval(r#""ΔЙあ叶葉".substring(1, 3)"#), "\u{419}\u{3042}");
    assert_eq!(eval(r#""ΔЙあ叶葉".slice(2)"#), "\u{3042}\u{53f6}\u{8449}");
    assert_eq!(eval(r#""ΔЙあ叶葉".substr(1, 2)"#), "\u{419}\u{3042}");
    assert_eq!(eval(r#""￿foo".substring(1)"#), "foo");
}

#[test]
fn surrogate_splitting_substring_yields_lone_surrogates() {
    // Slicing through a surrogate pair must yield a lone surrogate at the edge
    // (utf8_index returns None there; the slicing builtins unwrap-panicked).
    // "\u{1F320} test \u{1F320} TEST".substring(1,9) == "\uDF20 test \uD83C".
    assert_eq!(
        eval(r#"(function(){var r="\u{1F320} test \u{1F320} TEST".substring(1,9);
                 return r.length+":"+r.charCodeAt(0).toString(16)+".."+r.charCodeAt(r.length-1).toString(16);})()"#),
        "8:df20..d83c"
    );
    // slice through the leading pair: "\u{1F320}X".slice(1) == "\uDF20X".
    assert_eq!(
        eval(r#"(function(){var r="\u{1F320}X".slice(1);return r.length+":"+r.charCodeAt(0).toString(16);})()"#),
        "2:df20"
    );
    // substring(0,1) keeps only the high half: "\uD83C".
    assert_eq!(eval(r#""\u{1F320}X".substring(0,1).charCodeAt(0).toString(16)"#), "d83c");
}

#[test]
fn char_code_at_and_code_point_at_on_leading_astral() {
    // A string starting with a non-BMP char stores byte 0 for its high half as a
    // None entry (NonZeroUsize can't hold 0); charCodeAt(1)/codePointAt(1)
    // unwrap-panicked instead of returning the trailing low surrogate.
    assert_eq!(eval(r#"String("\u{1F320}X".charCodeAt(0))"#), "55356"); // 0xD83C high
    assert_eq!(eval(r#"String("\u{1F320}X".charCodeAt(1))"#), "57120"); // 0xDF20 low
    assert_eq!(eval(r#"String("\u{1F320}X".codePointAt(1))"#), "57120"); // lone low surrogate
    assert_eq!(eval(r#"String("\u{1F320}X".codePointAt(0))"#), "127776"); // 0x1F320 whole scalar
}
