//! The OS clipboard backend, over arboard.
//!
//! [`SystemClipboard`] holds a live arboard handle for its whole lifetime, so on
//! X11 and Wayland the selection ownership a write establishes stays alive as
//! long as the service does, rather than being dropped after a single set. The
//! Linux persistence story (content surviving the window's close) is the
//! capability plan's P4 and is not solved here.
//!
//! arboard reads text, html, image, and file lists, and each is enumerated on
//! [`read`](Clipboard::read). A write picks one primary representation, because
//! arboard empties the clipboard on each set and cannot hold text and image at
//! once; html carries a plain-text alternative in the same set, so text and html
//! do travel together. Simultaneous text+image and arbitrary [`Mime::Custom`]
//! types are the per-platform backend's job (the plan's P3).

use std::borrow::Cow;
use std::path::PathBuf;

use arboard::{Clipboard as Arboard, ImageData};

use crate::{Clipboard, ClipboardError, ClipboardItem, Image};

/// The OS clipboard, backed by arboard.
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

impl Clipboard for SystemClipboard {
    fn read(&mut self) -> Result<ClipboardItem, ClipboardError> {
        // Each representation is probed independently; one absent (or an
        // unreadable) format leaves the others intact rather than failing the
        // whole read.
        let mut item = ClipboardItem::new();
        if let Ok(text) = self.inner.get_text() {
            item = item.with_text(text);
        }
        if let Ok(html) = self.inner.get().html() {
            item = item.with_html(html);
        }
        if let Ok(image) = self.inner.get_image() {
            item = item.with_image(Image {
                width: image.width,
                height: image.height,
                rgba: image.bytes.into_owned(),
            });
        }
        if let Ok(paths) = self.inner.get().file_list() {
            let uris: Vec<String> = paths
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            if !uris.is_empty() {
                item = item.with_uris(uris);
            }
        }
        if item.is_empty() {
            Err(ClipboardError::Empty)
        } else {
            Ok(item)
        }
    }

    fn write(&mut self, item: &ClipboardItem) -> Result<(), ClipboardError> {
        // One primary representation, richest first. html carries the text as
        // its alternative, so text+html is a single two-format set.
        if let Some(image) = item.image() {
            self.inner
                .set_image(ImageData {
                    width: image.width,
                    height: image.height,
                    bytes: Cow::Borrowed(&image.rgba),
                })
                .map_err(map_error)
        } else if let Some(uris) = item.uris() {
            let paths: Vec<PathBuf> = uris.iter().map(PathBuf::from).collect();
            self.inner.set().file_list(&paths).map_err(map_error)
        } else if let Some(html) = item.html() {
            self.inner
                .set_html(html.to_owned(), item.text().map(str::to_owned))
                .map_err(map_error)
        } else if let Some(text) = item.text() {
            self.inner.set_text(text.to_owned()).map_err(map_error)
        } else {
            self.inner.clear().map_err(map_error)
        }
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
    use crate::TextClipboard;

    /// Round-trips text, html, and an image through the real OS clipboard on the
    /// host running the test. Ignored by default because it touches shared
    /// machine state and needs a display; it restores whatever text was on the
    /// clipboard first, so a local `--ignored` run does not clobber the user's
    /// copy buffer.
    #[test]
    #[ignore = "touches the real OS clipboard; run locally with --ignored"]
    fn system_clipboard_round_trips_text_html_and_image() {
        let mut clipboard = SystemClipboard::new().expect("a clipboard on this host");
        let restore = clipboard.get_text().ok();

        // text + html travel together (html's plain-text alternative is the text).
        clipboard
            .write(
                &ClipboardItem::new()
                    .with_text("a loop")
                    .with_html("<b>a loop</b>"),
            )
            .unwrap();
        let read = clipboard.read().unwrap();
        assert_eq!(read.text(), Some("a loop"));
        assert_eq!(read.html(), Some("<b>a loop</b>"));

        // An image round-trips its pixels and dimensions.
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        clipboard
            .write(&ClipboardItem::new().with_image(image.clone()))
            .unwrap();
        let read = clipboard.read().unwrap();
        let back = read.image().expect("an image round-trips");
        assert_eq!((back.width, back.height), (2, 1));
        assert_eq!(back.rgba, image.rgba);

        match restore {
            Some(text) => clipboard.set_text(&text).unwrap(),
            None => clipboard.clear().unwrap(),
        }
    }
}
