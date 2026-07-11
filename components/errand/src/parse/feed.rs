//! Feed parser — RSS 2.0 and Atom 1.0 into a portable [`Feed`].
//!
//! The two XML flavours share one event-driven walker: they differ in element
//! names but share the same logical shape — a feed-level title plus a sequence of
//! entries, each with a title, link, date, and summary. RSS expresses links as
//! element text; Atom uses `<link href=…>` attributes.
//!
//! Summary handling is deliberately lossy: HTML in `<description>` / `<content>`
//! is stripped to plain text (see [`strip_html_tags`]), and the count of stripped
//! entries is reported in [`Feed::html_stripped`] so a consumer can surface a
//! "degraded" hint. JSON Feed is *not* handled here (it needs a JSON dependency a
//! transport crate should not carry); a consumer parses it and builds a [`Feed`]
//! itself — the public fields make that straightforward.

use quick_xml::escape::unescape;
use quick_xml::events::Event;
use quick_xml::Reader;

/// A parsed feed: channel-level metadata plus entries, flavour-neutral.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Feed {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub link: Option<String>,
    pub lang: Option<String>,
    pub entries: Vec<FeedEntry>,
    /// How many entry summaries had HTML stripped (for a degraded-rendering hint).
    pub html_stripped: usize,
}

/// One feed entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedEntry {
    pub title: Option<String>,
    pub link: Option<String>,
    pub date: Option<String>,
    pub summary: Option<String>,
}

/// A feed parse error (malformed or truncated XML).
#[derive(Clone, Debug)]
pub struct FeedError(pub String);

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "feed parse error: {}", self.0)
    }
}

impl std::error::Error for FeedError {}

/// Parse an RSS 2.0 or Atom 1.0 document into a [`Feed`].
pub fn parse(body: &str) -> Result<Feed, FeedError> {
    let mut reader = Reader::from_str(body);
    // Don't enable trim_text: quick-xml splits text events around entity
    // references (`&lt;`, etc.), and trimming each chunk eats the spaces between
    // them. Element-level trimming happens at commit.
    let mut buf = Vec::new();
    let mut state = State::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                state.start_element(local.clone());
                if local == "link" {
                    if let Some(href) = atom_href(&e) {
                        state.set_link(href);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                // Atom self-closing <link href="..." rel="alternate"/>.
                let local = local_name(e.name().as_ref());
                if local == "link" {
                    if let Some(href) = atom_href(&e) {
                        state.set_link(href);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                let raw = std::str::from_utf8(t.as_ref()).map_err(err)?;
                let unescaped = unescape(raw).map_err(err)?.into_owned();
                state.append_text(&unescaped);
            }
            Ok(Event::GeneralRef(r)) => {
                // quick-xml 0.39 emits entity references as their own event,
                // split out of surrounding Text. Resolve and append.
                let name = std::str::from_utf8(r.as_ref()).map_err(err)?;
                let unescaped = unescape(&format!("&{name};")).map_err(err)?.into_owned();
                state.append_text(&unescaped);
            }
            Ok(Event::CData(c)) => {
                let raw = std::str::from_utf8(c.as_ref()).map_err(err)?;
                state.append_text(raw);
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                state.end_element(&local);
            }
            Ok(Event::Eof) => {
                if !state.path.is_empty() {
                    return Err(FeedError(format!(
                        "feed truncated; unclosed element <{}>",
                        state.path.last().map(String::as_str).unwrap_or("?")
                    )));
                }
                break;
            }
            Err(e) => return Err(err(e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(state.feed)
}

/// Naively strip HTML tags from a fragment, preserving text content and
/// collapsing the whitespace that stripping introduces. Shared with consumers
/// (e.g. a JSON Feed path) so summary handling matches across flavours.
pub fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_ws = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !prev_ws {
                collapsed.push(' ');
            }
            prev_ws = true;
        } else {
            collapsed.push(ch);
            prev_ws = false;
        }
    }
    collapsed.trim().to_string()
}

#[derive(Default)]
struct State {
    path: Vec<String>,
    pending_text: String,
    current_entry: Option<FeedEntry>,
    feed: Feed,
}

impl State {
    fn in_entry(&self) -> bool {
        self.path.iter().any(|p| p == "item" || p == "entry")
    }

    fn start_element(&mut self, name: String) {
        self.pending_text.clear();
        if name == "item" || name == "entry" {
            self.current_entry = Some(FeedEntry::default());
        }
        self.path.push(name);
    }

    /// Apply an Atom `<link href=…>` to the entry or the feed, first-wins.
    fn set_link(&mut self, href: String) {
        if self.in_entry() {
            if let Some(entry) = &mut self.current_entry {
                if entry.link.is_none() {
                    entry.link = Some(href);
                }
            }
        } else if self.feed.link.is_none() {
            self.feed.link = Some(href);
        }
    }

    fn end_element(&mut self, name: &str) {
        let text = std::mem::take(&mut self.pending_text);
        let trimmed = text.trim();
        let parent = self.path.iter().rev().nth(1).map(String::as_str);

        if !trimmed.is_empty() {
            if let Some(entry) = &mut self.current_entry {
                match name {
                    "title" => {
                        if entry.title.is_none() {
                            entry.title = Some(trimmed.to_string());
                        }
                    }
                    "link" => {
                        // Atom emits an empty <link/> with href; only RSS-style
                        // text content reaches here.
                        if entry.link.is_none() {
                            entry.link = Some(trimmed.to_string());
                        }
                    }
                    "pubDate" | "published" | "updated" => {
                        if entry.date.is_none() {
                            entry.date = Some(trimmed.to_string());
                        }
                    }
                    "description" | "summary" | "content" => {
                        if entry.summary.is_none() {
                            let had_tags = trimmed.contains('<');
                            entry.summary = Some(strip_html_tags(trimmed));
                            if had_tags {
                                self.feed.html_stripped += 1;
                            }
                        }
                    }
                    _ => {}
                }
            } else {
                match name {
                    "title" => {
                        if matches!(parent, Some("channel") | Some("feed"))
                            && self.feed.title.is_none()
                        {
                            self.feed.title = Some(trimmed.to_string());
                        }
                    }
                    "subtitle" | "description" => {
                        if matches!(parent, Some("channel") | Some("feed"))
                            && self.feed.subtitle.is_none()
                        {
                            self.feed.subtitle = Some(strip_html_tags(trimmed));
                        }
                    }
                    "link" => {
                        if matches!(parent, Some("channel")) && self.feed.link.is_none() {
                            self.feed.link = Some(trimmed.to_string());
                        }
                    }
                    "language" => {
                        if matches!(parent, Some("channel")) && self.feed.lang.is_none() {
                            self.feed.lang = Some(trimmed.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }

        let popped = self.path.pop();
        if popped.as_deref() == Some("item") || popped.as_deref() == Some("entry") {
            if let Some(entry) = self.current_entry.take() {
                if entry.title.is_some() || entry.link.is_some() || entry.summary.is_some() {
                    self.feed.entries.push(entry);
                }
            }
        }
    }

    fn append_text(&mut self, text: &str) {
        self.pending_text.push_str(text);
    }
}

fn local_name(qualified: &[u8]) -> String {
    let s = std::str::from_utf8(qualified).unwrap_or("");
    s.rsplit(':').next().unwrap_or(s).to_string()
}

fn atom_href(element: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    for attr in element.attributes().flatten() {
        if attr.key.local_name().as_ref() == b"href" {
            return attr
                .unescape_value()
                .ok()
                .map(|v| v.into_owned())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

fn err<E: std::fmt::Display>(e: E) -> FeedError {
    FeedError(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Capsule Log</title>
  <link>https://example.test/</link>
  <description>News &amp; notes</description>
  <language>en</language>
  <item>
    <title>First post</title>
    <link>https://example.test/1</link>
    <pubDate>Mon, 01 Jan 2026 00:00:00 GMT</pubDate>
    <description>A &lt;b&gt;bold&lt;/b&gt; summary.</description>
  </item>
  <item>
    <title>Second post</title>
    <link>https://example.test/2</link>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Capsule</title>
  <link href="https://atom.test/" rel="alternate"/>
  <subtitle>An atom feed</subtitle>
  <entry>
    <title>Atom one</title>
    <link href="https://atom.test/a" rel="alternate"/>
    <updated>2026-01-01T00:00:00Z</updated>
    <summary>Plain summary.</summary>
  </entry>
</feed>"#;

    #[test]
    fn rss_parses_channel_and_items() {
        let feed = parse(RSS).expect("rss");
        assert_eq!(feed.title.as_deref(), Some("Capsule Log"));
        assert_eq!(feed.link.as_deref(), Some("https://example.test/"));
        assert_eq!(feed.subtitle.as_deref(), Some("News & notes"));
        assert_eq!(feed.lang.as_deref(), Some("en"));
        assert_eq!(feed.entries.len(), 2);
        assert_eq!(feed.entries[0].title.as_deref(), Some("First post"));
        assert_eq!(feed.entries[0].link.as_deref(), Some("https://example.test/1"));
        assert_eq!(feed.entries[0].summary.as_deref(), Some("A bold summary."));
        assert_eq!(feed.html_stripped, 1, "the first item's HTML summary was stripped");
    }

    #[test]
    fn atom_uses_link_href_attributes() {
        let feed = parse(ATOM).expect("atom");
        assert_eq!(feed.title.as_deref(), Some("Atom Capsule"));
        assert_eq!(feed.link.as_deref(), Some("https://atom.test/"));
        assert_eq!(feed.subtitle.as_deref(), Some("An atom feed"));
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].link.as_deref(), Some("https://atom.test/a"));
        assert_eq!(feed.entries[0].date.as_deref(), Some("2026-01-01T00:00:00Z"));
    }

    #[test]
    fn truncated_feed_errors() {
        assert!(parse("<rss><channel><title>X").is_err());
    }

    #[test]
    fn strip_html_tags_collapses_whitespace() {
        assert_eq!(strip_html_tags("<p>a   <b>b</b></p>"), "a b");
    }
}
