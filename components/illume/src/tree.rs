//! The container tree: folds, outline, and structural selection.
//!
//! jotdown's nested `Start` / `End` events form a tree of block containers. This
//! module folds the byte-offset event stream into that tree and derives the three
//! editor-structure features the plan's Phase 3 needs, all pure Rust, all from the
//! same parse the highlighter uses:
//!
//! - [`folds`] — collapsible regions (a heading's section, a list, a quote, a code
//!   block, a div) that span more than one line.
//! - [`outline`] — the heading list with levels (the gloss outline lens consumes
//!   this).
//! - [`expand_selection`] — the smallest container strictly enclosing a selection,
//!   for Alt-Up grow-selection.
//!
//! Inline containers (emphasis, links) are skipped: structural editing works on
//! the block tree. The source text is never mutated; re-run on edit, cheap at note
//! size.

use std::ops::Range;

use jotdown::{Container, Event, Parser};

/// The kind of a block container in the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A heading's section (the heading plus its content until the next
    /// same-or-higher heading) — the natural fold unit.
    Section,
    /// A heading line, carrying its level (1..=6).
    Heading(u8),
    /// A list (ordered or unordered).
    List,
    /// A single list item.
    ListItem,
    /// A block quote.
    Blockquote,
    /// A fenced code block.
    CodeBlock,
    /// A fenced div (`::: class`).
    Div,
    /// A paragraph.
    Paragraph,
    /// A table.
    Table,
}

/// A node in the document's block-container tree: its source range, kind, and
/// children, in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    /// Byte range in the source string.
    pub range: Range<usize>,
    /// The container kind.
    pub kind: NodeKind,
    /// Child containers, in document order.
    pub children: Vec<TreeNode>,
}

/// The block container kind of a jotdown container, or `None` for containers the
/// tree does not model (inline spans, table rows/cells, description-list parts).
fn node_kind_of(c: &Container) -> Option<NodeKind> {
    Some(match c {
        Container::Section { .. } => NodeKind::Section,
        Container::Heading { level, .. } => NodeKind::Heading(*level as u8),
        Container::List { .. } => NodeKind::List,
        Container::ListItem | Container::TaskListItem { .. } => NodeKind::ListItem,
        Container::Blockquote => NodeKind::Blockquote,
        Container::CodeBlock { .. } => NodeKind::CodeBlock,
        Container::Div { .. } => NodeKind::Div,
        Container::Paragraph => NodeKind::Paragraph,
        Container::Table => NodeKind::Table,
        _ => return None,
    })
}

/// Fold the source's block containers into a tree (roots in document order).
/// jotdown guarantees well-nested events, so the closing container is always the
/// stack top, and skipping untracked containers preserves the tracked nesting.
pub fn container_tree(src: &str) -> Vec<TreeNode> {
    let mut stack: Vec<(NodeKind, usize, Vec<TreeNode>)> = Vec::new();
    let mut roots: Vec<TreeNode> = Vec::new();
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(c, _) => {
                if let Some(kind) = node_kind_of(&c) {
                    stack.push((kind, range.start, Vec::new()));
                }
            }
            Event::End(c) => {
                if node_kind_of(&c).is_some() {
                    if let Some((kind, start, children)) = stack.pop() {
                        let node = TreeNode {
                            range: start..range.end,
                            kind,
                            children,
                        };
                        match stack.last_mut() {
                            Some((_, _, siblings)) => siblings.push(node),
                            None => roots.push(node),
                        }
                    }
                }
            }
            _ => {}
        }
    }
    roots
}

/// A collapsible region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    /// Byte range the fold collapses.
    pub range: Range<usize>,
    /// What kind of container it is.
    pub kind: NodeKind,
}

/// Collapsible regions: sections, lists, quotes, code blocks, and divs that span
/// more than one line. Document order, parents before children.
pub fn folds(src: &str) -> Vec<Fold> {
    fn walk(nodes: &[TreeNode], src: &str, out: &mut Vec<Fold>) {
        for n in nodes {
            let foldable = matches!(
                n.kind,
                NodeKind::Section
                    | NodeKind::List
                    | NodeKind::Blockquote
                    | NodeKind::CodeBlock
                    | NodeKind::Div
            );
            if foldable && src[n.range.clone()].contains('\n') {
                out.push(Fold {
                    range: n.range.clone(),
                    kind: n.kind,
                });
            }
            walk(&n.children, src, out);
        }
    }
    let mut out = Vec::new();
    walk(&container_tree(src), src, &mut out);
    out
}

/// A heading in the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    /// Byte range of the heading line.
    pub range: Range<usize>,
    /// Heading level (1..=6).
    pub level: u8,
    /// The heading's plain text (for the outline label).
    pub text: String,
}

/// The document's headings, in order, with levels and plain text. A flat list;
/// the consumer (the gloss outline lens) nests it by level or by URL. Derived from
/// the event stream directly, since the outline needs the heading text.
pub fn outline(src: &str) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut current: Option<(usize, u8, String)> = None;
    for (event, range) in Parser::new(src).into_offset_iter() {
        match event {
            Event::Start(Container::Heading { level, .. }, _) => {
                current = Some((range.start, level as u8, String::new()));
            }
            Event::Str(s) => {
                if let Some((_, _, text)) = current.as_mut() {
                    text.push_str(s.as_ref());
                }
            }
            Event::End(Container::Heading { .. }) => {
                if let Some((start, level, text)) = current.take() {
                    items.push(OutlineItem {
                        range: start..range.end,
                        level,
                        text,
                    });
                }
            }
            _ => {}
        }
    }
    items
}

/// The smallest block container strictly enclosing `selection` (containing it but
/// not equal to it) — the Alt-Up grow-selection target. `None` when nothing
/// encloses it (the selection already spans the whole document or more).
pub fn expand_selection(src: &str, selection: Range<usize>) -> Option<Range<usize>> {
    fn collect(nodes: &[TreeNode], out: &mut Vec<Range<usize>>) {
        for n in nodes {
            out.push(n.range.clone());
            collect(&n.children, out);
        }
    }
    let mut ranges = Vec::new();
    collect(&container_tree(src), &mut ranges);
    ranges
        .into_iter()
        .filter(|r| r.start <= selection.start && selection.end <= r.end && *r != selection)
        .min_by_key(|r| r.end - r.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Title\n\nIntro paragraph.\n\n## Section A\n\n- one\n- two\n\n> a quote\n\n```rust\nfn x() {}\n```\n";

    /// Diagnostic dump — `cargo test -p knot-editor -- --nocapture dump_tree`.
    #[test]
    fn dump_tree() {
        fn walk(nodes: &[TreeNode], src: &str, depth: usize) {
            for n in nodes {
                eprintln!(
                    "{:indent$}{:?} {:?} {:?}",
                    "",
                    n.kind,
                    n.range.clone(),
                    &src[n.range.clone()].replace('\n', "\\n"),
                    indent = depth * 2
                );
                walk(&n.children, src, depth + 1);
            }
        }
        walk(&container_tree(SAMPLE), SAMPLE, 0);
    }

    #[test]
    fn outline_lists_headings_with_levels() {
        let items = outline(SAMPLE);
        let pairs: Vec<_> = items.iter().map(|i| (i.level, i.text.as_str())).collect();
        assert_eq!(pairs, vec![(1, "Title"), (2, "Section A")]);
    }

    #[test]
    fn folds_cover_multiline_regions() {
        let f = folds(SAMPLE);
        // The code block is multi-line and foldable.
        assert!(
            f.iter().any(|fold| fold.kind == NodeKind::CodeBlock
                && SAMPLE[fold.range.clone()].contains('\n')),
            "expected a code-block fold, got {f:?}"
        );
        // The list (two items across lines) folds.
        assert!(
            f.iter().any(|fold| fold.kind == NodeKind::List),
            "expected a list fold, got {f:?}"
        );
    }

    #[test]
    fn expand_selection_grows_to_the_enclosing_container() {
        // A caret inside "one" should expand to the list item, then the list.
        let at = SAMPLE.find("one").unwrap();
        let caret = at..at;
        let item = expand_selection(SAMPLE, caret.clone()).unwrap();
        assert!(item.start <= at && at < item.end, "{item:?}");
        // Expanding again from the item grows to something strictly larger.
        let bigger = expand_selection(SAMPLE, item.clone()).unwrap();
        assert!(
            bigger.start <= item.start && item.end <= bigger.end && bigger != item,
            "expand should grow: {item:?} -> {bigger:?}"
        );
    }

    #[test]
    fn empty_document_has_no_structure() {
        assert!(container_tree("").is_empty());
        assert!(outline("").is_empty());
        assert!(folds("").is_empty());
    }
}
