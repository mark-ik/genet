//! The OS clipboard backend, over arboard.
//!
//! [`SystemClipboard`] holds a live arboard handle for its whole lifetime, so on
//! X11 and Wayland the selection ownership a write establishes stays alive as
//! long as the service does, rather than being dropped after a single set. The
//! Linux persistence story (content surviving the window's close) is the
//! capability plan's P4 and is not solved here.

use arboard::Clipboard as Arboard;

use crate::{ClipboardError, TextClipboard};

/// The OS clipboard, backed by arboard (text and, at later phases, images).
pub struct SystemClipboard {
    inner: Arboard,
}

impl SystemClipboard {
    /// Open the OS clipboard, or report why it is unreachable (a headless host
    /// or missing display server, typically).
    pub fn new() -> Result<Self, ClipboardError> {
        Arboard::new()
            .map(|inner| Self { inner })
            .map_err(|error| ClipboardError::Unavailable(error.to_string()))
    }
}

impl TextClipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<String, ClipboardError> {
        self.inner.get_text().map_err(map_error)
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.inner.set_text(text).map_err(map_error)
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.inner.clear().map_err(map_error)
    }
}

/// Fold an arboard error into the service's vocabulary. arboard's error type is
/// `#[non_exhaustive]`, so unrecognized variants land in `Backend`.
fn map_error(error: arboard::Error) -> ClipboardError {
    match error {
        arboard::Error::ContentNotAvailable => ClipboardError::Empty,
        arboard::Error::ClipboardNotSupported => ClipboardError::Unavailable(error.to_string()),
        other => ClipboardError::Backend(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trips text through the real OS clipboard on the host running the
    /// test. Ignored by default because it touches shared machine state and
    /// needs a display; it restores whatever was on the clipboard first, so a
    /// local `--ignored` run does not clobber the user's copy buffer.
    #[test]
    #[ignore = "touches the real OS clipboard; run locally with --ignored"]
    fn system_clipboard_round_trips_on_this_host() {
        let mut clipboard = SystemClipboard::new().expect("a clipboard on this host");
        let restore = clipboard.get_text().ok();

        clipboard.set_text("genet-clipboard-probe").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "genet-clipboard-probe");

        match restore {
            Some(text) => clipboard.set_text(&text).unwrap(),
            None => clipboard.clear().unwrap(),
        }
    }
}
