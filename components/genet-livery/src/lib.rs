//! Genet's concrete integration path for the clean-room Livery engine.
//!
//! The shared boundary is [`layout_dom_api::LayoutDom`]. Livery values remain
//! concrete on this side of the document router; the Stylo-backed Fullweb path
//! remains concrete inside `genet-layout`.

#![forbid(unsafe_code)]

mod document;
mod dom;
mod invalidation;
mod layout;
mod paint;
mod style;
mod text;

pub use document::{ClickOutcome, LinkTarget, LiveryDocument};
pub use dom::{ElementRef, InteractionStates, SelectorTree};
pub use invalidation::{AttributeSnapshot, ElementSnapshot, IncrementalStyle, RestyleStats};
pub(crate) use layout::hit_test_with_scroll;
pub use layout::{
    Fragment, FragmentPlane, LayoutError, content_box_size, hit_test, layout,
    resolve_container_query_styles, resolve_container_relative_styles,
    used_value_context,
};
pub use livery::media::{Device, ViewportSize, ViewportSizes};
pub use livery::stylesheet::RuleMutationError;
pub use livery::{PropertyId, canonicalize_specified_longhand, canonicalize_specified_value};
pub(crate) use paint::emit_paint_list_with_text_system_scrolled_with_images;
pub use paint::{LiveryPaintList, emit_paint_list, emit_paint_list_with_text_system};
pub use style::{StylePlane, StyleSet, UsedValueContext, resolve_styles};
pub use text::TextSystem;

/// Clean-room UA defaults for the bounded Cambium structural lane.
///
/// This deliberately follows the lane contract rather than importing
/// `genet-layout`'s larger Stylo-oriented sheet.
pub const CAMBIUM_UA_DEFAULTS: &str = r#"
html, body, main, section, article, header, footer, nav, aside,
div, h1, h2, h3, h4, h5, h6, p, ul, ol, li, pre {
    display: block;
}

table { display: table; }
thead, tbody, tfoot { display: table-row-group; }
tr { display: table-row; }
td, th { display: table-cell; }
caption { display: table-caption; }

button, input, select, textarea {
    display: inline-block;
}

img {
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
