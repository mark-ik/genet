/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Serval-in-a-browser smoke — the wasm sibling of `netrender_smoke`.
//!
//! Receipt shape:
//! - Register a bundled font (`serval_layout::register_host_font`; wasm has
//!   no system font registry)
//! - Build a woodshed-flavored view tree through `xilem_serval::ServalAppRunner`
//!   into a `ScriptedDom`
//! - `serval_layout::lay_out_content` + `emit_band` -> `PaintList`
//! - `paint_list_render::translate_paint_cmd_stream` -> `netrender::Scene`
//! - `netrender_device::boot_async` (WebGPU backend) + canvas surface
//! - `Renderer::render_vello` -> present
//!
//! On success the page title flips to "SMOKE PASS" (automation hook) and the
//! scene is visible on the canvas.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use netrender::{create_netrender_instance, ColorLoad, NetrenderOptions};
use paint_list_api::PaintList as _;
use serval_layout::{NoImageLoader, ScrollOffsets};
use serval_scripted_dom::ScriptedDom;
use xilem_serval::{el, text, AnyView, ServalAppRunner, ServalCtx, ServalElement, View};

/// Boxed heterogeneous child view (the `NoteChild` pattern from meerkat).
type Child = Box<dyn AnyView<(), (), ServalCtx, ServalElement>>;

const W: u32 = 900;
const H: u32 = 600;

const SHEET: &str = r#"
.root { width: 900px; height: 600px; background-color: #171a21; color: #d7dae0;
        font-family: sans-serif; font-size: 14px; padding: 16px; }
.title { font-size: 18px; color: #e8e2d4; margin-bottom: 12px; }
.pills { display: flex; margin-bottom: 16px; }
.pill { padding: 6px 14px; margin-right: 6px; border-radius: 14px; color: #9aa0ac; }
.pill-active { background-color: #2a2f3a; color: #e8b15c; }
.body { display: flex; }
.side { width: 200px; margin-right: 16px; }
.side-item { padding: 5px 10px; color: #9aa0ac; }
.side-active { background-color: #232833; color: #d7dae0; border-radius: 6px; }
.board { background-color: #1e222b; border-radius: 10px; padding: 14px; }
.string { display: flex; margin-bottom: 8px; }
.fret { width: 44px; height: 26px; }
.dot { width: 22px; height: 22px; border-radius: 11px; background-color: #4a90b8;
       color: #10131a; font-size: 11px; text-align: center; }
.root-dot { background-color: #e8b15c; }
.caption { margin-top: 12px; color: #6b7280; font-size: 12px; }
"#;

/// A minor-pentatonic-ish scatter: (string, fret, is_root) per dot.
const DOTS: &[(usize, usize, bool)] = &[
    (0, 0, false), (0, 3, false), (1, 0, false), (1, 3, true),
    (2, 0, false), (2, 2, true), (3, 0, false), (3, 2, false),
    (4, 0, true), (4, 3, false), (5, 0, false), (5, 3, false),
];

fn string_row(string: usize) -> Child {
    let frets: Vec<Child> = (0..6)
        .map(|fret| {
            let dot = DOTS
                .iter()
                .find(|(s, f, _)| *s == string && *f == fret);
            match dot {
                Some((_, _, is_root)) => {
                    let class = if *is_root { "dot root-dot" } else { "dot" };
                    Box::new(
                        el(
                            "div",
                            (el("div", text(if *is_root { "R" } else { "" }))
                                .attr("class", class),),
                        )
                        .attr("class", "fret"),
                    ) as Child
                }
                None => Box::new(el("div", ()).attr("class", "fret")) as Child,
            }
        })
        .collect();
    Box::new(el("div", frets).attr("class", "string"))
}

fn pill(label: &str, active: bool) -> Child {
    Box::new(el("span", text(label.to_string())).attr(
        "class",
        if active { "pill pill-active" } else { "pill" },
    ))
}

fn side_item(label: &str, active: bool) -> Child {
    Box::new(el("div", text(label.to_string())).attr(
        "class",
        if active { "side-item side-active" } else { "side-item" },
    ))
}

fn view() -> impl View<(), (), ServalCtx, Element = ServalElement> {
    el(
        "div",
        (
            el("div", text("Woodshed — serval web smoke")).attr("class", "title"),
            el(
                "div",
                (
                    pill("Stage", true),
                    pill("Practice", false),
                    pill("Song", false),
                    pill("Rehearsal", false),
                    pill("Settings", false),
                ),
            )
            .attr("class", "pills"),
            el(
                "div",
                (
                    el(
                        "div",
                        (
                            side_item("Minor Pentatonic", true),
                            side_item("Major", false),
                            side_item("Dorian", false),
                            side_item("Mixolydian", false),
                        ),
                    )
                    .attr("class", "side"),
                    el(
                        "div",
                        (
                            string_row(0),
                            string_row(1),
                            string_row(2),
                            string_row(3),
                            string_row(4),
                            string_row(5),
                        ),
                    )
                    .attr("class", "board"),
                ),
            )
            .attr("class", "body"),
            el(
                "div",
                text("xilem-serval → serval-layout → paint list → netrender → WebGPU"),
            )
            .attr("class", "caption"),
        ),
    )
    .attr("class", "root")
}

fn set_title(t: &str) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        doc.set_title(t);
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        match run().await {
            Ok(()) => {
                web_sys::console::log_1(&"serval web smoke: PASS".into());
                set_title("SMOKE PASS");
            }
            Err(e) => {
                web_sys::console::error_1(&format!("serval web smoke FAIL: {e:?}").into());
                set_title("SMOKE FAIL");
            }
        }
    });
}

async fn run() -> Result<(), String> {
    // Stage breadcrumbs go to the title so external automation can see
    // where a hang happens without CDP access.
    set_title("stage: fonts");
    // Fonts: wasm has no system registry; supply Roboto as the sans-serif face.
    serval_layout::register_host_font(
        include_bytes!("../assets/Roboto-Regular.ttf").to_vec(),
    );

    // View tree -> ScriptedDom.
    set_title("stage: dom");
    let dom = Rc::new(RefCell::new(ScriptedDom::new()));
    let runner = ServalAppRunner::new(dom, |_: &()| view(), ());
    let dom = runner.dom();
    let dom_ref = dom.borrow();

    // Layout + paint-list emission.
    set_title("stage: layout");
    let sheets: Vec<&str> = vec![SHEET];
    let layout = serval_layout::lay_out_content(&*dom_ref, &sheets, &NoImageLoader, W, H);
    set_title("stage: emit");
    let (list, _scroll, _links) = layout.emit_band(&*dom_ref, 0, H, &ScrollOffsets::default());
    set_title("stage: translate");
    let translated = paint_list_render::translate_paint_cmd_stream(
        list.viewport(),
        list.commands(),
        list.fonts(),
        list.images(),
    );

    // WebGPU boot through netrender's own async path.
    set_title("stage: boot");
    let handles = netrender_device::boot_async()
        .await
        .map_err(|e| format!("boot_async: {e:?}"))?;
    set_title("stage: surface");

    let canvas: web_sys::HtmlCanvasElement = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("smoke"))
        .ok_or("no #smoke canvas")?
        .dyn_into()
        .map_err(|_| "element #smoke is not a canvas")?;

    let surface = handles
        .instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| format!("create_surface: {e:?}"))?;
    let caps = surface.get_capabilities(&handles.adapter);
    let format = *caps.formats.first().ok_or("no surface formats")?;
    let alpha_mode = *caps.alpha_modes.first().ok_or("no alpha modes")?;
    let device = handles.device.clone();
    let queue = handles.queue.clone();
    surface.configure(
        &device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: W,
            height: H,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        },
    );

    set_title("stage: renderer");
    let renderer = create_netrender_instance(
        handles,
        NetrenderOptions {
            tile_cache_size: Some(1024),
            enable_vello: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("create_netrender_instance: {e:?}"))?;
    set_title("stage: frame");

    // Vello's fine/compose stages write the target as an RGBA8 storage
    // texture; a browser swapchain view is RenderAttachment-only (and
    // BGRA). Rasterize into an intermediate storage texture, then blit
    // to the swapchain (the meerkat shape: scenes never see the
    // swapchain directly).
    let vello_target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke vello target"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let vello_view = vello_target.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_vello(
        &translated.scene,
        &vello_view,
        ColorLoad::Clear(wgpu::Color::BLACK),
    );

    set_title("stage: present");
    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        other => return Err(format!("get_current_texture: {other:?}")),
    };
    let frame_view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let blitter = wgpu::util::TextureBlitter::new(&device, format);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("smoke blit"),
    });
    blitter.copy(&device, &mut encoder, &vello_view, &frame_view);
    queue.submit([encoder.finish()]);
    frame.present();
    Ok(())
}
