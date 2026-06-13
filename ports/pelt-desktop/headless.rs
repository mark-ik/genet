/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Headless render + the reftest fixture harness (V3).
//!
//! `pelt --engine headless --out <path> <file>` renders a document GPU-free to a
//! deterministic textual *scene snapshot* (the `netrender::Scene` pelt's viewer would
//! present, dumped stably) — serval's first regression net beyond unit tests. On top
//! of it, [`run_reftests`] walks a directory of `name.html` + `name.scene` fixtures,
//! re-renders each, and compares against the committed snapshot; `--bless` (re)writes
//! the snapshots from the current render.
//!
//! The scene snapshot is the primary, GPU-free artifact (the plan's byte-deterministic
//! comparison). A rasterized PNG "for human eyes" is a deferred follow-up: it needs the
//! offscreen wgpu readback path (serval-wpt's `Renderer`), where the snapshot here
//! needs no GPU at all.

use std::path::{Path, PathBuf};

use crate::document::{LoadedDocument, LocalFetcher};

/// Default reftest viewport. Fixtures are authored against this size; a `--size`
/// override is a follow-up.
pub const DEFAULT_WIDTH: u32 = 800;
pub const DEFAULT_HEIGHT: u32 = 600;

/// Render `url` (a file path / `file://` / `data:`) GPU-free to a deterministic scene
/// snapshot string, unscrolled. The same load → cascade → layout → paint → scene
/// pipeline the on-screen viewer uses, stopping at the GPU-free `netrender::Scene`.
pub fn render_snapshot(url: &str, width: u32, height: u32) -> Result<String, String> {
    render_snapshot_scrolled(url, width, height, (0.0, 0.0))
}

/// Like [`render_snapshot`] but with the document scrolled to `(x, y)` device px first
/// (clamped to the real scroll range) — the harness's "apply a viewport scroll" hook,
/// so the viewport-family fixtures (document scroll, fixed-vs-absolute under scroll)
/// snapshot a *scrolled* scene rather than the static one. A fixture opts in with a
/// `name.scroll` sidecar (`"x y"`).
pub fn render_snapshot_scrolled(
    url: &str,
    width: u32,
    height: u32,
    scroll: (f32, f32),
) -> Result<String, String> {
    let mut doc = LoadedDocument::load(&LocalFetcher, url)?;
    // First frame builds the layout session at this size; scrolling then clamps to the
    // real range, and the second frame paints at the scrolled offset.
    let _ = doc.frame(width, height);
    if scroll != (0.0, 0.0) {
        doc.scroll_by(scroll.0, scroll.1);
    }
    let scene = doc.frame(width, height);
    Ok(snapshot_text(&scene))
}

/// The stable textual form of a scene: pretty-printed structure, with the
/// non-deterministic font-blob handles normalized, plus a trailing newline — so a
/// committed `.scene` fixture is a clean, reproducible text diff.
fn snapshot_text(scene: &netrender::Scene) -> String {
    let mut out = normalize_blob_ids(&format!("{scene:#?}"));
    out.push('\n');
    out
}

/// Replace the per-run font-blob handle ids (`data: Blob { id: <N>, .. }`) with a
/// stable placeholder. The id is a runtime font-cache handle that varies between
/// renders; it is not layout signal — the glyph ids + positions (deterministic) carry
/// the real font selection. Identified structurally: an `id: <N>,` line whose next
/// line is the `Blob` rest pattern `..` (Glyph / transform / font ids are never
/// followed by `..`, so they are untouched).
fn normalize_blob_ids(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = String::with_capacity(raw.len());
    for (i, line) in lines.iter().enumerate() {
        let next_is_blob_rest = lines.get(i + 1).is_some_and(|n| n.trim() == "..");
        if next_is_blob_rest && line.trim_start().starts_with("id: ") {
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(indent);
            out.push_str("id: <font-blob>,\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// One fixture's outcome.
pub enum Outcome {
    /// Snapshot matched (or was written, under `--bless`).
    Pass,
    /// Snapshot differed: the 1-based line number of the first difference, and both
    /// sides' text for a named diff.
    Fail { first_diff_line: usize, expected: String, got: String },
    /// The fixture could not be rendered, or its `.scene` was missing without
    /// `--bless`. Carries a message.
    Error(String),
}

/// A fixture's name paired with its outcome.
pub struct ReftestResult {
    pub name: String,
    pub outcome: Outcome,
}

/// Run every `name.html` fixture under `dir` (sorted), comparing each rendered scene
/// snapshot against the committed `name.scene`. With `bless`, write the snapshot
/// instead of comparing (creating/updating fixtures). Returns one result per fixture.
pub fn run_reftests(
    dir: &Path,
    width: u32,
    height: u32,
    bless: bool,
) -> Result<Vec<ReftestResult>, String> {
    let mut htmls: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("could not read reftest dir {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    htmls.sort();

    let mut results = Vec::new();
    for html in htmls {
        let name = html
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let scene_path = html.with_extension("scene");
        // Optional `name.scroll` sidecar ("x y"): render the fixture scrolled, so the
        // viewport-family scroll cases snapshot a scrolled scene.
        let scroll = read_scroll_sidecar(&html.with_extension("scroll"));
        let outcome = match render_snapshot_scrolled(
            &html.to_string_lossy(),
            width,
            height,
            scroll,
        ) {
            Err(e) => Outcome::Error(e),
            Ok(got) if bless => match std::fs::write(&scene_path, &got) {
                Ok(()) => Outcome::Pass,
                Err(e) => Outcome::Error(format!("could not write {}: {e}", scene_path.display())),
            },
            Ok(got) => match std::fs::read_to_string(&scene_path) {
                Err(_) => Outcome::Error(format!(
                    "no snapshot at {} — run with --bless to create it",
                    scene_path.display()
                )),
                Ok(expected) if expected == got => Outcome::Pass,
                Ok(expected) => {
                    let first_diff_line = first_diff_line(&expected, &got);
                    Outcome::Fail { first_diff_line, expected, got }
                }
            },
        };
        results.push(ReftestResult { name, outcome });
    }
    Ok(results)
}

/// Read a `name.scroll` sidecar holding `"x y"` device-px offsets, or `(0, 0)` if it
/// is absent or unparseable.
fn read_scroll_sidecar(path: &Path) -> (f32, f32) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (0.0, 0.0);
    };
    let mut parts = text.split_whitespace();
    let x = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let y = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (x, y)
}

/// The 1-based line number where two texts first differ (or the line past the shorter,
/// when one is a prefix of the other).
fn first_diff_line(a: &str, b: &str) -> usize {
    let mut a_lines = a.lines();
    let mut b_lines = b.lines();
    let mut line = 1usize;
    loop {
        match (a_lines.next(), b_lines.next()) {
            (Some(x), Some(y)) if x == y => line += 1,
            (None, None) => return line,
            _ => return line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene snapshot is deterministic: rendering the same document twice yields
    /// byte-identical text (the precondition for the whole reftest harness).
    #[test]
    fn snapshot_is_deterministic() {
        let url = "data:text/html,<h1>Hello</h1><p>reftest</p>";
        let a = render_snapshot(url, DEFAULT_WIDTH, DEFAULT_HEIGHT).expect("renders");
        let b = render_snapshot(url, DEFAULT_WIDTH, DEFAULT_HEIGHT).expect("renders");
        assert_eq!(a, b, "the same document renders to the same snapshot");
        assert!(a.contains("GlyphRun"), "the snapshot captures painted text");
    }

    /// A layout change moves the snapshot: different content yields a different scene,
    /// and `first_diff_line` names where (the red-on-change half of the harness).
    #[test]
    fn layout_change_changes_snapshot() {
        let one = render_snapshot("data:text/html,<p>one</p>", DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .expect("renders");
        let two = render_snapshot("data:text/html,<p>two words</p>", DEFAULT_WIDTH, DEFAULT_HEIGHT)
            .expect("renders");
        assert_ne!(one, two, "different content renders to different snapshots");
        assert!(first_diff_line(&one, &two) >= 1, "a diff line is named");
    }
}
