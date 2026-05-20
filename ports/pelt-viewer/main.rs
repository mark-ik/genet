/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Pelt viewer binary: a Xilem window showing serval-rendered HTML.
//!
//! The nav bar's input is a path to an HTML file (loaded + rendered) or,
//! when it isn't a readable file, the built-in sample page. "Go"
//! re-renders synchronously — fine for static content.

use dpi::LogicalSize;
use masonry::peniko::ImageData;
use xilem::core::one_of::OneOf2;
use xilem::view::{
    FlexExt, ObjectFit, flex_col, flex_row, image as image_view, label, text_button, text_input,
};
use xilem::{EventLoop, WidgetView, WindowOptions, Xilem};

use pelt_viewer::render_html;

const VIEWPORT_W: u32 = 800;
const VIEWPORT_H: u32 = 600;

const SAMPLE_HTML: &str = "<html><body>\
<h1>Pelt Viewer</h1>\
<p>This page is parsed, cascaded, laid out, and painted by \
<span class=\"hot\">serval</span>, then rendered through \
<span class=\"cool\">netrender</span> and shown in a \
<span class=\"hot\">Xilem</span> window.</p>\
<div class=\"box\">A bordered block with its own background.</div>\
</body></html>";

const SAMPLE_CSS: &[&str] = &[
    "body { background-color: rgb(250, 250, 252); color: rgb(30, 30, 40); }",
    "h1 { color: rgb(20, 40, 90); }",
    ".hot { color: rgb(200, 40, 60); font-weight: bold; }",
    ".cool { color: rgb(30, 110, 170); font-weight: bold; }",
    ".box { background-color: rgb(230, 236, 245); border: 3px solid rgb(120, 140, 180); }",
];

struct AppState {
    nav_input: String,
    current_image: Option<ImageData>,
}

/// Render the nav input: a readable file path → its HTML (no extra
/// stylesheet); otherwise the built-in sample page.
fn render_input(input: &str) -> Option<ImageData> {
    let (html, css): (String, Vec<&str>) = match std::fs::read_to_string(input) {
        Ok(contents) => (contents, Vec::new()),
        Err(_) => (SAMPLE_HTML.to_string(), SAMPLE_CSS.to_vec()),
    };
    match render_html(&html, &css, VIEWPORT_W, VIEWPORT_H) {
        Ok(img) => Some(img),
        Err(err) => {
            eprintln!("[pelt-viewer] render failed: {err}");
            None
        },
    }
}

fn app_logic(state: &mut AppState) -> impl WidgetView<AppState> + use<> {
    let nav_bar = flex_row((
        text_input(
            state.nav_input.clone(),
            |state: &mut AppState, new_text: String| {
                state.nav_input = new_text;
            },
        )
        .flex(1.0),
        text_button("Go", |state: &mut AppState| {
            state.current_image = render_input(&state.nav_input);
        }),
    ));

    let content = if let Some(img) = state.current_image.clone() {
        OneOf2::A(image_view(img).fit(ObjectFit::Contain))
    } else {
        OneOf2::B(label("No content rendered."))
    };

    flex_col((nav_bar, content.flex(1.0)))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let initial = std::env::args().nth(1).unwrap_or_else(|| "sample".to_string());
    let app_state = AppState {
        current_image: render_input(&initial),
        nav_input: initial,
    };

    let window_options = WindowOptions::new("Pelt Viewer")
        .with_min_inner_size(LogicalSize::new(VIEWPORT_W as f64, VIEWPORT_H as f64 + 48.0));

    Xilem::new_simple(app_state, app_logic, window_options).run_in(EventLoop::with_user_event())?;
    Ok(())
}
