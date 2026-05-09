/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Script-free Pelt entrypoint.

use std::env;

use pelt_core::{DeferredShellEngine, EngineProfile, ShellEngine};
use pelt_desktop::{StaticViewerConfig, WindowingMode, run_static_viewer};

use crate::VERSION;

pub(crate) fn main() {
    let mut engine_profile = EngineProfile::Viewer;
    let mut url = None;
    let mut netrender_smoke = false;
    #[cfg(feature = "windows-present")]
    let mut windows_present_smoke = false;
    #[cfg(feature = "macos-present")]
    let mut macos_present_smoke = false;

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
            #[cfg(feature = "windows-present")]
            "--windows-present-smoke" => {
                windows_present_smoke = true;
            },
            #[cfg(feature = "macos-present")]
            "--macos-present-smoke" => {
                macos_present_smoke = true;
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

    #[cfg(feature = "windows-present")]
    if windows_present_smoke {
        run_optional_windows_present_smoke();
        return;
    }

    #[cfg(feature = "macos-present")]
    if macos_present_smoke {
        run_optional_macos_present_smoke();
        return;
    }

    let config = StaticViewerConfig::new(engine.profile(), WindowingMode::Headed, url);
    match run_static_viewer(config) {
        Ok(outcome) => {
            println!(
                "pelt viewer rendered url={} created_window={} redraws={}",
                outcome.url, outcome.created_window, outcome.redraws
            );
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        },
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

#[cfg(feature = "windows-present")]
fn run_optional_windows_present_smoke() {
    let config = pelt_desktop::WindowsDxgiPresentSmokeConfig::default();
    match pelt_desktop::run_windows_dxgi_present_smoke(config) {
        Ok(outcome) => {
            println!(
                "pelt windows-present smoke {}x{} frames={} created_window={}",
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window
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
                outcome.width,
                outcome.height,
                outcome.frames_presented,
                outcome.created_window
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
    --windows-present-smoke   (requires --features windows-present, target_os = \"windows\")
    --macos-present-smoke     (requires --features macos-present, target_vendor = \"apple\")
    --version
    -h, --help"
    );
}
