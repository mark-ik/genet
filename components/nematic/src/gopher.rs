/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Gopher menu engine — parses RFC 1436 gopher menus into a portable
//! document.
//!
//! A gopher menu is a sequence of tab-delimited lines, each of the form:
//!
//! ```text
//! <type><display>\t<selector>\t<host>\t<port>\r\n
//! ```
//!
//! The first character of each line is the item type. A bare `.` on its own
//! line terminates the menu.
//!
//! Item types this engine handles explicitly:
//!
//! - `i` informational text (no resource) — merged into paragraph runs
//! - `0` text file — emitted as link
//! - `1` submenu / directory — emitted as link
//! - `7` full-text search server — emitted as link
//! - `9` binary — emitted as link
//! - `g` / `I` images — emitted as link
//! - `s` sound — emitted as link
//! - `T` telnet — emitted as link
//! - `h` URL item (selector starts with `URL:`) — extracted URL emitted
//!   as link
//! - `3` server error — folded into informational paragraph text
//!
//! Unknown types are still emitted as a synthesised `gopher://` link so the
//! menu remains navigable even when the type isn't in the table above.
//!
//! References:
//! - RFC 1436 (The Internet Gopher Protocol)
//! - RFC 4266 (gopher URI scheme)

use errand::parse::gopher::{GopherKind, parse as parse_gopher};
use inker::{
    Block, DocumentProvenance, DocumentTrustState, Engine, EngineDocument, EngineError,
    EngineInput, InlineSpan,
};

/// Stable engine identifier.
pub const ENGINE_ID: &str = "nematic.gopher";

/// Gopher menu engine.
pub struct GopherEngine;

impl GopherEngine {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GopherEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for GopherEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn render(&self, input: &EngineInput) -> Result<EngineDocument, EngineError> {
        // errand parses the RFC 1436 menu into typed items (with synthesised URLs);
        // nematic folds info/error runs into paragraphs and resources into links.
        let mut blocks: Vec<Block> = Vec::new();
        let mut info_run: Vec<String> = Vec::new();

        for item in parse_gopher(&input.body) {
            match item.kind {
                GopherKind::Info => info_run.push(item.display),
                GopherKind::Error => info_run.push(format!("[error] {}", item.display)),
                _ => {
                    // Every non-info/error item carries a URL.
                    if let Some(url) = item.url {
                        flush_info(&mut info_run, &mut blocks);
                        blocks.push(link_paragraph(item.display, url));
                    }
                }
            }
        }
        flush_info(&mut info_run, &mut blocks);

        Ok(EngineDocument {
            address: input.address.clone(),
            title: None,
            content_type: input
                .content_type
                .clone()
                .unwrap_or_else(|| "application/gopher-menu".to_string()),
            lang: None,
            provenance: DocumentProvenance::for_engine(self.engine_id(), &input.address),
            trust: DocumentTrustState::Unknown,
            diagnostics: Vec::new(),
            blocks,
        })
    }
}

fn flush_info(info_run: &mut Vec<String>, blocks: &mut Vec<Block>) {
    if info_run.is_empty() {
        return;
    }
    let mut spans = Vec::with_capacity(info_run.len() * 2);
    let mut first = true;
    for line in info_run.drain(..) {
        if !first {
            spans.push(InlineSpan::SoftBreak);
        }
        spans.push(InlineSpan::Text(line));
        first = false;
    }
    blocks.push(Block::Paragraph { spans });
}

fn link_paragraph(display: String, url: String) -> Block {
    Block::Paragraph {
        spans: vec![InlineSpan::Link {
            url,
            title: None,
            spans: vec![InlineSpan::Text(display)],
            predicate: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(body: &str) -> EngineDocument {
        GopherEngine::new()
            .render(&EngineInput::new("gopher://test/", body))
            .expect("render")
    }

    fn line(t: char, display: &str, selector: &str, host: &str, port: &str) -> String {
        format!("{t}{display}\t{selector}\t{host}\t{port}\r\n")
    }

    #[test]
    fn engine_id_is_stable() {
        assert_eq!(GopherEngine::new().engine_id(), "nematic.gopher");
    }

    #[test]
    fn standard_text_item_synthesises_gopher_url() {
        let body = line('0', "Welcome text", "/welcome.txt", "example.test", "70");
        let doc = render(&body);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let InlineSpan::Link { url, .. } = &spans[0] else {
            panic!("expected link");
        };
        assert_eq!(url, "gopher://example.test/0/welcome.txt");
    }

    #[test]
    fn non_default_port_appears_in_url() {
        let body = line('1', "Sub", "/sub", "example.test", "7070");
        let doc = render(&body);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let InlineSpan::Link { url, .. } = &spans[0] else {
            panic!("expected link");
        };
        assert_eq!(url, "gopher://example.test:7070/1/sub");
    }

    #[test]
    fn url_item_extracts_url_prefix() {
        let body = line('h', "External", "URL:https://example.test/", ".", "70");
        let doc = render(&body);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let InlineSpan::Link { url, .. } = &spans[0] else {
            panic!("expected link");
        };
        assert_eq!(url, "https://example.test/");
    }

    #[test]
    fn info_lines_merge_into_one_paragraph() {
        let body = format!(
            "{}{}{}",
            line('i', "Welcome to the menu", "", "example.test", "70"),
            line('i', "More info on a second line", "", "example.test", "70"),
            line('i', "Final info line", "", "example.test", "70"),
        );
        let doc = render(&body);
        assert_eq!(doc.blocks.len(), 1);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let breaks = spans
            .iter()
            .filter(|s| matches!(s, InlineSpan::SoftBreak))
            .count();
        assert_eq!(breaks, 2);
    }

    #[test]
    fn info_then_resource_then_info_yields_three_blocks() {
        let body = format!(
            "{}{}{}",
            line('i', "header", "", "example.test", "70"),
            line('1', "submenu", "/sub", "example.test", "70"),
            line('i', "footer", "", "example.test", "70"),
        );
        let doc = render(&body);
        assert_eq!(doc.blocks.len(), 3);
    }

    #[test]
    fn period_terminator_stops_parsing() {
        let body = format!(
            "{}{}{}",
            line('i', "before", "", "example.test", "70"),
            ".\r\n",
            line('1', "should-not-appear", "/x", "example.test", "70"),
        );
        let doc = render(&body);
        assert_eq!(doc.blocks.len(), 1);
    }

    #[test]
    fn error_lines_become_info_with_prefix() {
        let body = line('3', "host unreachable", "", "example.test", "70");
        let doc = render(&body);
        let Block::Paragraph { spans } = &doc.blocks[0] else {
            panic!("expected paragraph");
        };
        let InlineSpan::Text(text) = &spans[0] else {
            panic!("expected text");
        };
        assert!(text.starts_with("[error]"), "got: {text}");
    }

    #[test]
    fn resource_with_missing_host_is_skipped() {
        // Tab-delimited but with empty host — malformed but realistic.
        let body = "1Bad item\t/sel\t\t70\r\n";
        let doc = render(body);
        assert!(doc.blocks.is_empty());
    }

    #[test]
    fn outgoing_links_collect_all_resource_urls() {
        let body = format!(
            "{}{}{}",
            line('i', "header", "", ".", "70"),
            line('1', "sub", "/sub", "example.test", "70"),
            line('h', "ext", "URL:https://example.test/", ".", "70"),
        );
        let doc = render(&body);
        let urls = doc.outgoing_links();
        assert_eq!(
            urls,
            vec!["gopher://example.test/1/sub", "https://example.test/"]
        );
    }

    #[test]
    fn dispatches_through_inker_registry() {
        use inker::EngineRegistry;
        use inker::routing::{
            EngineRouteDecision, SurfaceContract, SurfaceContractMode, SurfaceTargetId,
        };

        let mut registry = EngineRegistry::new();
        registry.register(Box::new(GopherEngine::new()));
        let decision = EngineRouteDecision {
            engine_id: ENGINE_ID.to_string(),
            surface_contract: SurfaceContract {
                target: SurfaceTargetId::new("gopher:1"),
                mode: SurfaceContractMode::CompositedTexture,
            },
        };
        let body = line('1', "Submenu", "/s", "example.test", "70");
        let doc = registry
            .dispatch(
                &decision,
                &EngineInput::new("gopher://example.test/", &body),
            )
            .expect("dispatch");
        assert_eq!(doc.outgoing_links(), vec!["gopher://example.test/1/s"]);
    }
}
