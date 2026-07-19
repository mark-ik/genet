//! genet-clipboard: the shared clipboard service for genet consumers.
//!
//! Genet has two host worlds. The browser path (the pelt port) reaches the OS
//! clipboard through the embedder message seam; cambium application hosts render
//! outside the embedder and have no clipboard at all. This crate is the one
//! backend both worlds delegate to, so a musician's loop or a peer's hand-off
//! token crosses the same seam a web page's `navigator.clipboard` will.
//!
//! The model is the web [`ClipboardItem`] shape: an item is an ordered set of
//! representations (richest first), each a [`Mime`] type and its payload. A
//! reader takes the first representation it understands; a writer offers what it
//! can. [`Clipboard`] reads and writes items; [`TextClipboard`] is the plain
//! text convenience every `Clipboard` gets for free.
//!
//! Backends: [`SystemClipboard`] (arboard: text, html, image, and file/uri
//! lists) and an in-memory [`MemoryClipboard`] for tests and headless hosts.
//! arboard holds one primary representation per write and cannot carry arbitrary
//! custom MIME types; simultaneous text+image and `Mime::Custom` (the audio and
//! app-format lane) are the per-platform backend's follow-on, tracked in the
//! capability plan's P3.

use std::fmt;

/// A clipboard representation's media type. The named types round-trip through
/// the OS backend today; [`Custom`](Self::Custom) carries an arbitrary MIME
/// string that only the in-memory backend holds until the platform backend
/// lands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mime {
    TextPlain,
    TextHtml,
    ImagePng,
    /// A newline-separated list of URIs or file paths (`text/uri-list`).
    UriList,
    /// An arbitrary MIME type, e.g. `audio/wav` or `application/x-hocket-loop`.
    Custom(String),
}

/// Raw RGBA image pixels plus dimensions, matching the OS backend's image shape.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// A clipboard payload: the representations offered to or read from the
/// clipboard, richest first. Build one with the `with_*` methods and read it
/// with the accessors; [`formats`](Self::formats) lists what it carries.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct ClipboardItem {
    text: Option<String>,
    html: Option<String>,
    image: Option<Image>,
    uris: Option<Vec<String>>,
}

impl ClipboardItem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.html = Some(html.into());
        self
    }

    pub fn with_image(mut self, image: Image) -> Self {
        self.image = Some(image);
        self
    }

    pub fn with_uris(mut self, uris: Vec<String>) -> Self {
        self.uris = Some(uris);
        self
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn html(&self) -> Option<&str> {
        self.html.as_deref()
    }

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn uris(&self) -> Option<&[String]> {
        self.uris.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none() && self.uris.is_none()
    }

    /// The MIME types this item carries, richest first, so a reader can pick.
    pub fn formats(&self) -> Vec<Mime> {
        let mut formats = Vec::new();
        if self.html.is_some() {
            formats.push(Mime::TextHtml);
        }
        if self.image.is_some() {
            formats.push(Mime::ImagePng);
        }
        if self.uris.is_some() {
            formats.push(Mime::UriList);
        }
        if self.text.is_some() {
            formats.push(Mime::TextPlain);
        }
        formats
    }
}

/// The typed clipboard: read and write multi-representation [`ClipboardItem`]s.
pub trait Clipboard {
    /// Read every representation the clipboard currently holds into one item.
    fn read(&mut self) -> Result<ClipboardItem, ClipboardError>;

    /// Offer `item`'s representations to the clipboard.
    fn write(&mut self, item: &ClipboardItem) -> Result<(), ClipboardError>;

    /// Empty the clipboard.
    fn clear(&mut self) -> Result<(), ClipboardError>;
}

/// Read and write the clipboard's plain text. Every [`Clipboard`] implements
/// this for free, so a text-only consumer (a contact token, a URL bar) need not
/// touch the item model.
pub trait TextClipboard {
    /// The clipboard's current text, or [`ClipboardError::Empty`] when it holds
    /// none (distinct from the clipboard being unavailable).
    fn get_text(&mut self) -> Result<String, ClipboardError>;

    /// Replace the clipboard's contents with `text`.
    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

impl<C: Clipboard> TextClipboard for C {
    fn get_text(&mut self) -> Result<String, ClipboardError> {
        self.read()?
            .text()
            .map(str::to_owned)
            .ok_or(ClipboardError::Empty)
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.write(&ClipboardItem::new().with_text(text))
    }
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
    /// The clipboard is reachable but holds nothing the reader asked for.
    Empty,
    /// The backend failed for another reason.
    Backend(String),
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "clipboard unavailable: {why}"),
            Self::Empty => f.write_str("clipboard holds nothing to read"),
            Self::Backend(why) => write!(f, "clipboard error: {why}"),
        }
    }
}

impl std::error::Error for ClipboardError {}

/// An in-process clipboard that never touches the OS.
///
/// It backs tests and headless hosts, and it is the reference behaviour the OS
/// backend is checked against. Unlike the arboard backend, it holds every
/// representation at once, so it is where the full item model is exercised.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    item: ClipboardItem,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clipboard for MemoryClipboard {
    fn read(&mut self) -> Result<ClipboardItem, ClipboardError> {
        if self.item.is_empty() {
            Err(ClipboardError::Empty)
        } else {
            Ok(self.item.clone())
        }
    }

    fn write(&mut self, item: &ClipboardItem) -> Result<(), ClipboardError> {
        self.item = item.clone();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.item = ClipboardItem::default();
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
    fn memory_clipboard_round_trips_a_multi_format_item() {
        let mut clipboard = MemoryClipboard::new();
        assert!(matches!(clipboard.read(), Err(ClipboardError::Empty)));

        let item = ClipboardItem::new()
            .with_text("a loop")
            .with_html("<b>a loop</b>")
            .with_image(Image {
                width: 1,
                height: 1,
                rgba: vec![10, 20, 30, 255],
            })
            .with_uris(vec!["file:///song.wav".to_string()]);
        clipboard.write(&item).unwrap();

        let read = clipboard.read().unwrap();
        assert_eq!(read, item);
        assert_eq!(read.text(), Some("a loop"));
        assert_eq!(read.html(), Some("<b>a loop</b>"));
        assert_eq!(read.image().unwrap().rgba, vec![10, 20, 30, 255]);
        assert_eq!(read.uris(), Some(["file:///song.wav".to_string()].as_slice()));

        clipboard.clear().unwrap();
        assert!(matches!(clipboard.read(), Err(ClipboardError::Empty)));
    }

    #[test]
    fn formats_are_listed_richest_first() {
        let item = ClipboardItem::new()
            .with_text("t")
            .with_html("<i>t</i>")
            .with_image(Image {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 0],
            })
            .with_uris(vec!["u".to_string()]);
        assert_eq!(
            item.formats(),
            vec![Mime::TextHtml, Mime::ImagePng, Mime::UriList, Mime::TextPlain]
        );
    }

    #[test]
    fn text_convenience_reads_and_writes_over_the_typed_api() {
        let mut clipboard = MemoryClipboard::new();
        assert!(matches!(clipboard.get_text(), Err(ClipboardError::Empty)));
        clipboard.set_text("ab12cd").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), "ab12cd");
        // An item with only an image has no text: get_text reports Empty, not the
        // clipboard being unavailable.
        clipboard
            .write(&ClipboardItem::new().with_image(Image {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }))
            .unwrap();
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
