// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A small, self-contained CSS selector matcher for `querySelector` /
//! `querySelectorAll` / `matches`, generic over [`LayoutDom`].
//!
//! Supported: selector lists (`a, b`), type / universal (`div`, `*`), `#id`,
//! `.class`, attribute selectors (`[a]`, `[a=v]`, `[a~=v]`), and the descendant
//! (` `) and child (`>`) combinators, in any compound combination
//! (`div.foo#bar[x] > .y`). **Not** supported (yet): pseudo-classes/elements
//! (`:hover`, `::before`), sibling combinators (`+`, `~`), and the other
//! attribute operators (`^=`, `$=`, `*=`, `|=`). The safety rule is **never
//! over-match**: any unsupported syntax makes the whole selector list match
//! nothing (an empty [`Selectors`]), rather than silently matching a prefix.
//!
//! This is a pragmatic subset, not `selectors`-crate-grade matching; the
//! eventual `web-api` layer can swap in full selector matching (the crate
//! genet-layout already uses) behind the same sink surface.

use layout_dom_api::{LayoutDom, LocalName, Namespace};

/// A parsed selector list. Empty means "matches nothing" (no selectors, or an
/// unsupported one made the whole list invalid).
pub struct Selectors(Vec<Complex>);

/// One complex selector: compound selectors joined by combinators, stored
/// left-to-right. `parts[i].0` is the combinator linking `parts[i-1]` to
/// `parts[i]`; `parts[0].0` is unused.
struct Complex {
    parts: Vec<(Combinator, Compound)>,
}

#[derive(Clone, Copy, PartialEq)]
enum Combinator {
    Descendant,
    Child,
}

/// A compound selector (no combinators): a single element's constraints.
#[derive(Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrSel>,
}

struct AttrSel {
    name: String,
    op: AttrOp,
}

enum AttrOp {
    Exists,
    Equals(String),
    Includes(String),
}

impl Selectors {
    /// `true` if `node` matches any complex selector in the list.
    pub fn matches<D: LayoutDom>(&self, dom: &D, node: D::NodeId) -> bool {
        self.0.iter().any(|c| matches_complex(dom, node, c))
    }

    /// All descendants of `scope` matching the list, in document (preorder).
    pub fn query_all<D: LayoutDom>(&self, dom: &D, scope: D::NodeId) -> Vec<D::NodeId> {
        let mut descendants = Vec::new();
        collect_descendants(dom, scope, &mut descendants);
        descendants
            .into_iter()
            .filter(|&n| self.matches(dom, n))
            .collect()
    }

    /// The first descendant of `scope` matching the list, or `None`.
    pub fn query_first<D: LayoutDom>(&self, dom: &D, scope: D::NodeId) -> Option<D::NodeId> {
        let mut descendants = Vec::new();
        collect_descendants(dom, scope, &mut descendants);
        descendants.into_iter().find(|&n| self.matches(dom, n))
    }
}

fn collect_descendants<D: LayoutDom>(dom: &D, node: D::NodeId, out: &mut Vec<D::NodeId>) {
    for child in dom.dom_children(node).collect::<Vec<_>>() {
        out.push(child);
        collect_descendants(dom, child, out);
    }
}

/// Match `node` against one complex selector, right-to-left: the node must match
/// the rightmost compound, then each preceding compound must match an
/// ancestor/parent per its combinator.
fn matches_complex<D: LayoutDom>(dom: &D, node: D::NodeId, complex: &Complex) -> bool {
    let parts = &complex.parts;
    let last = parts.len() - 1;
    if !matches_compound(dom, node, &parts[last].1) {
        return false;
    }
    let mut current = node;
    let mut i = last;
    while i > 0 {
        let combinator = parts[i].0;
        let target = &parts[i - 1].1;
        match combinator {
            Combinator::Child => {
                let Some(parent) = dom.parent(current) else {
                    return false;
                };
                if !matches_compound(dom, parent, target) {
                    return false;
                }
                current = parent;
            },
            Combinator::Descendant => {
                let mut ancestor = dom.parent(current);
                loop {
                    match ancestor {
                        Some(a) if matches_compound(dom, a, target) => {
                            current = a;
                            break;
                        },
                        Some(a) => ancestor = dom.parent(a),
                        None => return false,
                    }
                }
            },
        }
        i -= 1;
    }
    true
}

fn matches_compound<D: LayoutDom>(dom: &D, node: D::NodeId, c: &Compound) -> bool {
    // Only elements match a compound selector.
    let Some(qname) = dom.element_name(node) else {
        return false;
    };
    if let Some(tag) = &c.tag {
        if !qname.local.as_ref().eq_ignore_ascii_case(tag) {
            return false;
        }
    }
    let ns = Namespace::from("");
    if let Some(id) = &c.id {
        if dom.attribute(node, &ns, &LocalName::from("id")) != Some(id.as_str()) {
            return false;
        }
    }
    if !c.classes.is_empty() {
        let class_attr = dom
            .attribute(node, &ns, &LocalName::from("class"))
            .unwrap_or("");
        for needed in &c.classes {
            if !class_attr.split_whitespace().any(|h| h == needed) {
                return false;
            }
        }
    }
    for a in &c.attrs {
        let val = dom.attribute(node, &ns, &LocalName::from(a.name.as_str()));
        let ok = match &a.op {
            AttrOp::Exists => val.is_some(),
            AttrOp::Equals(v) => val == Some(v.as_str()),
            AttrOp::Includes(v) => val.is_some_and(|s| s.split_whitespace().any(|t| t == v)),
        };
        if !ok {
            return false;
        }
    }
    true
}

// ---- parsing --------------------------------------------------------------

/// Parse a selector list. Any unsupported construct yields an empty [`Selectors`]
/// (matches nothing) rather than a partial match.
pub fn parse(input: &str) -> Selectors {
    let mut list = Vec::new();
    for part in split_top_level(input, ',') {
        match parse_complex(part.trim()) {
            Some(c) => list.push(c),
            None => return Selectors(Vec::new()),
        }
    }
    Selectors(list)
}

/// Split on `sep` at bracket depth 0 (so commas inside `[...]` are preserved).
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '[' => {
                depth += 1;
                cur.push(ch);
            },
            ']' => {
                depth -= 1;
                cur.push(ch);
            },
            c if c == sep && depth == 0 => {
                out.push(std::mem::take(&mut cur));
            },
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

enum Raw {
    Compound(String),
    Child,
    Space,
}

fn parse_complex(s: &str) -> Option<Complex> {
    // Tokenize into compound strings + combinators, respecting bracket depth.
    let mut raws = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '[' => {
                depth += 1;
                cur.push(ch);
            },
            ']' => {
                depth -= 1;
                cur.push(ch);
            },
            c if depth == 0 && c.is_whitespace() => {
                if !cur.is_empty() {
                    raws.push(Raw::Compound(std::mem::take(&mut cur)));
                }
                raws.push(Raw::Space);
            },
            '>' if depth == 0 => {
                if !cur.is_empty() {
                    raws.push(Raw::Compound(std::mem::take(&mut cur)));
                }
                raws.push(Raw::Child);
            },
            ('+' | '~') if depth == 0 => return None, // sibling combinators unsupported
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        raws.push(Raw::Compound(cur));
    }

    // Fold into parts. A `Space` becomes a descendant combinator only between two
    // compounds; `Child` overrides an adjacent space.
    let mut parts: Vec<(Combinator, Compound)> = Vec::new();
    let mut pending: Option<Combinator> = None;
    for raw in raws {
        match raw {
            Raw::Compound(cs) => {
                let compound = parse_compound(&cs)?;
                let combinator = pending.take().unwrap_or(Combinator::Descendant);
                parts.push((combinator, compound));
            },
            Raw::Child => pending = Some(Combinator::Child),
            Raw::Space => {
                if pending.is_none() && !parts.is_empty() {
                    pending = Some(Combinator::Descendant);
                }
            },
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(Complex { parts })
}

fn parse_compound(s: &str) -> Option<Compound> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut c = Compound::default();

    // Optional leading type selector or universal.
    if i < chars.len() && (chars[i] == '*' || is_ident_start(chars[i])) {
        if chars[i] == '*' {
            i += 1;
        } else {
            let (ident, next) = read_ident(&chars, i);
            c.tag = Some(ident);
            i = next;
        }
    }

    while i < chars.len() {
        match chars[i] {
            '#' => {
                let (ident, next) = read_ident(&chars, i + 1);
                if ident.is_empty() {
                    return None;
                }
                c.id = Some(ident);
                i = next;
            },
            '.' => {
                let (ident, next) = read_ident(&chars, i + 1);
                if ident.is_empty() {
                    return None;
                }
                c.classes.push(ident);
                i = next;
            },
            '[' => {
                let close = chars[i..].iter().position(|&ch| ch == ']')? + i;
                let inner: String = chars[i + 1..close].iter().collect();
                c.attrs.push(parse_attr(&inner)?);
                i = close + 1;
            },
            _ => return None, // pseudo-classes, etc. — unsupported
        }
    }
    Some(c)
}

fn parse_attr(inner: &str) -> Option<AttrSel> {
    let inner = inner.trim();
    match inner.find('=') {
        None => {
            if inner.is_empty() {
                return None;
            }
            Some(AttrSel {
                name: inner.to_string(),
                op: AttrOp::Exists,
            })
        },
        Some(eq) => {
            let before = &inner[..eq];
            let raw_value = inner[eq + 1..].trim();
            let value = strip_quotes(raw_value).to_string();
            // The operator prefix char, if any: `~=`, or unsupported `^$*|=`.
            let (name, op) = match before.chars().last() {
                Some('~') => (before[..before.len() - 1].trim(), AttrOp::Includes(value)),
                Some('^' | '$' | '*' | '|') => return None,
                _ => (before.trim(), AttrOp::Equals(value)),
            };
            if name.is_empty() {
                return None;
            }
            Some(AttrSel {
                name: name.to_string(),
                op,
            })
        },
    }
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '-' || !ch.is_ascii()
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || !ch.is_ascii()
}

/// Read an identifier starting at `start`; returns it plus the next index.
fn read_ident(chars: &[char], start: usize) -> (String, usize) {
    let mut i = start;
    while i < chars.len() && is_ident_char(chars[i]) {
        i += 1;
    }
    (chars[start..i].iter().collect(), i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout_dom_api::{LayoutDom, LayoutDomMut, QualName};
    use genet_scripted_dom::ScriptedDom;

    // Build `<body><div id=a class="x y"><p class=x></p><span></span></div></body>`.
    // The `<body>` is created explicitly because fragment-parsing `<body>` under the
    // document root drops the wrapper; set_inner_html on the body keeps the subtree.
    fn fixture() -> ScriptedDom {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let body = dom.create_element(QualName::new(
            None,
            Namespace::from("http://www.w3.org/1999/xhtml"),
            LocalName::from("body"),
        ));
        dom.append_child(root, body);
        dom.set_inner_html(
            body,
            "<div id='a' class='x y'><p class='x'></p><span></span></div>",
        );
        dom
    }

    fn q(dom: &ScriptedDom, sel: &str) -> Vec<String> {
        let sels = parse(sel);
        sels.query_all(dom, dom.document())
            .iter()
            .map(|&n| {
                dom.element_name(n)
                    .map(|q| q.local.as_ref().to_string())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn type_id_class_and_combinators() {
        let dom = fixture();
        assert_eq!(q(&dom, "div"), vec!["div"]);
        assert_eq!(q(&dom, "#a"), vec!["div"]);
        assert_eq!(q(&dom, ".x"), vec!["div", "p"]);
        assert_eq!(q(&dom, "div.x"), vec!["div"]);
        assert_eq!(q(&dom, "div > p"), vec!["p"]);
        assert_eq!(q(&dom, "div p"), vec!["p"]);
        assert_eq!(q(&dom, "body span"), vec!["span"]);
        // selector list
        assert_eq!(q(&dom, "p, span"), vec!["p", "span"]);
        // universal under div
        assert_eq!(q(&dom, "div > *"), vec!["p", "span"]);
    }

    #[test]
    fn attribute_selectors() {
        let dom = fixture();
        assert_eq!(q(&dom, "[id]"), vec!["div"]);
        assert_eq!(q(&dom, "[id=a]"), vec!["div"]);
        assert_eq!(q(&dom, "[class~=y]"), vec!["div"]);
        assert!(q(&dom, "[id=nope]").is_empty());
    }

    #[test]
    fn unsupported_matches_nothing() {
        let dom = fixture();
        assert!(q(&dom, "div:hover").is_empty());
        assert!(q(&dom, "p + span").is_empty());
        assert!(q(&dom, "[class^=x]").is_empty());
        assert!(q(&dom, "::before").is_empty());
    }
}
