//! The nex directory-listing format: plain text where each line beginning
//! `=> ` followed by a URL is a link. The URL may be absolute or relative.

/// One line of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingLine {
    /// A `=> ` link line. The spec defines only the URL; text after the
    /// first whitespace is preserved as a label by client convention.
    Link { url: String, label: Option<String> },
    /// Any other line, verbatim.
    Text(String),
}

/// Parse a directory listing into its lines.
pub fn parse_listing(text: &str) -> Vec<ListingLine> {
    text.lines()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            match line.strip_prefix("=> ") {
                Some(rest) if !rest.trim().is_empty() => {
                    let rest = rest.trim_start();
                    match rest.split_once(char::is_whitespace) {
                        Some((url, label)) => {
                            let label = label.trim();
                            ListingLine::Link {
                                url: url.to_string(),
                                label: (!label.is_empty()).then(|| label.to_string()),
                            }
                        },
                        None => ListingLine::Link {
                            url: rest.to_string(),
                            label: None,
                        },
                    }
                },
                _ => ListingLine::Text(line.to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_and_text_split_per_spec_examples() {
        let listing = parse_listing(
            "Welcome!\n=> nex://my-site.net\n=> about.txt\n=> ../nexlog/ my nexlog\nplain line\n",
        );
        assert_eq!(listing[0], ListingLine::Text("Welcome!".to_string()));
        assert_eq!(
            listing[1],
            ListingLine::Link {
                url: "nex://my-site.net".to_string(),
                label: None
            }
        );
        assert_eq!(
            listing[2],
            ListingLine::Link {
                url: "about.txt".to_string(),
                label: None
            }
        );
        assert_eq!(
            listing[3],
            ListingLine::Link {
                url: "../nexlog/".to_string(),
                label: Some("my nexlog".to_string())
            }
        );
        assert_eq!(listing[4], ListingLine::Text("plain line".to_string()));
    }

    #[test]
    fn a_bare_arrow_is_text_not_a_link() {
        assert_eq!(
            parse_listing("=> ")[0],
            ListingLine::Text("=> ".to_string())
        );
        // No space after the arrow: the spec requires "=> ".
        assert_eq!(
            parse_listing("=>about.txt")[0],
            ListingLine::Text("=>about.txt".to_string())
        );
    }
}
