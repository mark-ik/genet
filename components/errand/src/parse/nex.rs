//! Nex parser — Nex (`nex://`), the minimal smolweb protocol
//! (<https://nex.nightfall.city>).
//!
//! Nex has two response shapes and no item-type prefixes (unlike gopher):
//!
//! - **Directory listing** — one entry per line; a trailing `/` marks a
//!   subdirectory, otherwise a file.
//! - **Plain text** — a content response, just text.
//!
//! Detection is by line shape: if every non-empty line is a plausible entry
//! (short, whitespace-free), it is a directory. [`parse`] returns the entries
//! when so, or `None` for a content response the caller renders as text. URL
//! resolution against the request address is the caller's step; [`base_url`]
//! computes the base to join entry names onto.

/// One directory entry. `name` is the raw entry (including a trailing `/` for a
/// subdirectory, which is also what joins onto [`base_url`] to form its URL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NexEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Parse a Nex body. `Some(entries)` when it is a directory listing (every
/// non-empty line is a plausible entry); `None` when it is a content response the
/// caller should render as plain text.
pub fn parse(body: &str) -> Option<Vec<NexEntry>> {
    if !looks_like_directory(body) {
        return None;
    }
    Some(
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| NexEntry {
                name: line.to_string(),
                is_dir: line.ends_with('/'),
            })
            .collect(),
    )
}

/// True when every non-empty line is a plausible directory entry: either ends
/// with `/`, or is a single whitespace-free token of bounded length.
pub fn looks_like_directory(body: &str) -> bool {
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return false;
    }
    lines
        .iter()
        .all(|line| is_directory_entry_line(line.trim()))
}

fn is_directory_entry_line(line: &str) -> bool {
    if line.is_empty() || line.len() > 200 {
        return false;
    }
    !line.contains(char::is_whitespace)
}

/// The base URL directory entries resolve against. `nex://host/path/` stays as
/// itself; `nex://host/page` drops to `nex://host/`; an address with no path
/// slash gets a trailing `/` appended.
pub fn base_url(address: &str) -> String {
    if address.ends_with('/') {
        return address.to_string();
    }
    if let Some(idx) = address.rfind('/') {
        // Keep the slash only if it is part of the path, not the `://` separator.
        if let Some(scheme_end) = address.find("://") {
            if idx <= scheme_end + 2 {
                return format!("{address}/");
            }
        }
        return address[..=idx].to_string();
    }
    format!("{address}/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_listing_classifies_dirs_and_files() {
        let entries = parse("README.txt\nabout/\nphotos/\ncontact.txt\n").expect("directory");
        assert_eq!(
            entries,
            vec![
                NexEntry {
                    name: "README.txt".into(),
                    is_dir: false
                },
                NexEntry {
                    name: "about/".into(),
                    is_dir: true
                },
                NexEntry {
                    name: "photos/".into(),
                    is_dir: true
                },
                NexEntry {
                    name: "contact.txt".into(),
                    is_dir: false
                },
            ]
        );
    }

    #[test]
    fn whitespace_line_disqualifies_directory() {
        assert!(parse("ok\na b\n").is_none());
    }

    #[test]
    fn empty_body_is_not_a_directory() {
        assert!(parse("").is_none());
    }

    #[test]
    fn base_url_keeps_trailing_slash() {
        assert_eq!(
            base_url("nex://example.test/path/"),
            "nex://example.test/path/"
        );
    }

    #[test]
    fn base_url_drops_page_to_parent() {
        assert_eq!(
            base_url("nex://example.test/path/page"),
            "nex://example.test/path/"
        );
    }

    #[test]
    fn base_url_appends_when_only_authority() {
        assert_eq!(base_url("nex://example.test"), "nex://example.test/");
        // A page directly under the authority resolves against the root.
        assert_eq!(base_url("nex://example.test/page"), "nex://example.test/");
    }
}
