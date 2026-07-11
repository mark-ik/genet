//! Gopher menu parser — RFC 1436 menus into a typed item list.
//!
//! A gopher menu is tab-delimited lines: `<type><display>\t<selector>\t<host>
//! \t<port>`. The first character is the item type; a bare `.` terminates the
//! menu. This parser classifies each line into a [`GopherItem`] carrying the
//! item [`GopherKind`] (so a native viewer can show a per-type affordance) plus
//! the resolved resource URL (synthesised per RFC 4266, or extracted for a URL
//! item). Info and error lines carry no URL. A consumer decides how to present
//! each kind; the parser holds no document or render model.
//!
//! References: RFC 1436 (Gopher), RFC 4266 (gopher URI scheme).

/// The semantic class of a gopher menu line, from its item-type character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GopherKind {
    /// `i` — informational text, no resource.
    Info,
    /// `3` — server error message, no resource.
    Error,
    /// `0` — text file.
    Text,
    /// `1` — submenu / directory.
    Submenu,
    /// `7` — full-text search server.
    Search,
    /// `9` — binary.
    Binary,
    /// `g` / `I` — image.
    Image,
    /// `s` — sound.
    Sound,
    /// `T` — telnet session.
    Telnet,
    /// `h` — URL item (the selector carries an external URL).
    Url,
    /// Any other (still navigable) item type.
    Other(char),
}

/// One parsed gopher menu line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GopherItem {
    pub kind: GopherKind,
    pub display: String,
    /// The resource URL: a synthesised `gopher://` URL (RFC 4266) for standard
    /// items, or the extracted target for a `h` URL item. `None` for [`Info`] and
    /// [`Error`] lines.
    ///
    /// [`Info`]: GopherKind::Info
    /// [`Error`]: GopherKind::Error
    pub url: Option<String>,
}

/// Parse a gopher menu body into items, in source order. Stops at the RFC 1436
/// `.` terminator; skips blank and malformed lines (a non-info/error item with no
/// host, or a URL item with no target).
pub fn parse(body: &str) -> Vec<GopherItem> {
    let mut items = Vec::new();
    for line in body.lines() {
        if line == "." {
            break;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(item) = parse_line(line) {
            items.push(item);
        }
    }
    items
}

fn parse_line(line: &str) -> Option<GopherItem> {
    let mut chars = line.chars();
    let type_char = chars.next()?;
    let rest = chars.as_str();

    let mut parts = rest.splitn(4, '\t');
    let display = parts.next()?.to_string();
    let selector = parts.next().unwrap_or("");
    let host = parts.next().unwrap_or("");
    let port = parts.next().unwrap_or("70");

    match type_char {
        'i' => Some(GopherItem { kind: GopherKind::Info, display, url: None }),
        '3' => Some(GopherItem { kind: GopherKind::Error, display, url: None }),
        'h' => {
            // URL items: selector is typically "URL:https://…". Strip the prefix;
            // skip the line when it carries no usable target.
            let url = selector.strip_prefix("URL:").unwrap_or(selector).trim();
            if url.is_empty() {
                return None;
            }
            Some(GopherItem { kind: GopherKind::Url, display, url: Some(url.to_string()) })
        }
        _ => {
            if host.is_empty() {
                return None;
            }
            let url = synthesise_gopher_url(type_char, host, port, selector);
            Some(GopherItem { kind: kind_of(type_char), display, url: Some(url) })
        }
    }
}

fn kind_of(type_char: char) -> GopherKind {
    match type_char {
        '0' => GopherKind::Text,
        '1' => GopherKind::Submenu,
        '7' => GopherKind::Search,
        '9' => GopherKind::Binary,
        'g' | 'I' => GopherKind::Image,
        's' => GopherKind::Sound,
        'T' => GopherKind::Telnet,
        other => GopherKind::Other(other),
    }
}

fn synthesise_gopher_url(type_char: char, host: &str, port: &str, selector: &str) -> String {
    let port_part = if port.is_empty() || port == "70" {
        String::new()
    } else {
        format!(":{port}")
    };
    // RFC 4266: gopher-path = <gophertype><selector>. The type character is the
    // first path segment, immediately followed by the selector (which may already
    // begin with `/`).
    format!("gopher://{host}{port_part}/{type_char}{selector}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(t: char, display: &str, selector: &str, host: &str, port: &str) -> String {
        format!("{t}{display}\t{selector}\t{host}\t{port}\r\n")
    }

    #[test]
    fn standard_item_synthesises_url_with_type_and_selector() {
        let items = parse(&line('0', "Welcome text", "/welcome.txt", "example.test", "70"));
        assert_eq!(
            items,
            vec![GopherItem {
                kind: GopherKind::Text,
                display: "Welcome text".into(),
                url: Some("gopher://example.test/0/welcome.txt".into()),
            }]
        );
    }

    #[test]
    fn non_default_port_appears_in_url() {
        let items = parse(&line('1', "Sub", "/sub", "example.test", "7070"));
        assert_eq!(items[0].url.as_deref(), Some("gopher://example.test:7070/1/sub"));
        assert_eq!(items[0].kind, GopherKind::Submenu);
    }

    #[test]
    fn url_item_extracts_target() {
        let items = parse(&line('h', "External", "URL:https://example.test/", ".", "70"));
        assert_eq!(items[0].kind, GopherKind::Url);
        assert_eq!(items[0].url.as_deref(), Some("https://example.test/"));
    }

    #[test]
    fn info_and_error_carry_no_url() {
        let items = parse(&format!(
            "{}{}",
            line('i', "hello", "", "example.test", "70"),
            line('3', "boom", "", "example.test", "70"),
        ));
        assert_eq!(items[0], GopherItem { kind: GopherKind::Info, display: "hello".into(), url: None });
        assert_eq!(items[1], GopherItem { kind: GopherKind::Error, display: "boom".into(), url: None });
    }

    #[test]
    fn period_terminator_stops_parsing() {
        let items = parse(&format!(
            "{}{}{}",
            line('i', "before", "", "example.test", "70"),
            ".\r\n",
            line('1', "after", "/x", "example.test", "70"),
        ));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn resource_with_missing_host_is_skipped() {
        assert!(parse("1Bad item\t/sel\t\t70\r\n").is_empty());
    }

    #[test]
    fn unknown_type_stays_navigable() {
        let items = parse(&line('X', "weird", "/sel", "example.test", "70"));
        assert_eq!(items[0].kind, GopherKind::Other('X'));
        assert_eq!(items[0].url.as_deref(), Some("gopher://example.test/X/sel"));
    }
}
