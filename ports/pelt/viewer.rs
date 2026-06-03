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
        run_optional_netrender_smoke();
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

    // Launch the Xilem viewer (serval content + chrome). `url` is the
    // first nav input — a file path renders that HTML, anything else
    // (e.g. "about:blank") falls back to the built-in sample page.
    if let Err(error) = pelt_viewer::run(Some(url)) {
        eprintln!("{error}");
        std::process::exit(1);
    }
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

Usage: pelt --engine viewer [URL]

Script-free Pelt validation entrypoint.

Options:
    --engine <browser|viewer|static|headless>
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
