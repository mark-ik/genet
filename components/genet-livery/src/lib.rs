//! Genet's concrete integration path for the clean-room Livery engine.
//!
//! The shared boundary is [`layout_dom_api::LayoutDom`]. Livery values remain
//! concrete on this side of the document router; the Stylo-backed Fullweb path
//! remains concrete inside `genet-layout`.

#![forbid(unsafe_code)]

mod dom;
mod layout;
mod style;

pub use dom::{ElementRef, InteractionStates, SelectorTree};
pub use layout::{Fragment, FragmentPlane, LayoutError, layout};
pub use livery::media::Device;
pub use style::{StylePlane, StyleSet, resolve_styles};

/// Clean-room UA defaults for the bounded Cambium structural lane.
///
/// This deliberately follows the lane contract rather than importing
/// `genet-layout`'s larger Stylo-oriented sheet.
pub const CAMBIUM_UA_DEFAULTS: &str = r#"
html, body, main, section, article, header, footer, nav, aside,
div, h1, h2, h3, h4, h5, h6, p, ul, ol, li, pre {
    display: block;
}

button, input, select, textarea {
    display: inline-block;
}

head, title, meta, link, style, script, template {
    display: none;
}

html { width: 100%; }
body { width: 100%; margin: 8px; }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold; }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold; }
h3 { font-size: 1.17em; margin: 1em 0; font-weight: bold; }
p, ul, ol, pre { margin: 1em 0; }
ul, ol { padding-left: 40px; }
ul { list-style-type: disc; }
ol { list-style-type: decimal; }
pre { white-space: pre; }
"#;
