//! The curated `logos` pack: built-in injection lexers for the common languages a
//! note carries.
//!
//! Each language is a small [`logos`] token enum compiled into a single DFA, the
//! fastest pure-Rust lexer and wasm-clean. [`default_pack`] returns an
//! [`InjectionRegistry`] pre-loaded with the pack; the host adds the precise
//! hand-lexers (quick-xml, html5ever, rhai) and any mod lexers on top. Growing the
//! pack is "add a token enum plus one `register` line".

use logos::Logos;

use crate::highlight::{Span, SyntaxKind};
use crate::injection::{InjectionLexer, InjectionRegistry};

mod lua;
mod web;

pub use lua::LuaLexer;
pub use web::{CssLexer, HtmlLexer};

/// An [`InjectionRegistry`] pre-loaded with the built-in curated pack. The host
/// extends it with hand-lexers and mod lexers (all the same trait).
pub fn default_pack() -> InjectionRegistry {
    let mut reg = InjectionRegistry::new();
    reg.register("json", Box::new(JsonLexer));
    // JSON-LD is JSON syntactically, and so are engram payloads (mere-native /
    // json-schema / json-ld are all JSON), so the JSON lexer covers them.
    reg.register("json-ld", Box::new(JsonLexer));
    reg.register("jsonld", Box::new(JsonLexer));
    reg.register("toml", Box::new(TomlLexer));
    for label in ["rust", "rs"] {
        reg.register(label, Box::new(ClikeLexer::new(RUST_KEYWORDS)));
    }
    // JS / rhai / rune are C-family: a coarse logos floor so highlighting never
    // depends on which execution engine (Nova / Boa / the rhai or rune feature) is
    // compiled in. A host may override any of these with a precise reuse-lexer.
    for label in ["js", "javascript", "mjs"] {
        reg.register(label, Box::new(ClikeLexer::new(JS_KEYWORDS)));
    }
    reg.register("rhai", Box::new(ClikeLexer::new(RHAI_KEYWORDS)));
    reg.register("rune", Box::new(ClikeLexer::new(RUNE_KEYWORDS)));
    // Lua (piccolo): its own lexer, since piccolo is not a dependency to reuse.
    reg.register("lua", Box::new(LuaLexer));
    // CSS and HTML: logos floors. cssparser / html5ever are parse-oriented and do
    // not hand back clean highlight spans, so a coarse DFA is the right tool; a
    // host may override for precision.
    reg.register("css", Box::new(CssLexer));
    for label in ["html", "htm"] {
        reg.register(label, Box::new(HtmlLexer));
    }
    reg
}

// --- JSON ---------------------------------------------------------------------

/// JSON tokens. Whitespace is skipped; anything unrecognized lexes as an error and
/// is left unstyled.
#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum JsonToken {
    #[token("true")]
    #[token("false")]
    #[token("null")]
    Keyword,
    #[regex(r#""([^"\\]|\\.)*""#)]
    Str,
    #[regex(r"-?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?")]
    Number,
    #[token("{")]
    #[token("}")]
    #[token("[")]
    #[token("]")]
    #[token(":")]
    #[token(",")]
    Punct,
}

/// JSON highlighter (a `logos` DFA): strings, numbers, the `true` / `false` /
/// `null` literals, and structural punctuation.
pub struct JsonLexer;

impl InjectionLexer for JsonLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = JsonToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(JsonToken::Keyword) => SyntaxKind::Keyword,
                Ok(JsonToken::Str) => SyntaxKind::StringLit,
                Ok(JsonToken::Number) => SyntaxKind::Number,
                Ok(JsonToken::Punct) => SyntaxKind::Punctuation,
                // Unrecognized input: leave it plain rather than mis-coloring.
                Err(_) => continue,
            };
            spans.push(Span {
                range: lex.span(),
                kind,
            });
        }
        spans
    }
}

// --- C-family (Rust, Rune, JS-fallback, …) ------------------------------------

/// Tokens shared by the curly-brace C-family. One DFA; the language is the
/// keyword set [`ClikeLexer`] checks identifiers against, so Rust, Rune, and kin
/// share this lexer. Coarse on purpose: line and block comments, double- and
/// single-quoted strings, decimal / hex numbers, identifiers, punctuation. It does
/// not model raw strings, template literals, or regex literals — a host
/// reuse-lexer (Boa for JS) covers those where precision matters.
#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum ClikeToken {
    #[regex(r"//[^\n]*", allow_greedy = true)]
    #[token("/*", block_comment)]
    Comment,
    #[regex(r#""([^"\\]|\\.)*""#)]
    #[regex(r"'([^'\\]|\\.)*'")]
    // Backtick template literals (JS). Coarse: the whole literal colors as a
    // string, interpolation included. Harmless for Rust / Rune / rhai (no
    // backtick strings there).
    #[regex(r"`([^`\\]|\\.)*`")]
    Str,
    #[regex(r"0[xX][0-9a-fA-F_]+")]
    #[regex(r"[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?")]
    Number,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Ident,
    #[regex(r"[+\-*/%=<>!&|^~?:.,;@#(){}\[\]]+")]
    Punct,
}

/// Consume a `/* … */` block comment from the lexer's current position (just past
/// the opening `/*`) to the closing `*/`, or to end of input if unterminated.
fn block_comment(lex: &mut logos::Lexer<ClikeToken>) {
    let rest = lex.remainder();
    let len = rest.find("*/").map(|i| i + 2).unwrap_or(rest.len());
    lex.bump(len);
}

/// A C-family highlighter parameterized by its keyword set. One per language
/// (`ClikeLexer::new(RUST_KEYWORDS)`): identifiers in the set color as keywords,
/// the rest stay plain.
pub struct ClikeLexer {
    keywords: &'static [&'static str],
}

impl ClikeLexer {
    pub const fn new(keywords: &'static [&'static str]) -> Self {
        Self { keywords }
    }
}

impl InjectionLexer for ClikeLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = ClikeToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(ClikeToken::Comment) => SyntaxKind::Comment,
                Ok(ClikeToken::Str) => SyntaxKind::StringLit,
                Ok(ClikeToken::Number) => SyntaxKind::Number,
                Ok(ClikeToken::Punct) => SyntaxKind::Punctuation,
                Ok(ClikeToken::Ident) if self.keywords.contains(&lex.slice()) => {
                    SyntaxKind::Keyword
                }
                // A plain identifier or unrecognized input: leave it unstyled.
                Ok(ClikeToken::Ident) | Err(_) => continue,
            };
            spans.push(Span {
                range: lex.span(),
                kind,
            });
        }
        spans
    }
}

/// Rust keywords (including reserved plus `async` / `await` / `dyn`).
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

/// JavaScript keywords. The coarse C-family floor: engine-independent, so JS
/// highlighting does not depend on which VM (Nova / Boa) is compiled in. A host
/// may override with a precise reuse-lexer (Boa's / Nova's / oxc's) where present.
const JS_KEYWORDS: &[&str] = &[
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Rhai keywords. A C-family floor like JS, so rhai highlights even in a build
/// where the rhai engine itself is feature-gated out.
const RHAI_KEYWORDS: &[&str] = &[
    "as", "break", "catch", "const", "continue", "do", "else", "false", "fn", "for", "global",
    "if", "import", "in", "let", "loop", "private", "return", "switch", "this", "throw", "true",
    "try", "until", "while", "yield",
];

/// Rune keywords (Rust-shaped). Floor lexer; the rune engine need not be present.
const RUNE_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "return",
    "select", "self", "static", "struct", "super", "true", "use", "while", "yield",
];

// --- TOML ---------------------------------------------------------------------

/// TOML tokens: `#` comments, quoted strings, numbers, the `true` / `false`
/// literals, bare keys, and structural punctuation. Coarse: table headers and
/// dotted keys read as bare keys plus punctuation, enough to make a config
/// legible.
#[derive(Logos, Debug, Clone, Copy, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
enum TomlToken {
    #[regex(r"#[^\n]*", allow_greedy = true)]
    Comment,
    #[token("true")]
    #[token("false")]
    Bool,
    #[regex(r#""([^"\\]|\\.)*""#)]
    #[regex(r"'[^'\n]*'")]
    Str,
    #[regex(r"[+-]?[0-9][0-9_]*(\.[0-9_]+)?([eE][+-]?[0-9]+)?")]
    Number,
    #[regex(r"[A-Za-z_][A-Za-z0-9_-]*")]
    Key,
    #[regex(r"[=\[\]{}.,]+")]
    Punct,
}

/// TOML highlighter (a `logos` DFA).
pub struct TomlLexer;

impl InjectionLexer for TomlLexer {
    fn lex(&self, inner: &str) -> Vec<Span> {
        let mut spans = Vec::new();
        let mut lex = TomlToken::lexer(inner);
        while let Some(result) = lex.next() {
            let kind = match result {
                Ok(TomlToken::Comment) => SyntaxKind::Comment,
                Ok(TomlToken::Bool) => SyntaxKind::Keyword,
                Ok(TomlToken::Str) => SyntaxKind::StringLit,
                Ok(TomlToken::Number) => SyntaxKind::Number,
                Ok(TomlToken::Punct) => SyntaxKind::Punctuation,
                // Bare keys stay plain.
                Ok(TomlToken::Key) | Err(_) => continue,
            };
            spans.push(Span {
                range: lex.span(),
                kind,
            });
        }
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_lexer_classifies_tokens() {
        let src = r#"{"k": [true, 42, null]}"#;
        let spans = JsonLexer.lex(src);
        let has = |k: SyntaxKind| spans.iter().any(|s| s.kind == k);
        assert!(has(SyntaxKind::StringLit), "string: {spans:?}");
        assert!(has(SyntaxKind::Number), "number: {spans:?}");
        assert!(has(SyntaxKind::Keyword), "true/null: {spans:?}");
        assert!(has(SyntaxKind::Punctuation), "braces: {spans:?}");
        // The string token covers its quotes.
        let s = spans
            .iter()
            .find(|s| s.kind == SyntaxKind::StringLit)
            .unwrap();
        assert_eq!(&src[s.range.clone()], "\"k\"");
    }

    #[test]
    fn default_pack_has_json() {
        let reg = default_pack();
        assert!(reg.has("json"));
        assert!(reg.has("JSON")); // case-insensitive
        assert!(!reg.has("python"));
    }

    #[test]
    fn rust_lexer_colors_keywords_strings_numbers_comments() {
        let lexer = ClikeLexer::new(RUST_KEYWORDS);
        let src = "fn main() { let x = 42; /* c */ let s = \"hi\"; }";
        let slices = |k: SyntaxKind| {
            lexer
                .lex(src)
                .into_iter()
                .filter(move |s| s.kind == k)
                .map(|s| src[s.range].to_string())
                .collect::<Vec<_>>()
        };
        let kw = slices(SyntaxKind::Keyword);
        assert!(kw.contains(&"fn".to_string()), "keywords: {kw:?}");
        assert!(kw.contains(&"let".to_string()), "keywords: {kw:?}");
        assert!(slices(SyntaxKind::Number).contains(&"42".to_string()));
        assert!(slices(SyntaxKind::StringLit).contains(&"\"hi\"".to_string()));
        assert!(slices(SyntaxKind::Comment).contains(&"/* c */".to_string()));
    }

    #[test]
    fn toml_lexer_colors_comment_string_number_bool() {
        let src = "# c\nkey = \"v\"\nn = 3\nb = true";
        let spans = TomlLexer.lex(src);
        let has = |k: SyntaxKind, t: &str| {
            spans
                .iter()
                .any(|s| s.kind == k && &src[s.range.clone()] == t)
        };
        assert!(has(SyntaxKind::Comment, "# c"), "{spans:?}");
        assert!(has(SyntaxKind::StringLit, "\"v\""), "{spans:?}");
        assert!(has(SyntaxKind::Number, "3"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "true"), "{spans:?}");
    }

    #[test]
    fn default_pack_registers_the_batch() {
        let reg = default_pack();
        for label in [
            "json",
            "json-ld",
            "toml",
            "rust",
            "rs",
            "js",
            "javascript",
            "rhai",
            "rune",
            "lua",
            "css",
            "html",
            "htm",
        ] {
            assert!(reg.has(label), "missing {label}");
        }
    }

    #[test]
    fn js_floor_is_engine_independent() {
        // JS highlighting must survive whichever VM (Nova / Boa) is compiled in;
        // the pack ships a coarse C-family floor, no engine required.
        let reg = default_pack();
        let src = "const s = `hi ${x}`; // c\nfunction f() { return 1; }";
        let spans = reg.lex("js", src).unwrap();
        let has = |k: SyntaxKind, t: &str| {
            spans
                .iter()
                .any(|s| s.kind == k && &src[s.range.clone()] == t)
        };
        assert!(has(SyntaxKind::Keyword, "const"), "{spans:?}");
        assert!(has(SyntaxKind::Keyword, "function"), "{spans:?}");
        assert!(has(SyntaxKind::Comment, "// c"), "{spans:?}");
        assert!(
            spans.iter().any(|s| s.kind == SyntaxKind::StringLit),
            "template literal should color as a string: {spans:?}"
        );
    }
}
