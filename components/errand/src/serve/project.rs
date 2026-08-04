//! Projecting one [`Listing`] into each protocol's native format.
//!
//! These are pure functions on purpose: they are the part of serving most
//! worth testing, and they are where the "project faithfully, add nothing"
//! rule is either kept or quietly broken.
//!
//! Nothing here invents syntax. A gophermap is RFC 1436's tab-separated item
//! lines; gemtext is its own line grammar. A reader must not be able to tell
//! from the bytes that these came from the same source, let alone from us.

use super::{EntryKind, Listing};

/// Render a listing as gemtext.
///
/// A title becomes a level-one heading, preamble lines become paragraphs, and
/// every entry becomes a link line. Gemtext has no item types, so the kind is
/// carried by the target and the reader's client, not by a marker we invent.
pub fn to_gemtext(listing: &Listing) -> String {
    let mut out = String::new();
    if let Some(title) = &listing.title {
        out.push_str("# ");
        out.push_str(title);
        out.push_str("\n\n");
    }
    for line in &listing.preamble {
        out.push_str(line);
        out.push('\n');
    }
    if !listing.preamble.is_empty() {
        out.push('\n');
    }
    for entry in &listing.entries {
        out.push_str("=> ");
        out.push_str(&entry.target);
        if !entry.display.is_empty() {
            out.push(' ');
            out.push_str(&entry.display);
        }
        out.push('\n');
    }
    out
}

/// Render a listing as an RFC 1436 gophermap.
///
/// `host` and `port` are stamped into every local item, because gopher item
/// lines carry their own host and port rather than being relative. Entries
/// whose target is already absolute become `h` URL items, which is how gopher
/// points outside itself.
pub fn to_gophermap(listing: &Listing, host: &str, port: u16) -> String {
    let mut out = String::new();

    // Gopher has no heading construct: a title is info text like any other.
    // Rendering it as anything else would be inventing syntax.
    if let Some(title) = &listing.title {
        push_info(&mut out, title, host, port);
        push_info(&mut out, "", host, port);
    }
    for line in &listing.preamble {
        push_info(&mut out, line, host, port);
    }

    for entry in &listing.entries {
        if entry.is_absolute() {
            // RFC 4266's URL item: type `h`, selector `URL:<target>`.
            out.push_str(&format!(
                "h{}\tURL:{}\t{}\t{}\r\n",
                entry.display, entry.target, host, port
            ));
        } else {
            out.push_str(&format!(
                "{}{}\t{}\t{}\t{}\r\n",
                item_type(entry.kind),
                entry.display,
                entry.target,
                host,
                port
            ));
        }
    }
    // The RFC 1436 terminator.
    out.push_str(".\r\n");
    out
}

/// Render a listing as plain text, for finger and anything else with no
/// document format at all.
pub fn to_plain_text(listing: &Listing) -> String {
    let mut out = String::new();
    if let Some(title) = &listing.title {
        out.push_str(title);
        out.push('\n');
        out.push('\n');
    }
    for line in &listing.preamble {
        out.push_str(line);
        out.push('\n');
    }
    if !listing.preamble.is_empty() && !listing.entries.is_empty() {
        out.push('\n');
    }
    for entry in &listing.entries {
        out.push_str(&entry.display);
        out.push_str(" - ");
        out.push_str(&entry.target);
        out.push('\n');
    }
    out
}

/// The gopher item-type character for an entry kind.
fn item_type(kind: EntryKind) -> char {
    match kind {
        EntryKind::Directory => '1',
        EntryKind::Text => '0',
        EntryKind::Search => '7',
        EntryKind::Image => 'I',
        EntryKind::Sound => 's',
        EntryKind::Binary => '9',
    }
}

/// An `i` info line. Gopher requires the host and port fields even on lines
/// that point at nothing, so they are filled rather than left empty.
fn push_info(out: &mut String, text: &str, host: &str, port: u16) {
    out.push_str(&format!("i{text}\t\t{host}\t{port}\r\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::Entry;

    fn listing() -> Listing {
        Listing {
            title: Some("Dictionary".into()),
            preamble: vec!["Words people added.".into()],
            entries: vec![
                Entry::new("Aardvark", "/w/aardvark", EntryKind::Text),
                Entry::new("Browse letters", "/w/", EntryKind::Directory),
                Entry::new("Search", "/search", EntryKind::Search),
                Entry::new("Elsewhere", "gemini://other.test/", EntryKind::Text),
            ],
        }
    }

    #[test]
    fn gemtext_uses_link_lines_and_a_heading() {
        let out = to_gemtext(&listing());
        assert!(out.starts_with("# Dictionary\n\n"));
        assert!(out.contains("Words people added.\n"));
        assert!(out.contains("=> /w/aardvark Aardvark\n"));
        assert!(out.contains("=> gemini://other.test/ Elsewhere\n"));
    }

    #[test]
    fn gemtext_invents_no_type_markers() {
        // A search entry and a text entry look the same in gemtext, because
        // gemtext has no item types. Adding one would be extending a format
        // we do not own.
        let out = to_gemtext(&listing());
        assert!(out.contains("=> /search Search\n"));
        assert!(!out.contains('\t'), "gemtext carries no tabs");
    }

    #[test]
    fn a_gophermap_stamps_host_and_port_on_every_item() {
        let out = to_gophermap(&listing(), "example.test", 70);
        assert!(out.contains("0Aardvark\t/w/aardvark\texample.test\t70\r\n"));
        assert!(out.contains("1Browse letters\t/w/\texample.test\t70\r\n"));
    }

    #[test]
    fn gopher_item_types_follow_the_entry_kind() {
        let out = to_gophermap(&listing(), "h", 70);
        assert!(out.contains("7Search\t/search"), "search is type 7");
        assert!(out.starts_with("iDictionary\t\th\t70\r\n"), "title is info text");
    }

    #[test]
    fn an_absolute_target_becomes_a_url_item() {
        let out = to_gophermap(&listing(), "h", 70);
        assert!(out.contains("hElsewhere\tURL:gemini://other.test/\th\t70\r\n"));
    }

    #[test]
    fn a_gophermap_ends_with_the_rfc_terminator() {
        assert!(to_gophermap(&listing(), "h", 70).ends_with(".\r\n"));
    }

    #[test]
    fn the_gophermap_round_trips_through_the_gopher_parser() {
        // The receipt that we project into a format its own parser accepts.
        let out = to_gophermap(&listing(), "example.test", 70);
        let items = gopher_protocol::parse_menu(&out);

        let kinds: Vec<_> = items.iter().map(|i| i.kind.clone()).collect();
        assert!(kinds.contains(&gopher_protocol::GopherKind::Info));
        assert!(kinds.contains(&gopher_protocol::GopherKind::Text));
        assert!(kinds.contains(&gopher_protocol::GopherKind::Submenu));
        assert!(kinds.contains(&gopher_protocol::GopherKind::Search));
        assert!(kinds.contains(&gopher_protocol::GopherKind::Url));

        let aardvark = items.iter().find(|i| i.display == "Aardvark").unwrap();
        assert_eq!(
            aardvark.url.as_deref(),
            Some("gopher://example.test/0/w/aardvark")
        );
    }

    #[test]
    fn the_gemtext_round_trips_through_the_gemtext_parser() {
        let out = to_gemtext(&listing());
        let lines = gemini_protocol::parse_gemtext(&out);

        let links: Vec<_> = lines
            .iter()
            .filter_map(|line| match line {
                gemini_protocol::GemLine::Link { url, label } => {
                    Some((url.clone(), label.clone()))
                },
                _ => None,
            })
            .collect();

        assert_eq!(links.len(), 4, "every entry became a link line");
        assert_eq!(links[0].0, "/w/aardvark");
        assert_eq!(links[0].1, "Aardvark");
    }

    #[test]
    fn plain_text_is_readable_without_any_format_at_all() {
        let out = to_plain_text(&listing());
        assert!(out.starts_with("Dictionary\n\n"));
        assert!(out.contains("Aardvark - /w/aardvark\n"));
        assert!(!out.contains("=>"), "no gemtext syntax leaks in");
        assert!(!out.contains('\t'), "no gophermap syntax leaks in");
    }

    #[test]
    fn an_empty_listing_still_produces_valid_output() {
        let empty = Listing::default();
        assert_eq!(to_gemtext(&empty), "");
        assert_eq!(to_gophermap(&empty, "h", 70), ".\r\n");
        assert_eq!(to_plain_text(&empty), "");
    }
}
