//! genet-clipboard: the shared clipboard service for genet consumers.
//!
//! Genet has two host worlds. The browser path (the pelt port) reaches the OS
//! clipboard through the embedder message seam; cambium application hosts render
//! outside the embedder and have no clipboard at all. This crate is the one
//! backend both worlds delegate to, so a musician's loop or a peer's hand-off
//! token crosses the same seam a web page's `navigator.clipboard` will.
//!
//! P0 is the text surface: [`TextClipboard`], its OS-backed [`SystemClipboard`]
//! (arboard), and an in-memory [`MemoryClipboard`] for tests and headless hosts.
//! The plan (`docs/2026-07-18_clipboard_capability_plan.md`) widens this to the
//! web `ClipboardItem` model: ordered items, each a map of MIME type to a bytes
//! or lazy payload, carrying images, url-lists, custom formats, and audio.

use std::fmt;

/// Read and write the clipboard's plain text.
///
/// The P0 surface, deliberately small. Later phases add typed multi-format
/// items alongside these methods rather than replacing them, since text is
/// always one representation an item can carry.
pub trait TextClipboard {
    /// The clipboard's current text, or [`ClipboardError::Empty`] when it holds
    /// none (which is distinct from the clipboard being unavailable).
    fn get_text(&mut self) -> Result<String, ClipboardError>;

    /// Replace the clipboard's contents with `text`.
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;

    /// Empty the clipboard.
    fn clear(&mut self) -> Result<(), ClipboardError>;
}

/// Why a clipboard operation could not complete.
///
/// [`Empty`](Self::Empty) and [`Unavailable`](Self::Unavailable) are kept
/// distinct on purpose: an empty clipboard is a normal state a caller handles
/// silently, while an unavailable one (a headless host, no display) is worth
/// surfacing once.
#[derive(Debug)]
pub enum ClipboardError {
    /// No clipboard is reachable: a headless host, or no display server.
    Unavailable(String),
    /// The clipboard is reachable but holds no text.
    Empty,
    /// The backend failed for another reason.
    Backend(String),
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "clipboard unavailable: {why}"),
            Self::Empty => f.write_str("clipboard holds no text"),
            Self::Backend(why) => write!(f, "clipboard error: {why}"),
        }
    }
}

impl std::error::Error for ClipboardError {}

/// An in-process clipboard that never touches the OS.
///
/// It backs tests and headless hosts, and it stands in wherever a real
/// clipboard is not wanted. It is the reference behaviour the OS backends are
/// checked against.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    text: Option<String>,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TextClipboard for MemoryClipboard {
    fn get_text(&mut self) -> Result<String, ClipboardError> {
        self.text.clone().ok_or(ClipboardError::Empty)
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.text = Some(text.to_owned());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.text = None;
        Ok(())
    }
}

#[cfg(feature = "system")]
mod system;
#[cfg(feature = "system")]
pub use system::SystemClipboard;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_clipboard_round_trips_and_distinguishes_empty() {
        let mut clipboard = MemoryClipboard::new();
        assert!(matches!(clipboard.get_text(), Err(ClipboardError::Empty)));

        clipboard.set_text("ab12cd").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "ab12cd");

        clipboard.set_text("replaced").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "replaced");

        clipboard.clear().unwrap();
        assert!(matches!(clipboard.get_text(), Err(ClipboardError::Empty)));
    }

    #[test]
    fn empty_and_unavailable_render_differently() {
        assert_ne!(
            ClipboardError::Empty.to_string(),
            ClipboardError::Unavailable("no display".to_string()).to_string()
        );
    }
}
