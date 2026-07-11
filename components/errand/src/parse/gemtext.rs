//! Gemtext line parser — Gemini's `text/gemini` grammar.
//!
//! Line-oriented: each line's prefix decides its kind, so the AST is a flat list
//! of [`GemLine`] (the shape Lagrange's `GmDocument` and Geopard's line model both
//! use). Grouping decisions — consecutive `* ` items into one list, consecutive
//! `> ` lines into one quote, runs of plain text into a paragraph — are left to the
//! consumer, because they belong to the consumer's model (a native viewer renders
//! line by line; a notes engine groups into blocks). The one piece of state the
//! grammar itself owns is the preformatted fence, which spans lines.
//!
//! Spec: <https://geminiprotocol.net/docs/specification.gmi>.

/// One classified gemtext line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GemLine {
    /// `# ` / `## ` / `### ` heading (level 1–3); `text` is trimmed.
    Heading { level: u8, text: String },
    /// A non-prefixed, non-blank text line (paragraph fodder, verbatim).
    Text(String),
    /// `=> url [label]`. `label` is empty when the line carried only a URL.
    /// A bare `=>` with no URL is dropped (it has no meaning and no target).
    Link { url: String, label: String },
    /// `* ` list item; `text` is trimmed.
    Item(String),
    /// `> ` quote line; `text` is trimmed.
    Quote(String),
    /// A ```` ``` ````-fenced preformatted block. `alt` is the text after the
    /// opening fence (a language / alt hint), `text` the verbatim body including
    /// trailing newlines. An unterminated fence runs to end of input.
    Pre { alt: Option<String>, text: String },
    /// A blank line. A consumer flushes a paragraph on it; list/quote runs
    /// conventionally survive a single blank.
    Blank,
}

/// Parse `body` into gemtext lines, in source order.
pub fn parse(body: &str) -> Vec<GemLine> {
    let mut out = Vec::new();
    // `Some((alt, collected))` while inside a preformatted block.
    let mut pre: Option<(Option<String>, String)> = None;

    for line in body.lines() {
        if let Some((_, text)) = pre.as_mut() {
            // Inside a fence: a line starting with ``` closes it; everything else
            // is captured verbatim (so `=>`, `#`, etc. inside stay literal).
            if line.starts_with("```") {
                let (alt, text) = pre.take().expect("inside a preformatted block");
                out.push(GemLine::Pre { alt, text });
            } else {
                text.push_str(line);
                text.push('\n');
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("```") {
            pre = Some((trimmed_or_none(rest), String::new()));
        } else if let Some(t) = line.strip_prefix("### ") {
            out.push(GemLine::Heading { level: 3, text: t.trim().to_string() });
        } else if let Some(t) = line.strip_prefix("## ") {
            out.push(GemLine::Heading { level: 2, text: t.trim().to_string() });
        } else if let Some(t) = line.strip_prefix("# ") {
            out.push(GemLine::Heading { level: 1, text: t.trim().to_string() });
        } else if let Some(rest) = line.strip_prefix("=>") {
            if let Some(link) = parse_link(rest) {
                out.push(link);
            }
        } else if let Some(t) = line.strip_prefix("* ") {
            out.push(GemLine::Item(t.trim().to_string()));
        } else if let Some(t) = line.strip_prefix("> ") {
            out.push(GemLine::Quote(t.trim().to_string()));
        } else if line.is_empty() {
            out.push(GemLine::Blank);
        } else {
            out.push(GemLine::Text(line.to_string()));
        }
    }

    // An unterminated fence flushes what it captured.
    if let Some((alt, text)) = pre.take() {
        out.push(GemLine::Pre { alt, text });
    }
    out
}

/// Parse the text after `=>` into a [`GemLine::Link`], or `None` for a bare `=>`.
/// The URL is the first whitespace-delimited token; the remainder (trimmed) is the
/// label.
fn parse_link(rest: &str) -> Option<GemLine> {
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    let (url, label) = match rest.find(char::is_whitespace) {
        Some(idx) => (rest[..idx].to_string(), rest[idx..].trim().to_string()),
        None => (rest.to_string(), String::new()),
    };
    Some(GemLine::Link { url, label })
}

fn trimmed_or_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_at_three_levels() {
        let lines = parse("# one\n## two\n### three\n");
        assert_eq!(
            lines,
            vec![
                GemLine::Heading { level: 1, text: "one".into() },
                GemLine::Heading { level: 2, text: "two".into() },
                GemLine::Heading { level: 3, text: "three".into() },
            ]
        );
    }

    #[test]
    fn link_with_and_without_label() {
        let lines = parse("=> gemini://example.test/  Example capsule\n=> gemini://example.test/page\n");
        assert_eq!(
            lines,
            vec![
                GemLine::Link {
                    url: "gemini://example.test/".into(),
                    label: "Example capsule".into(),
                },
                GemLine::Link {
                    url: "gemini://example.test/page".into(),
                    label: String::new(),
                },
            ]
        );
    }

    #[test]
    fn bare_arrow_is_dropped() {
        assert_eq!(parse("=>\n"), vec![]);
        assert_eq!(parse("=>   \n"), vec![]);
    }

    #[test]
    fn items_and_quotes_stay_line_level() {
        let lines = parse("* one\n* two\n> q1\n> q2\n");
        assert_eq!(
            lines,
            vec![
                GemLine::Item("one".into()),
                GemLine::Item("two".into()),
                GemLine::Quote("q1".into()),
                GemLine::Quote("q2".into()),
            ]
        );
    }

    #[test]
    fn preformatted_with_alt_captures_verbatim() {
        let lines = parse("```rust\nfn main() {}\n```\n");
        assert_eq!(
            lines,
            vec![GemLine::Pre {
                alt: Some("rust".into()),
                text: "fn main() {}\n".into(),
            }]
        );
    }

    #[test]
    fn preformatted_swallows_other_prefixes() {
        let lines = parse("```\n=> not-a-link\n# not-a-heading\n```\n");
        let GemLine::Pre { alt, text } = &lines[0] else {
            panic!("expected a preformatted block, got {:?}", lines);
        };
        assert_eq!(*alt, None);
        assert!(text.contains("=> not-a-link"));
        assert!(text.contains("# not-a-heading"));
    }

    #[test]
    fn unterminated_fence_runs_to_end() {
        let lines = parse("```\nstill open\n");
        assert_eq!(
            lines,
            vec![GemLine::Pre { alt: None, text: "still open\n".into() }]
        );
    }

    #[test]
    fn blank_lines_and_text_preserved_in_order() {
        let lines = parse("first\nsecond\n\nnext\n");
        assert_eq!(
            lines,
            vec![
                GemLine::Text("first".into()),
                GemLine::Text("second".into()),
                GemLine::Blank,
                GemLine::Text("next".into()),
            ]
        );
    }
}
