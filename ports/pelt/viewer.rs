/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Script-free Pelt entrypoint.

use std::env;

use pelt_core::{DeferredShellEngine, EngineProfile, ShellEngine};

use crate::VERSION;

pub(crate) fn main() {
    let mut engine_profile = EngineProfile::Viewer;
    let mut url = None;
    // JS backend for `--engine scripted` (boa default; nova needs --features
    // scripted-nova). Parsed as a string so the flag exists even in builds without
    // the scripted profile.
    let mut js_engine = String::from("boa");
    // Headless profile (V3 reftest harness): `--out <path>` writes a scene snapshot;
    // `--reftest <dir>` runs a fixture directory; `--bless` (re)writes the snapshots.
    let mut out_path: Option<String> = None;
    let mut reftest_dir: Option<String> = None;
    let mut bless = false;
    // Chrome demo (V2): wrap the content viewer in an omnibar + back/forward strip.
    let mut with_chrome = false;
    let mut strip_side = String::from("top");
    // Tiles demo (V5): split the window into tiles, one document each.
    let mut with_tiles = false;
    let mut tile_urls: Vec<String> = Vec::new();
    let mut netrender_smoke = false;
    let mut webgl_wgpu_smoke = false;
    #[cfg(feature = "windows-present")]
    let mut windows_present_smoke = false;
    #[cfg(feature = "windows-present")]
    let mut windows_present_surfaces_smoke = false;
    #[cfg(feature = "macos-present")]
    let mut macos_present_smoke = false;
    #[cfg(feature = "macos-present")]
    let mut macos_present_surfaces_smoke = false;
    #[cfg(feature = "linux-present")]
    let mut wayland_present_smoke = false;
    #[cfg(feature = "linux-present")]
    let mut wayland_present_surfaces_smoke = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return;
            },
            "--version" => {
                println!("{VERSION}");
                return;
            },
            "--engine" => {
                let Some(value) = args.next() else {
                    eprintln!("--engine requires browser, viewer, static, or headless");
                    std::process::exit(2);
                };
                engine_profile = parse_engine_profile(&value);
            },
            value if value.starts_with("--engine=") => {
                engine_profile = parse_engine_profile(&value["--engine=".len()..]);
            },
            "--js" => {
                let Some(value) = args.next() else {
                    eprintln!("--js requires boa or nova");
                    std::process::exit(2);
                };
                js_engine = value;
            },
            value if value.starts_with("--js=") => {
                js_engine = value["--js=".len()..].to_owned();
            },
            "--out" => {
                let Some(value) = args.next() else {
                    eprintln!("--out requires a path");
                    std::process::exit(2);
                };
                out_path = Some(value);
            },
            value if value.starts_with("--out=") => {
                out_path = Some(value["--out=".len()..].to_owned());
            },
            "--reftest" => {
                let Some(value) = args.next() else {
                    eprintln!("--reftest requires a fixture directory");
                    std::process::exit(2);
                };
                reftest_dir = Some(value);
            },
            value if value.starts_with("--reftest=") => {
                reftest_dir = Some(value["--reftest=".len()..].to_owned());
            },
            "--bless" => {
                bless = true;
            },
            "--chrome" => {
                with_chrome = true;
            },
            "--tiles" => {
                with_tiles = true;
            },
            "--strip" => {
                let Some(value) = args.next() else {
                    eprintln!("--strip requires top, bottom, left, or right");
                    std::process::exit(2);
                };
                strip_side = value;
            },
            value if value.starts_with("--strip=") => {
                strip_side = value["--strip=".len()..].to_owned();
            },
            "--netrender-smoke" => {
                netrender_smoke = true;
            },
            "--webgl-wgpu-smoke" => {
                webgl_wgpu_smoke = true;
            },
            #[cfg(feature = "windows-present")]
            "--windows-present-smoke" => {
                windows_present_smoke = true;
            },
            #[cfg(feature = "windows-present")]
            "--windows-present-surfaces-smoke" => {
                windows_present_surfaces_smoke = true;
            },
            #[cfg(feature = "macos-present")]
            "--macos-present-smoke" => {
                macos_present_smoke = true;
            },
            #[cfg(feature = "macos-present")]
            "--macos-present-surfaces-smoke" => {
                macos_present_surfaces_smoke = true;
            },
            #[cfg(feature = "linux-present")]
            "--wayland-present-smoke" => {
                wayland_present_smoke = true;
            },
            #[cfg(feature = "linux-present")]
            "--wayland-present-surfaces-smoke" => {
                wayland_present_surfaces_smoke = true;
            },
            value if value.starts_with('-') => {
                eprintln!("unsupported script-free viewer option: {value}");
                std::process::exit(2);
            },
            value => {
                url = Some(value.to_owned());
                tile_urls.push(value.to_owned());
            },
        }
    }

    if engine_profile.is_browser() {
        eprintln!("pelt does not host the old browser engine profile");
        std::process::exit(2);
    }

    let engine = DeferredShellEngine::new(engine_profile);
    let capabilities = engine.capabilities();
    let url = url.unwrap_or_else(|| "about:blank".to_owned());
    println!(
        "pelt viewer profile={} url={} javascript={} webdriver={} devtools={} webgpu={} webxr={}",
        engine.profile(),
        url,
        capabilities.javascript,
        capabilities.webdriver,
        capabilities.devtools,
        capabilities.webgpu,
        capabilities.webxr
    );

    if netrender_smoke {
        // Pre-retirement this fell through into the viewer window; with the
        // viewer gone the smoke is the whole run, exiting clean.
        run_optional_netrender_smoke();
        return;
    }

    if webgl_wgpu_smoke {
        run_optional_webgl_wgpu_smoke();
        return;
    }

    #[cfg(feature = "windows-present")]
    if windows_present_smoke {
        run_optional_windows_present_smoke();
        return;
    }

    #[cfg(feature = "windows-present")]
    if windows_present_surfaces_smoke {
        run_optional_windows_present_surfaces_smoke();
        return;
    }

    #[cfg(feature = "macos-present")]
    if macos_present_smoke {
        run_optional_macos_present_smoke();
        return;
    }

    #[cfg(feature = "macos-present")]
    if macos_present_surfaces_smoke {
        run_optional_macos_present_surfaces_smoke();
        return;
    }

    #[cfg(feature = "linux-present")]
    if wayland_present_smoke {
        run_optional_wayland_present_smoke();
        return;
    }

    #[cfg(feature = "linux-present")]
    if wayland_present_surfaces_smoke {
        run_optional_wayland_present_surfaces_smoke();
        return;
    }

    // `pelt --engine static|viewer <url>`: the genet-native on-screen document
    // viewer (the orrery-host present shape over the pelt-core / pelt-desktop
    // contracts). Static and Viewer are the script-free document profiles.
    if matches!(
        engine_profile,
        EngineProfile::Static | EngineProfile::Viewer
    ) {
        // A smolweb scheme (gemini/gopher/…) renders natively through the smolweb
        // viewer (errand transport + parse + native themed view), not the HTML path.
        #[cfg(feature = "smolweb")]
        if is_smolweb_url(&url) {
            run_smolweb_profile(url, strip_side.clone(), engine_profile);
            return;
        }
        // `--tiles`: split the window into tiles, one document each (V5's tile surface).
        if with_tiles {
            run_tiles_profile(tile_urls);
            return;
        }
        // `--chrome`: wrap the content in a xilem-serval omnibar + back/forward strip
        // (V2's two-root browser shell).
        if with_chrome {
            run_chrome_profile(url, strip_side, engine_profile);
            return;
        }
        let config = pelt_desktop::StaticViewerConfig::new(
            engine_profile,
            pelt_desktop::WindowingMode::Headed,
            url,
        );
        match pelt_desktop::run_static_viewer(config) {
            Ok(outcome) => {
                println!(
                    "pelt static viewer url={} window={} redraws={}",
                    outcome.url, outcome.created_window, outcome.redraws
                );
                return;
            },
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            },
        }
    }

    // `pelt --engine scripted <url> [--js boa|nova]`: the live, script-driven
    // document profile (V4). Runs the page's inline <script> on the chosen engine and
    // renders the mutated DOM, driving timers + the GC tick at frame cadence.
    if matches!(engine_profile, EngineProfile::Scripted) {
        run_scripted_profile(url, js_engine);
        return;
    }

    // `pelt --engine headless`: GPU-free scene-snapshot render (V3 reftest harness).
    // `--reftest <dir>` runs a fixture directory; otherwise `--out <path>` (or stdout)
    // writes the snapshot for `<file>`.
    if matches!(engine_profile, EngineProfile::Headless) {
        run_headless_profile(url, out_path, reftest_dir, bless);
        return;
    }

    eprintln!(
        "pelt has no engine for profile {engine_profile} in this build; use \
         --engine static <url> for the on-screen document viewer, \
         --engine scripted <url> for the scripted profile (needs --features scripted), \
         or a smoke flag (--help)."
    );
    std::process::exit(2);
}

/// Whether `url` is a smolweb scheme the native smolweb viewer handles.
#[cfg(feature = "smolweb")]
fn is_smolweb_url(url: &str) -> bool {
    [
        "gemini://",
        "gopher://",
        "nex://",
        "finger://",
        "spartan://",
        "guppy://",
    ]
    .iter()
    .any(|scheme| url.starts_with(scheme))
}

/// Dispatch a smolweb URL to the chrome browser over a native smolweb content root:
/// omnibar + back/forward + link navigation, the same shell `--chrome` uses for HTML.
#[cfg(feature = "smolweb")]
fn run_smolweb_profile(url: String, side: String, profile: EngineProfile) {
    use pelt_desktop::StripSide;
    let side = match side.to_ascii_lowercase().as_str() {
        "top" => StripSide::Top,
        "bottom" => StripSide::Bottom,
        "left" => StripSide::Left,
        "right" => StripSide::Right,
        other => {
            eprintln!("--strip expects top, bottom, left, or right (got '{other}')");
            std::process::exit(2);
        },
    };
    let thickness = if matches!(side, StripSide::Left | StripSide::Right) {
        280
    } else {
        40
    };
    let config =
        pelt_desktop::StaticViewerConfig::new(profile, pelt_desktop::WindowingMode::Headed, url);
    match pelt_desktop::run_smolweb_browser(config, side, thickness) {
        Ok(outcome) => println!(
            "pelt smolweb browser url={} window={} redraws={}",
            outcome.url, outcome.created_window, outcome.redraws
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

/// Dispatch the scripted profile to the on-screen scripted viewer on the chosen JS
/// backend. Present only when built with `--features scripted`.
#[cfg(feature = "scripted")]
fn run_scripted_profile(url: String, js: String) {
    let Some(engine) = pelt_desktop::ScriptedEngine::parse(&js) else {
        eprintln!("--js expects boa or nova (got '{js}')");
        std::process::exit(2);
    };
    let config = pelt_desktop::StaticViewerConfig::new(
        EngineProfile::Scripted,
        pelt_desktop::WindowingMode::Headed,
        url,
    );
    match pelt_desktop::run_scripted_viewer(config, engine) {
        Ok(outcome) => println!(
            "pelt scripted viewer engine={} url={} window={} redraws={}",
            engine.label(),
            outcome.url,
            outcome.created_window,
            outcome.redraws
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

/// Dispatch `--tiles` to the tile viewer: a window split into tiles, one document per
/// content URL. Present only when built with `--features tiles`.
#[cfg(feature = "tiles")]
fn run_tiles_profile(urls: Vec<String>) {
    match pelt_desktop::run_tile_viewer(urls, pelt_desktop::WindowingMode::Headed) {
        Ok(outcome) => println!(
            "pelt tile viewer url={} window={} redraws={}",
            outcome.url, outcome.created_window, outcome.redraws
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

/// Without the tiles demo compiled in, `--tiles` is a clean error pointing at the
/// feature to enable.
#[cfg(not(feature = "tiles"))]
fn run_tiles_profile(_urls: Vec<String>) {
    eprintln!("pelt was built without the tiles demo; rebuild with `--features tiles`");
    std::process::exit(2);
}

/// Dispatch `--chrome` to the two-root browser shell: the content viewer wrapped in a
/// xilem-serval omnibar + back/forward strip on the chosen side. Present only when
/// built with `--features chrome`.
#[cfg(feature = "chrome")]
fn run_chrome_profile(url: String, side: String, profile: EngineProfile) {
    use pelt_desktop::StripSide;
    let side = match side.to_ascii_lowercase().as_str() {
        "top" => StripSide::Top,
        "bottom" => StripSide::Bottom,
        "left" => StripSide::Left,
        "right" => StripSide::Right,
        other => {
            eprintln!("--strip expects top, bottom, left, or right (got '{other}')");
            std::process::exit(2);
        },
    };
    // A vertical strip wants room for the toolbar; a horizontal one is a thin bar.
    let thickness = if matches!(side, StripSide::Left | StripSide::Right) {
        280
    } else {
        40
    };
    let config =
        pelt_desktop::StaticViewerConfig::new(profile, pelt_desktop::WindowingMode::Headed, url);
    match pelt_desktop::run_chrome_viewer(config, side, thickness) {
        Ok(outcome) => println!(
            "pelt chrome viewer url={} window={} redraws={}",
            outcome.url, outcome.created_window, outcome.redraws
        ),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

/// Without the chrome demo compiled in, `--chrome` is a clean error pointing at the
/// feature to enable.
#[cfg(not(feature = "chrome"))]
fn run_chrome_profile(_url: String, _side: String, _profile: EngineProfile) {
    eprintln!("pelt was built without the chrome demo; rebuild with `--features chrome`");
    std::process::exit(2);
}

/// Without the scripted profile compiled in, `--engine scripted` is a clean error
/// pointing at the feature to enable.
#[cfg(not(feature = "scripted"))]
fn run_scripted_profile(_url: String, _js: String) {
    eprintln!(
        "pelt was built without the scripted profile; rebuild with `--features scripted` \
         (or `--features scripted-nova` for the Nova backend)"
    );
    std::process::exit(2);
}

/// The headless reftest harness. With `reftest`, run every `name.html` fixture in the
/// directory against its committed `name.scene` snapshot (or write them under `bless`);
/// otherwise render `url` to a single scene snapshot, to `out` (or stdout). Exits
/// non-zero on any reftest failure / error. (Always available here — viewer.rs is
/// wholly under `viewer-engine`, which enables pelt-desktop's viewer stack.)
fn run_headless_profile(url: String, out: Option<String>, reftest: Option<String>, bless: bool) {
    use pelt_desktop::{DEFAULT_HEIGHT, DEFAULT_WIDTH, Outcome, render_snapshot, run_reftests};

    if let Some(dir) = reftest {
        let results = match run_reftests(
            std::path::Path::new(&dir),
            DEFAULT_WIDTH,
            DEFAULT_HEIGHT,
            bless,
        ) {
            Ok(results) => results,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            },
        };
        let (mut passed, mut failed, mut errored) = (0u32, 0u32, 0u32);
        for result in &results {
            match &result.outcome {
                Outcome::Pass => {
                    passed += 1;
                    println!("  ok     {}", result.name);
                },
                Outcome::Fail {
                    first_diff_line, ..
                } => {
                    failed += 1;
                    println!(
                        "  FAIL   {} (first diff at line {first_diff_line})",
                        result.name
                    );
                },
                Outcome::PngFail { detail } => {
                    failed += 1;
                    println!("  FAIL   {} (png: {detail})", result.name);
                },
                Outcome::Error(message) => {
                    errored += 1;
                    println!("  ERROR  {} ({message})", result.name);
                },
            }
        }
        let blessed = if bless { " (blessed)" } else { "" };
        println!("reftest: {passed} passed, {failed} failed, {errored} errored{blessed}");
        if failed > 0 || errored > 0 {
            std::process::exit(1);
        }
        return;
    }

    // `--out *.png`: the rasterized "for human eyes" artifact instead of the scene
    // text snapshot. GPU-required, so behind the png-reftest feature.
    if out.as_deref().is_some_and(|p| p.ends_with(".png")) {
        let path = out.expect("checked Some above");
        write_headless_png(&url, &path);
        return;
    }

    match render_snapshot(&url, DEFAULT_WIDTH, DEFAULT_HEIGHT) {
        Ok(snapshot) => match out {
            Some(path) => match std::fs::write(&path, &snapshot) {
                Ok(()) => println!("pelt headless wrote {} bytes to {path}", snapshot.len()),
                Err(error) => {
                    eprintln!("could not write {path}: {error}");
                    std::process::exit(1);
                },
            },
            None => print!("{snapshot}"),
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

/// Render `url` to a PNG and write it to `path`. Present only with `--features
/// png-reftest` (it boots wgpu); without it, a clean pointer to the feature.
#[cfg(feature = "png-reftest")]
fn write_headless_png(url: &str, path: &str) {
    use pelt_desktop::{DEFAULT_HEIGHT, DEFAULT_WIDTH, render_png};
    match render_png(url, DEFAULT_WIDTH, DEFAULT_HEIGHT) {
        Ok(png) => match std::fs::write(path, &png) {
            Ok(()) => println!("pelt headless wrote {} PNG bytes to {path}", png.len()),
            Err(error) => {
                eprintln!("could not write {path}: {error}");
                std::process::exit(1);
            },
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(not(feature = "png-reftest"))]
fn write_headless_png(_url: &str, _path: &str) {
    eprintln!(
        "pelt was built without the PNG lane; rebuild with `--features png-reftest` to \
         write a .png (the GPU-free .scene snapshot needs no feature)."
    );
    std::process::exit(2);
}

fn run_optional_netrender_smoke() {
    match pelt_desktop::run_netrender_smoke() {
        Ok(outcome) => {
            println!(
                "pelt netrender smoke rendered {}x{} painted_pixels={}",
                outcome.width, outcome.height, outcome.painted_pixels
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

fn run_optional_webgl_wgpu_smoke() {
    match pelt_desktop::run_webgl_wgpu_smoke() {
        Ok(outcome) => {
            println!(
                "pelt webgl-wgpu smoke rendered {}x{} painted_pixels={} canvas_center={:?} overlay_center={:?}",
                outcome.width,
                outcome.height,
                outcome.painted_pixels,
                outcome.canvas_center,
                outcome.overlay_center
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "windows-present")]
fn run_optional_windows_present_smoke() {
    let config = pelt_desktop::WindowsDxgiPresentSmokeConfig::default();
    match pelt_desktop::run_windows_dxgi_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt windows-present smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "windows-present")]
fn run_optional_windows_present_surfaces_smoke() {
    let config = pelt_desktop::WindowsDxgiPresentSmokeConfig {
        title: "pelt — windows-dxgi present smoke (with declared surface)".into(),
        declare_subsurface: true,
        frames: 0,
        ..pelt_desktop::WindowsDxgiPresentSmokeConfig::default()
    };
    match pelt_desktop::run_windows_dxgi_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt windows-present surfaces smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "macos-present")]
fn run_optional_macos_present_smoke() {
    let config = pelt_desktop::MacosCALayerPresentSmokeConfig::default();
    match pelt_desktop::run_macos_calayer_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt macos-present smoke {}x{} frames={} created_window={}",
                outcome.width, outcome.height, outcome.frames_presented, outcome.created_window
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "macos-present")]
fn run_optional_macos_present_surfaces_smoke() {
    // Same shape as the basic smoke but flips `declare_subsurface`
    // on so the smoke also exercises the per-`SurfaceKey`
    // declare/destroy/present path through `MacosCALayerBackend`.
    // Visual: red full-viewport master with a green top-left
    // quarter; the per-surface CALayer overlays the green region
    // at 50% opacity, producing a yellow-ish blend if the
    // per-surface path is correctly composited above the master
    // CALayer.
    let config = pelt_desktop::MacosCALayerPresentSmokeConfig {
        title: "pelt — macos-calayer present smoke (with declared surface)".into(),
        declare_subsurface: true,
        // `frames: 0` keeps the window open until the user closes
        // it, so they can take a screenshot at their leisure
        // (instead of the basic smoke's auto-exit after ~1s).
        frames: 0,
        ..pelt_desktop::MacosCALayerPresentSmokeConfig::default()
    };
    match pelt_desktop::run_macos_calayer_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt macos-present surfaces smoke {}x{} frames={} created_window={}",
                outcome.width, outcome.height, outcome.frames_presented, outcome.created_window
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "linux-present")]
fn run_optional_wayland_present_smoke() {
    let config = pelt_desktop::WaylandPresentSmokeConfig::default();
    match pelt_desktop::run_wayland_subsurface_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt wayland-present smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

#[cfg(feature = "linux-present")]
fn run_optional_wayland_present_surfaces_smoke() {
    // Same shape as the basic smoke but flips `declare_subsurface`
    // on and runs frames=0 (held until window close) so the per-
    // surface composition is visible long enough for the visual
    // receipt: red master + green declared-quarter at 50% opacity
    // producing olive blend where they compose.
    let config = pelt_desktop::WaylandPresentSmokeConfig {
        title: "pelt — wayland-subsurface present smoke (with declared surface)".into(),
        declare_subsurface: true,
        frames: 0,
        ..pelt_desktop::WaylandPresentSmokeConfig::default()
    };
    match pelt_desktop::run_wayland_subsurface_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt wayland-present surfaces smoke {}x{} frames={} created_window={} declared_subsurface={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window,
                outcome.declared_subsurface
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
    }
}

fn parse_engine_profile(value: &str) -> EngineProfile {
    match value.parse() {
        Ok(profile) => profile,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        },
    }
}

fn print_help() {
    println!(
        "\
pelt {VERSION}

Usage: pelt [--engine <profile>] [<url-or-file>] [options]

Script-free Pelt: genet's reference browser. `--engine static <url-or-file>`
opens the genet-native on-screen document viewer (file://, a bare path, and
data: URLs — percent-encoded or base64; http(s) needs --features netfetch);
`--chrome` wraps it in an omnibar + back/forward strip, `--tiles` splits the
window into per-document tiles. The
other profiles: `--engine scripted` runs a page's <script> (needs --features
scripted), `--engine headless` is the GPU-free scene-snapshot / reftest harness.
Smoke runners validate the present backends (--help lists them).

Options:
    --engine <browser|viewer|static|scripted|headless>
    --chrome                           (wrap the content viewer in an omnibar + back/forward strip; needs --features chrome)
    --strip <top|bottom|left|right>    (chrome strip side; default top)
    --tiles <url>...                   (split the window into tiles, one document each; needs --features tiles)
    --js <boa|nova>                    (scripted profile: JS backend; nova needs --features scripted-nova)
    --out <path>                       (headless profile: write the scene snapshot for <file>)
    --reftest <dir>                    (headless profile: run a name.html + name.scene fixture dir)
    --bless                            (headless --reftest: (re)write the .scene snapshots)
    --netrender-smoke
    --webgl-wgpu-smoke
    --windows-present-smoke            (requires --features windows-present, target_os = \"windows\")
    --windows-present-surfaces-smoke   (same as --windows-present-smoke + a declared compositor surface)
    --macos-present-smoke              (requires --features macos-present, target_vendor = \"apple\")
    --macos-present-surfaces-smoke     (same as --macos-present-smoke + a declared compositor surface)
    --wayland-present-smoke            (requires --features linux-present, target_os = \"linux\")
    --wayland-present-surfaces-smoke   (same as --wayland-present-smoke + a declared compositor surface)
    --version
    -h, --help"
    );
}
