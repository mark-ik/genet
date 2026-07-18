//! A static-file [`Handler`]: serve a directory tree, gemtext-first.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;

use super::server::{Handler, Request, SpartanResponse};

/// Serves files under a root directory. A path ending in `/` serves that
/// directory's `index.gmi`. Uploads are refused with a client error (bring
/// your own [`Handler`] for upload endpoints). Requests that %-decode to
/// something escaping the root are refused.
#[derive(Debug, Clone)]
pub struct FileHandler {
    root: PathBuf,
}

impl FileHandler {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn resolve(&self, request_path: &str) -> Option<PathBuf> {
        let decoded = percent_decode_str(request_path).decode_utf8().ok()?;
        let mut resolved = self.root.clone();
        for segment in decoded.split('/') {
            match segment {
                "" | "." => continue,
                ".." => return None,
                segment if segment.contains(['\\', ':']) => return None,
                segment => resolved.push(segment),
            }
        }
        if decoded.ends_with('/') || decoded == "" {
            resolved.push("index.gmi");
        }
        Some(resolved)
    }
}

/// The extension → MIME table (spartan's preferred document is gemtext).
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "gmi" | "gemini" => "text/gemini",
        "txt" | "" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

impl Handler for FileHandler {
    async fn handle(&self, request: Request) -> SpartanResponse {
        if !request.data.is_empty() {
            return SpartanResponse::ClientError {
                message: "This server does not accept uploads.".to_string(),
            };
        }
        let Some(path) = self.resolve(&request.path) else {
            return SpartanResponse::ClientError {
                message: "Bad path.".to_string(),
            };
        };
        // A bare directory hit (no trailing slash) redirects to the slash
        // form, so relative links inside the document resolve correctly.
        if path.is_dir() {
            return SpartanResponse::Redirect {
                path: format!("{}/", request.path),
            };
        }
        match tokio::fs::read(&path).await {
            Ok(body) => SpartanResponse::Success {
                mime: mime_for(&path).to_string(),
                body,
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                SpartanResponse::ClientError {
                    message: format!("File {} not found.", request.path),
                }
            },
            Err(_) => SpartanResponse::ServerError {
                message: "Could not read file.".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_maps_paths_and_refuses_escapes() {
        let handler = FileHandler::new("/srv/spartan");
        assert_eq!(
            handler.resolve("/notes.txt").unwrap(),
            PathBuf::from("/srv/spartan").join("notes.txt")
        );
        assert_eq!(
            handler.resolve("/").unwrap(),
            PathBuf::from("/srv/spartan").join("index.gmi")
        );
        assert_eq!(
            handler.resolve("/sub/").unwrap(),
            PathBuf::from("/srv/spartan").join("sub").join("index.gmi")
        );
        assert!(handler.resolve("/../etc/passwd").is_none());
        assert!(
            handler.resolve("/%2e%2e/etc/passwd").is_none(),
            "encoded dots"
        );
        assert!(handler.resolve("/a%5Cb").is_none(), "backslash segments");
    }

    #[test]
    fn mime_table_prefers_gemtext() {
        assert_eq!(mime_for(Path::new("index.gmi")), "text/gemini");
        assert_eq!(mime_for(Path::new("a.txt")), "text/plain");
        assert_eq!(mime_for(Path::new("README")), "text/plain");
        assert_eq!(mime_for(Path::new("x.bin")), "application/octet-stream");
    }
}
