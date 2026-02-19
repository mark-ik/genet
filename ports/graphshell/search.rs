/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Graph node fuzzy search helpers.

use crate::graph::{Graph, NodeKey};
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher};

#[derive(Clone)]
struct SearchCandidate {
    key: NodeKey,
    text: String,
}

impl AsRef<str> for SearchCandidate {
    fn as_ref(&self) -> &str {
        &self.text
    }
}

/// Fuzzy-match arbitrary candidates and return them ranked by relevance.
pub(crate) fn fuzzy_match_items<T>(candidates: Vec<T>, query: &str) -> Vec<T>
where
    T: AsRef<str> + Clone,
{
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Never);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    pattern
        .match_list(candidates, &mut matcher)
        .into_iter()
        .map(|(candidate, _score)| candidate)
        .collect()
}

/// Return node keys ranked by fuzzy match quality for `query`.
pub(crate) fn fuzzy_match_node_keys(graph: &Graph, query: &str) -> Vec<NodeKey> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let candidates: Vec<SearchCandidate> = graph
        .nodes()
        .map(|(key, node)| SearchCandidate {
            key,
            text: format!("{} {}", node.title, node.url),
        })
        .collect();
    fuzzy_match_items(candidates, query)
        .into_iter()
        .map(|candidate| candidate.key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use euclid::default::Point2D;

    #[test]
    fn test_fuzzy_match_node_keys_matches_typo() {
        let mut graph = Graph::new();
        let github = graph.add_node("https://github.com".into(), Point2D::new(0.0, 0.0));
        graph.add_node("https://gitlab.com".into(), Point2D::new(20.0, 0.0));
        graph.add_node("https://example.com".into(), Point2D::new(40.0, 0.0));

        let matches = fuzzy_match_node_keys(&graph, "gthub");
        assert_eq!(matches.first().copied(), Some(github));
    }

    #[test]
    fn test_fuzzy_match_node_keys_matches_titles_and_urls() {
        let mut graph = Graph::new();
        let rust_book = graph.add_node("https://doc.rust-lang.org/book".into(), Point2D::zero());
        let cargo_ref = graph.add_node(
            "https://doc.rust-lang.org/cargo".into(),
            Point2D::new(20.0, 0.0),
        );

        if let Some(node) = graph.get_node_mut(cargo_ref) {
            node.title = "Cargo Reference".into();
        }
        if let Some(node) = graph.get_node_mut(rust_book) {
            node.title = "The Rust Book".into();
        }

        let title_matches = fuzzy_match_node_keys(&graph, "cargo ref");
        assert_eq!(title_matches.first().copied(), Some(cargo_ref));

        let url_matches = fuzzy_match_node_keys(&graph, "rust-lang book");
        assert_eq!(url_matches.first().copied(), Some(rust_book));
    }

    #[test]
    fn test_fuzzy_match_node_keys_empty_query_returns_no_matches() {
        let graph = Graph::new();
        assert!(fuzzy_match_node_keys(&graph, "").is_empty());
        assert!(fuzzy_match_node_keys(&graph, "   ").is_empty());
    }

    #[test]
    fn test_fuzzy_match_items_ranks_by_match_quality() {
        let candidates = vec![
            "alpha beta".to_string(),
            "gamma delta".to_string(),
            "alpha gamma".to_string(),
        ];
        let matches = fuzzy_match_items(candidates, "alpa");
        assert!(!matches.is_empty());
        assert_eq!(matches[0], "alpha beta");
    }
}
