// Regression guard for Nova's RegExp on the regress engine (replacing the Rust
// `regex` crate, which cannot do lookahead/lookbehind/backreferences). Diagnosed
// in docs/2026-06-02_nova_regress_regex_engine.md. These assert behaviour through
// the public engine surface so a future engine swap or Nova rebase can't silently
// regress them. Astral chars and surrogate-split match positions are exercised
// because regress (non-Unicode mode) matches per code unit.
#![cfg(not(target_arch = "wasm32"))]

use script_engine_api::ScriptEngine;
use script_engine_nova::NovaEngine;

fn eval(src: &str) -> String {
    let mut e = NovaEngine::new().unwrap();
    let v = e.eval(src).unwrap_or_else(|e| panic!("eval {src:?} errored: {e}"));
    e.value_to_string(&v).unwrap_or_else(|e| panic!("value_to_string errored: {e}"))
}

#[test]
fn backtracking_features_the_regex_crate_cannot_do() {
    // All of these threw SyntaxError under the `regex` crate.
    assert_eq!(eval(r#"String(/a(?!b)/.test("ac"))"#), "true"); // negative lookahead
    assert_eq!(eval(r#"String(/a(?=b)/.test("ab"))"#), "true"); // positive lookahead
    assert_eq!(eval(r#"String(/(?<=a)b/.test("ab"))"#), "true"); // lookbehind
    assert_eq!(eval(r#"String(/(?<!a)b/.test("cb"))"#), "true"); // negative lookbehind
    assert_eq!(eval(r#"String(/(a)\1/.test("aa"))"#), "true"); // backreference
    assert_eq!(eval(r#"String(/(?:\{x\})/.test("{x}"))"#), "true"); // literal brace
}

#[test]
fn named_groups_and_captures() {
    assert_eq!(eval(r#""2024-01".match(/(?<y>\d+)-(?<m>\d+)/).groups.y"#), "2024");
    assert_eq!(eval(r#""2024-01".match(/(?<y>\d+)-(?<m>\d+)/).groups.m"#), "01");
    assert_eq!(eval(r#""2024-01".replace(/(\d+)-(\d+)/, "$2/$1")"#), "01/2024");
    assert_eq!(eval(r#""ab".replace(/(?<g>a)/, "[$<g>]")"#), "[a]b");
    assert_eq!(
        eval(r#"JSON.stringify([..."a,b,c".matchAll(/(\w)/g)].map(m => m[1]))"#),
        r#"["a","b","c"]"#
    );
}

#[test]
fn match_index_and_lastindex_are_code_units() {
    assert_eq!(eval(r#"String("aΔo".match(/o/).index)"#), "2"); // non-ASCII before match
    assert_eq!(eval(r#"String("Δあ𐐷x".match(/x/).index)"#), "4"); // astral counts 2 units
    assert_eq!(eval(r#"String("café".search(/é/))"#), "3");
    // global lastIndex advance over an astral char (matchAll indices).
    assert_eq!(
        eval(r#"Array.from("𠮷a𠮷b𠮷".matchAll(/𠮷/g)).map(m => m.index).join(",")"#),
        "0,3,6"
    );
}

#[test]
fn replace_split_through_a_surrogate_pair_does_not_panic() {
    // A non-Unicode match can split a surrogate pair; @@replace / @@split copy
    // the inter-match segments by code unit (lone surrogate preserved), rather
    // than panicking on the absent WTF-8 byte boundary. "𝌆" is one astral char.
    assert_eq!(eval(r#"String("𝌆".replace(/(?:)/g, "X").length)"#), "5"); // X D834 X DF06 X
    assert_eq!(eval(r#"String("𝌆".split(/(?:)/).length)"#), "2");
    assert_eq!(eval(r#""ab𝌆cd".replace(/𝌆/, "_")"#), "ab_cd");
}

#[test]
fn d_flag_has_indices() {
    assert_eq!(eval(r#"JSON.stringify("abc".match(/b/d).indices[0])"#), "[1,2]");
    assert_eq!(eval(r#"JSON.stringify("zabcz".match(/(a)(b)/d).indices[1])"#), "[1,2]");
    assert_eq!(eval(r#"JSON.stringify("xab".match(/(?<g>a)/d).indices.groups.g)"#), "[1,2]");
    // an unmatched optional group's index pair is undefined.
    assert_eq!(eval(r#"String("b".match(/(a)?b/d).indices[1])"#), "undefined");
}
