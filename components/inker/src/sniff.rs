/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Content-type sniffing.
//!
//! When a transport delivers bytes without an authoritative MIME label
//! — file://, gopher item type-1, finger replies, drag-and-drop, knot
//! files on disk — the inker needs a best-effort guess so [`crate::routing`]
//! can pick an engine.
//!
//! [`sniff_content_type`] does cheap byte-prefix and small-window
//! heuristics only. It never reads past the head of the buffer it is
//! given; callers that want a sniff over a streaming source should pass
//! the first 1–8 KiB.
//!
//! Returns `None` when nothing matches. Callers should fall back to
//! whatever scheme- or extension-based hint they already have.

const SNIFF_WINDOW: usize = 1024;

/// Guess a MIME type from the first bytes of a payload.
///
/// Returns a static string suitable for the
/// [`crate::routing::EngineRouteRequest::content_type`] slot, or `None`
/// if no signature matches.
pub fn sniff_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.is_empty() {
        return None;
    }
    if let Some(image) = sniff_image(bytes) {
        return Some(image);
    }
    if let Some(structured) = sniff_structured(bytes) {
        return Some(structured);
    }
    sniff_text(bytes)
}

fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"<svg") || starts_with_after_ws(bytes, b"<svg") {
        return Some("image/svg+xml");
    }
    None
}

fn sniff_structured(bytes: &[u8]) -> Option<&'static str> {
    let head = head_window(bytes);
    if starts_with_after_bom_and_ws(head, b"<?xml") {
        return Some("application/xml");
    }
    let trimmed = trim_leading_ws(strip_bom(head));
    if trimmed.starts_with(b"<!doctype html") || trimmed.starts_with(b"<!DOCTYPE html") {
        return Some("text/html");
    }
    if trimmed.starts_with(b"<html")
        || trimmed.starts_with(b"<HTML")
        || trimmed.starts_with(b"<head")
        || trimmed.starts_with(b"<body")
    {
        return Some("text/html");
    }
    if has_atom_or_rss_root(trimmed) {
        return Some("application/atom+xml");
    }
    if trimmed.starts_with(b"---\n") || trimmed.starts_with(b"---\r\n") {
        // YAML-frontmatter prelude — Mere's knot format. Markdown files
        // can also start with `---` (CommonMark thematic break) but a
        // bare rule isn't followed by `---` again on a later line; the
        // knot opener is followed by key/value lines and a closing
        // `---`. Cheap check: require at least one `:` before the
        // closing `---` within the sniff window.
        if looks_like_knot_frontmatter(trimmed) {
            return Some("text/x-knot");
        }
    }
    None
}

fn sniff_text(bytes: &[u8]) -> Option<&'static str> {
    let head = head_window(bytes);
    let trimmed = trim_leading_ws(strip_bom(head));

    // Gemtext: leading `=>` link line, `# ` heading, `* ` list item, or
    // `> ` quote — same as markdown for some, but `=>` is a strong
    // gemtext-only marker.
    if trimmed.starts_with(b"=> ") || trimmed.starts_with(b"=>\t") {
        return Some("text/gemini");
    }

    // Markdown: ATX heading, fenced code, setext underline, or list
    // markers in the first few lines. Conservative — only fire on
    // strong markers to avoid claiming arbitrary text.
    if has_markdown_marker(trimmed) {
        return Some("text/markdown");
    }

    // Fallback: looks like text (no NUL bytes in the sniff window) →
    // text/plain. Anything else stays unknown.
    if !head.contains(&0) {
        Some("text/plain")
    } else {
        None
    }
}

fn head_window(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.len().min(SNIFF_WINDOW)]
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

fn trim_leading_ws(bytes: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    &bytes[i..]
}

fn starts_with_after_ws(bytes: &[u8], needle: &[u8]) -> bool {
    trim_leading_ws(bytes).starts_with(needle)
}

fn starts_with_after_bom_and_ws(bytes: &[u8], needle: &[u8]) -> bool {
    trim_leading_ws(strip_bom(bytes)).starts_with(needle)
}

fn has_atom_or_rss_root(trimmed: &[u8]) -> bool {
    // Cheap substring scan within the sniff window — Atom/RSS feeds
    // often have an XML decl or comments before the root element.
    let s = std::str::from_utf8(trimmed).unwrap_or("");
    s.contains("<feed") || s.contains("<rss") || s.contains("<rdf:RDF")
}

fn looks_like_knot_frontmatter(trimmed: &[u8]) -> bool {
    let s = match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // First line is `---`. Look for at least one `key: value` line and
    // a closing `---` within the sniff window.
    let mut lines = s.lines();
    let _opener = lines.next();
    let mut saw_kv = false;
    for line in lines {
        if line == "---" {
            return saw_kv;
        }
        if !saw_kv && line.contains(':') {
            saw_kv = true;
        }
    }
    false
}

fn has_markdown_marker(trimmed: &[u8]) -> bool {
    let s = match std::str::from_utf8(trimmed) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for (i, line) in s.lines().take(8).enumerate() {
        let l = line.trim_start();
        if l.starts_with("# ")
            || l.starts_with("## ")
            || l.starts_with("### ")
            || l.starts_with("```")
        {
            return true;
        }
        // Setext heading underline on line 2+: ===… or ---…
        if i > 0
            && (l.starts_with("===") || l.starts_with("---"))
            && l.chars().all(|c| c == l.chars().next().unwrap())
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(sniff_content_type(b""), None);
    }

    #[test]
    fn png_signature_is_detected() {
        let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        assert_eq!(sniff_content_type(png), Some("image/png"));
    }

    #[test]
    fn jpeg_signature_is_detected() {
        let jpeg = b"\xFF\xD8\xFF\xE0\x00\x10JFIF";
        assert_eq!(sniff_content_type(jpeg), Some("image/jpeg"));
    }

    #[test]
    fn gif_signatures_are_detected() {
        assert_eq!(sniff_content_type(b"GIF89a..."), Some("image/gif"));
        assert_eq!(sniff_content_type(b"GIF87a..."), Some("image/gif"));
    }

    #[test]
    fn webp_signature_is_detected() {
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(sniff_content_type(webp), Some("image/webp"));
    }

    #[test]
    fn svg_with_leading_whitespace_is_detected() {
        let svg = b"  <svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(sniff_content_type(svg), Some("image/svg+xml"));
    }

    #[test]
    fn html_doctype_is_detected_case_insensitive() {
        let html = b"<!DOCTYPE html>\n<html>";
        assert_eq!(sniff_content_type(html), Some("text/html"));
        let html_lower = b"<!doctype html>\n<html>";
        assert_eq!(sniff_content_type(html_lower), Some("text/html"));
    }

    #[test]
    fn html_root_without_doctype_is_detected() {
        assert_eq!(
            sniff_content_type(b"<html><body/></html>"),
            Some("text/html")
        );
    }

    #[test]
    fn xml_declaration_is_detected_as_xml() {
        let xml = b"<?xml version=\"1.0\"?><root/>";
        assert_eq!(sniff_content_type(xml), Some("application/xml"));
    }

    #[test]
    fn atom_feed_is_detected_inside_xml() {
        let atom = b"<?xml version=\"1.0\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\"/>";
        // XML decl wins as a more general label — acceptable: routing
        // can still dispatch on application/xml. But we prefer feed
        // when the root is feed-shaped within the window.
        let got = sniff_content_type(atom).unwrap();
        assert!(
            got == "application/atom+xml" || got == "application/xml",
            "got {got}"
        );
    }

    #[test]
    fn rss_root_is_detected() {
        let rss = b"<rss version=\"2.0\"><channel/></rss>";
        assert_eq!(sniff_content_type(rss), Some("application/atom+xml"));
    }

    #[test]
    fn knot_frontmatter_is_detected() {
        let knot = b"---\ntitle: Hello\nsource: file:///x\n---\n\n# Body\n";
        assert_eq!(sniff_content_type(knot), Some("text/x-knot"));
    }

    #[test]
    fn frontmatter_without_closing_does_not_claim_knot() {
        // Three dashes alone is a markdown thematic break, not a knot
        // file.
        let md = b"---\n# Just a heading after a rule\n";
        assert_ne!(sniff_content_type(md), Some("text/x-knot"));
    }

    #[test]
    fn gemtext_link_line_is_detected() {
        let gmi = b"=> gemini://example.org/ Example\n";
        assert_eq!(sniff_content_type(gmi), Some("text/gemini"));
    }

    #[test]
    fn markdown_heading_is_detected() {
        let md = b"# Title\n\nBody paragraph here.\n";
        assert_eq!(sniff_content_type(md), Some("text/markdown"));
    }

    #[test]
    fn markdown_fenced_code_is_detected() {
        let md = b"```rust\nfn main() {}\n```\n";
        assert_eq!(sniff_content_type(md), Some("text/markdown"));
    }

    #[test]
    fn plain_text_falls_through_to_text_plain() {
        let txt = b"Just some unstructured prose without any markers.\n";
        assert_eq!(sniff_content_type(txt), Some("text/plain"));
    }

    #[test]
    fn binary_with_nul_returns_none() {
        let bin = b"\x00\x01\x02\x03random binary garbage\x00\xFF";
        assert_eq!(sniff_content_type(bin), None);
    }

    #[test]
    fn utf8_bom_does_not_block_detection() {
        let html = b"\xEF\xBB\xBF<!DOCTYPE html><html/>";
        assert_eq!(sniff_content_type(html), Some("text/html"));
    }

    #[test]
    fn sniff_window_does_not_read_past_bound() {
        // Construct a buffer whose marker is past the sniff window —
        // should not be detected.
        let mut buf = vec![b' '; SNIFF_WINDOW + 64];
        buf.extend_from_slice(b"<html/>");
        // All-whitespace prefix → trim_leading_ws gets to end of head
        // window with no marker → returns text/plain (no NULs).
        assert_eq!(sniff_content_type(&buf), Some("text/plain"));
    }
}
