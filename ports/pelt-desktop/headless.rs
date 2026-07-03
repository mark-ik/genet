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

/// Render `url` GPU-fully to an RGBA8 PNG — the "for human eyes" reftest artifact
/// (and the source for an optional `name.png` fixture comparison). Boots wgpu and
/// renders the same `netrender::Scene` the snapshot lane captures, then reads the
/// master back and PNG-encodes it (mirrors `smoke_netrender`). The canvas clears to
/// white so a page without its own background reads like a browser default.
///
/// GPU-required, hence the `png-reftest` feature gate — the scene-snapshot lane above
/// stays GPU-free.
#[cfg(feature = "png-reftest")]
pub fn render_png(url: &str, width: u32, height: u32) -> Result<Vec<u8>, String> {
    render_png_scrolled(url, width, height, (0.0, 0.0))
}

/// Like [`render_png`] but with the document scrolled to `(x, y)` first — the
/// `name.scroll` hook, mirroring [`render_snapshot_scrolled`].
#[cfg(feature = "png-reftest")]
pub fn render_png_scrolled(
    url: &str,
    width: u32,
    height: u32,
    scroll: (f32, f32),
) -> Result<Vec<u8>, String> {
    let mut doc = LoadedDocument::load(&LocalFetcher, url)?;
    let _ = doc.frame(width, height);
    if scroll != (0.0, 0.0) {
        doc.scroll_by(scroll.0, scroll.1);
    }
    let scene = doc.frame(width, height);

    let handles = netrender::boot().map_err(|e| format!("netrender wgpu boot failed: {e}"))?;
    let device = handles.device.clone();
    let renderer = netrender::create_netrender_instance(
        handles,
        netrender::NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("netrender renderer init failed: {e:?}"))?;

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("pelt headless png target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor {
        label: Some("pelt headless png view"),
        format: Some(wgpu::TextureFormat::Rgba8Unorm),
        ..Default::default()
    });
    renderer.render_vello(
        &scene,
        &view,
        netrender::ColorLoad::Clear(wgpu::Color::WHITE),
    );

    // Readback is tightly-packed width*height*4 RGBA8 (no row padding), so it feeds
    // `RgbaImage::from_raw` directly.
    let bytes = renderer
        .wgpu_device
        .read_rgba8_texture(&target, width, height);
    let img = image::RgbaImage::from_raw(width, height, bytes)
        .ok_or_else(|| "readback byte length did not match width*height*4".to_string())?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(png)
}

/// Per-fixture PNG fuzz tolerance (the WPT-reftest shape): a max per-channel delta
/// and the max fraction of pixels allowed to exceed it — absorbing anti-aliasing and
/// driver jitter so a rendered PNG can be compared across machines. Overridable per
/// fixture via a `name.fuzz` sidecar (`"maxDelta maxFraction"`).
#[cfg(feature = "png-reftest")]
#[derive(Clone, Copy, Debug)]
pub struct Fuzz {
    pub max_channel_delta: u8,
    pub max_diff_fraction: f64,
}

#[cfg(feature = "png-reftest")]
impl Default for Fuzz {
    fn default() -> Self {
        Self {
            max_channel_delta: 2,
            max_diff_fraction: 0.001,
        }
    }
}

#[cfg(feature = "png-reftest")]
fn read_fuzz_sidecar(path: &Path) -> Fuzz {
    let default = Fuzz::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return default;
    };
    let mut parts = text.split_whitespace();
    Fuzz {
        max_channel_delta: parts
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default.max_channel_delta),
        max_diff_fraction: parts
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default.max_diff_fraction),
    }
}

/// Compare two encoded PNGs under `fuzz`. `Ok(())` if within tolerance; `Err` with the
/// pixel-diff stats otherwise (or on a size mismatch / decode failure).
#[cfg(feature = "png-reftest")]
pub fn png_within_fuzz(expected_png: &[u8], got_png: &[u8], fuzz: Fuzz) -> Result<(), String> {
    let exp = image::load_from_memory(expected_png)
        .map_err(|e| format!("decode expected png: {e}"))?
        .to_rgba8();
    let got = image::load_from_memory(got_png)
        .map_err(|e| format!("decode got png: {e}"))?
        .to_rgba8();
    if exp.dimensions() != got.dimensions() {
        return Err(format!(
            "png size {:?} != expected {:?}",
            got.dimensions(),
            exp.dimensions()
        ));
    }
    let total = (exp.width() as u64 * exp.height() as u64).max(1) as f64;
    let over = exp
        .pixels()
        .zip(got.pixels())
        .filter(|(a, b)| {
            a.0.iter()
                .zip(b.0.iter())
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0)
                > fuzz.max_channel_delta
        })
        .count();
    let frac = over as f64 / total;
    if frac > fuzz.max_diff_fraction {
        Err(format!(
            "{over} px ({:.3}%) exceed channel-delta {} (budget {:.3}%)",
            frac * 100.0,
            fuzz.max_channel_delta,
            fuzz.max_diff_fraction * 100.0,
        ))
    } else {
        Ok(())
    }
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
    Fail {
        first_diff_line: usize,
        expected: String,
        got: String,
    },
    /// The scene snapshot matched but the optional rasterized `name.png` differed
    /// beyond its fuzz tolerance (only reachable under `png-reftest`). Carries the
    /// pixel-diff detail.
    PngFail { detail: String },
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
        #[allow(unused_mut)]
        let mut outcome =
            match render_snapshot_scrolled(&html.to_string_lossy(), width, height, scroll) {
                Err(e) => Outcome::Error(e),
                Ok(got) if bless => match std::fs::write(&scene_path, &got) {
                    Ok(()) => Outcome::Pass,
                    Err(e) => {
                        Outcome::Error(format!("could not write {}: {e}", scene_path.display()))
                    },
                },
                Ok(got) => match std::fs::read_to_string(&scene_path) {
                    Err(_) => Outcome::Error(format!(
                        "no snapshot at {} — run with --bless to create it",
                        scene_path.display()
                    )),
                    Ok(expected) if expected == got => Outcome::Pass,
                    Ok(expected) => {
                        let first_diff_line = first_diff_line(&expected, &got);
                        Outcome::Fail {
                            first_diff_line,
                            expected,
                            got,
                        }
                    },
                },
            };

        // Rasterized-PNG lane (additive): the `.scene` is primary; a `name.png` is
        // compared only when present, under a fuzz threshold (the GPU-rendered PNG can
        // jitter across machines). `--bless` (re)writes it. GPU-required, so gated.
        #[cfg(feature = "png-reftest")]
        {
            let png_path = html.with_extension("png");
            let url = html.to_string_lossy();
            if bless && !matches!(outcome, Outcome::Error(_)) {
                if let Ok(png) = render_png_scrolled(&url, width, height, scroll) {
                    let _ = std::fs::write(&png_path, &png);
                }
            } else if !bless && png_path.exists() && matches!(outcome, Outcome::Pass) {
                // Only a scene-Pass is upgraded to a PNG check — a scene Fail/Error is
                // the primary signal and stands.
                outcome = match render_png_scrolled(&url, width, height, scroll) {
                    Ok(got) => match std::fs::read(&png_path) {
                        Ok(expected) => {
                            let fuzz = read_fuzz_sidecar(&html.with_extension("fuzz"));
                            match png_within_fuzz(&expected, &got, fuzz) {
                                Ok(()) => Outcome::Pass,
                                Err(detail) => Outcome::PngFail { detail },
                            }
                        },
                        Err(e) => {
                            Outcome::Error(format!("could not read {}: {e}", png_path.display()))
                        },
                    },
                    Err(e) => Outcome::Error(format!("png render failed: {e}")),
                };
            }
        }

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
        let two = render_snapshot(
            "data:text/html,<p>two words</p>",
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
        )
        .expect("renders");
        assert_ne!(one, two, "different content renders to different snapshots");
        assert!(first_diff_line(&one, &two) >= 1, "a diff line is named");
    }

    /// `render_png` boots wgpu and produces a decodable PNG at the requested size —
    /// the GPU half of the V3 lane. (GPU-required; only built under `png-reftest`.)
    #[cfg(feature = "png-reftest")]
    #[test]
    fn render_png_produces_a_valid_image() {
        let png = render_png(
            "data:text/html,<h1>Hello</h1><p>pixels</p>",
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
        )
        .expect("renders a png");
        let img = image::load_from_memory(&png)
            .expect("decodes as png")
            .to_rgba8();
        assert_eq!(
            img.dimensions(),
            (DEFAULT_WIDTH, DEFAULT_HEIGHT),
            "png is the viewport size"
        );
    }

    /// The fuzz comparison tolerates sub-threshold jitter but catches a real color
    /// difference — the comparison half of the V3 lane (no GPU needed).
    #[cfg(feature = "png-reftest")]
    #[test]
    fn png_fuzz_tolerates_jitter_but_catches_real_diffs() {
        let encode = |rgba: [u8; 4]| {
            let img = image::RgbaImage::from_pixel(8, 8, image::Rgba(rgba));
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            png
        };
        let base = encode([100, 100, 100, 255]);
        let jitter = encode([101, 101, 101, 255]); // per-channel delta 1 <= 2 (default)
        let different = encode([200, 100, 100, 255]); // delta 100 on every pixel
        let fuzz = Fuzz::default();
        assert!(
            png_within_fuzz(&base, &jitter, fuzz).is_ok(),
            "1-level jitter is within fuzz"
        );
        assert!(
            png_within_fuzz(&base, &different, fuzz).is_err(),
            "a real color diff is caught"
        );
    }
}
