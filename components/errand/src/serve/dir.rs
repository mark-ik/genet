//! A [`Source`] over a directory of files: the self-serve case.
//!
//! This is the capsule, the gopherhole, the `.plan` — one author, a directory,
//! served. No coordination machinery, because a single-author publication has
//! no coordination problem. The moot projection is the *other* implementation
//! of the same trait, and it is where authority lives.

use std::path::{Component, Path, PathBuf};

use super::{Entry, EntryKind, Item, Listing, Source, SourceRequest};

/// Serve a directory tree.
#[derive(Clone, Debug)]
pub struct Directory {
    root: PathBuf,
    /// Filenames tried, in order, when a request lands on a directory.
    index_names: Vec<String>,
    /// Whether a directory with no index file is listed rather than refused.
    pub list_directories: bool,
}

impl Directory {
    /// Serve `root`, with the usual index names and directory listing on.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            index_names: vec![
                "index.gmi".into(),
                "index.gemini".into(),
                "gophermap".into(),
                "index.txt".into(),
            ],
            list_directories: true,
        }
    }

    /// Replace the index filenames tried for a directory request.
    pub fn with_index_names(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.index_names = names.into_iter().collect();
        self
    }

    /// Resolve a request path to a path inside the root, or `None` if it
    /// escapes.
    ///
    /// The guard is structural rather than textual: the path is walked
    /// component by component and any `..` pops, so a request can never name
    /// anything above the root however it is spelled or encoded. Comparing
    /// canonicalized strings instead would be a race and would follow symlinks
    /// out of the tree.
    fn resolve(&self, request_path: &str) -> Option<PathBuf> {
        let mut safe = PathBuf::new();
        for component in Path::new(request_path).components() {
            match component {
                Component::Normal(part) => safe.push(part),
                Component::ParentDir => {
                    // Refuse rather than clamp: a request that climbs out is a
                    // request we should not quietly reinterpret.
                    if !safe.pop() {
                        return None;
                    }
                },
                // Leading `/`, `.`, and Windows prefixes carry no meaning here.
                Component::RootDir | Component::CurDir => {},
                Component::Prefix(_) => return None,
            }
        }
        Some(self.root.join(safe))
    }
}

impl Source for Directory {
    async fn get(&self, request: &SourceRequest) -> Option<Item> {
        let path = self.resolve(&request.path)?;
        let metadata = tokio::fs::metadata(&path).await.ok()?;

        if metadata.is_file() {
            let body = tokio::fs::read(&path).await.ok()?;
            return Some(Item::Document {
                mime: mime_for(&path).to_string(),
                body,
            });
        }

        if !metadata.is_dir() {
            return None;
        }

        // A directory: try its index files before listing it.
        for name in &self.index_names {
            let candidate = path.join(name);
            if let Ok(body) = tokio::fs::read(&candidate).await {
                return Some(Item::Document {
                    mime: mime_for(&candidate).to_string(),
                    body,
                });
            }
        }

        if !self.list_directories {
            return None;
        }
        Some(Item::Listing(listing(&path, &request.path).await?))
    }
}

/// Build a listing for a directory, sorted with directories first so a reader
/// meets structure before leaves.
async fn listing(path: &Path, request_path: &str) -> Option<Listing> {
    let mut read = tokio::fs::read_dir(path).await.ok()?;
    let mut entries = Vec::new();

    while let Ok(Some(item)) = read.next_entry().await {
        let name = item.file_name().to_string_lossy().to_string();
        // Dotfiles are not published. A capsule author who wants one served
        // can name it without the dot.
        if name.starts_with('.') {
            continue;
        }
        let is_dir = item.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        let target = join_request_path(request_path, &name, is_dir);
        entries.push(Entry::new(
            if is_dir { format!("{name}/") } else { name.clone() },
            target,
            if is_dir {
                EntryKind::Directory
            } else {
                kind_for(Path::new(&name))
            },
        ));
    }

    entries.sort_by(|a, b| {
        let order = |k: EntryKind| u8::from(k != EntryKind::Directory);
        order(a.kind)
            .cmp(&order(b.kind))
            .then_with(|| a.display.cmp(&b.display))
    });

    Some(Listing {
        title: Some(request_path.to_string()),
        preamble: Vec::new(),
        entries,
    })
}

/// Join a request path and a child name, keeping exactly one separator.
fn join_request_path(base: &str, name: &str, is_dir: bool) -> String {
    let mut out = String::from(base);
    if !out.ends_with('/') {
        out.push('/');
    }
    out.push_str(name);
    if is_dir {
        out.push('/');
    }
    out
}

/// A MIME type from a file extension. Deliberately short: this is the set the
/// small web actually carries, and an unknown extension is opaque bytes rather
/// than a guess.
pub fn mime_for(path: &Path) -> &'static str {
    match extension(path).as_deref() {
        Some("gmi") | Some("gemini") => "text/gemini",
        Some("txt") | Some("md") | Some("plan") => "text/plain",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ivg") => "image/x-iconvg",
        Some("xml") | Some("atom") | Some("rss") => "application/xml",
        _ => "application/octet-stream",
    }
}

/// The coarse entry kind for a filename, for protocols that carry item types.
fn kind_for(path: &Path) -> EntryKind {
    match mime_for(path) {
        m if m.starts_with("text/") => EntryKind::Text,
        m if m.starts_with("image/") => EntryKind::Image,
        m if m.starts_with("audio/") => EntryKind::Sound,
        _ => EntryKind::Binary,
    }
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scheme;

    fn dir() -> Directory {
        Directory::new("/srv/capsule")
    }

    fn request(path: &str) -> SourceRequest {
        SourceRequest {
            scheme: Scheme::Gemini,
            path: path.into(),
            query: None,
        }
    }

    #[test]
    fn an_ordinary_path_resolves_under_the_root() {
        assert_eq!(
            dir().resolve("/notes/hello.gmi"),
            Some(PathBuf::from("/srv/capsule").join("notes").join("hello.gmi"))
        );
    }

    #[test]
    fn traversal_is_refused_however_it_is_spelled() {
        // The whole point of the guard. Each of these tries to leave the root.
        for attempt in [
            "/../etc/passwd",
            "/notes/../../etc/passwd",
            "/../../..",
            "/a/../../b",
        ] {
            assert_eq!(dir().resolve(attempt), None, "{attempt} should be refused");
        }
    }

    #[test]
    fn an_interior_parent_segment_is_allowed_because_it_stays_inside() {
        // `/a/b/../c` never leaves the root, so it resolves to `/a/c`.
        assert_eq!(
            dir().resolve("/a/b/../c"),
            Some(PathBuf::from("/srv/capsule").join("a").join("c"))
        );
    }

    #[test]
    fn current_directory_segments_are_ignored() {
        assert_eq!(
            dir().resolve("/./notes/./hello.gmi"),
            Some(PathBuf::from("/srv/capsule").join("notes").join("hello.gmi"))
        );
    }

    #[test]
    fn the_root_itself_resolves() {
        assert_eq!(dir().resolve("/"), Some(PathBuf::from("/srv/capsule")));
    }

    #[test]
    fn mime_types_cover_what_the_small_web_carries() {
        assert_eq!(mime_for(Path::new("a.gmi")), "text/gemini");
        assert_eq!(mime_for(Path::new("a.GMI")), "text/gemini", "case-insensitive");
        assert_eq!(mime_for(Path::new("a.txt")), "text/plain");
        assert_eq!(mime_for(Path::new("a.png")), "image/png");
        assert_eq!(mime_for(Path::new("a.ivg")), "image/x-iconvg");
        assert_eq!(
            mime_for(Path::new("a.wat")),
            "application/octet-stream",
            "an unknown extension is opaque, not a guess"
        );
    }

    #[test]
    fn entry_kinds_follow_the_mime_family() {
        assert_eq!(kind_for(Path::new("a.gmi")), EntryKind::Text);
        assert_eq!(kind_for(Path::new("a.png")), EntryKind::Image);
        assert_eq!(kind_for(Path::new("a.bin")), EntryKind::Binary);
    }

    #[test]
    fn child_paths_keep_exactly_one_separator() {
        assert_eq!(join_request_path("/", "a.gmi", false), "/a.gmi");
        assert_eq!(join_request_path("/notes", "a.gmi", false), "/notes/a.gmi");
        assert_eq!(join_request_path("/notes/", "a.gmi", false), "/notes/a.gmi");
        assert_eq!(join_request_path("/notes", "sub", true), "/notes/sub/");
    }

    #[tokio::test]
    async fn a_missing_file_is_none_rather_than_an_error() {
        let found = Directory::new(std::env::temp_dir())
            .get(&request("/definitely-not-here-42.gmi"))
            .await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn a_traversing_request_never_reaches_the_filesystem() {
        let found = Directory::new(std::env::temp_dir())
            .get(&request("/../../etc/passwd"))
            .await;
        assert!(found.is_none());
    }
}
