/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cambium is a Genet-native reactive GUI toolkit. It diffs a [`meristem`] view
//! tree into Genet's mutable [`ScriptedDom`].
//! Cambium owns views, controls, message routing, and application state;
//! Genet owns the DOM, style, layout, paint, input, and accessibility passes.
//!
//! # Backend model
//!
//! Genet exposes one DOM node type, [`NodeId`](genet_scripted_dom::NodeId), so
//! Cambium uses a uniform element without browser-style node erasure or
//! downcasts. Mutations are applied eagerly to the `ScriptedDom`; Genet batches
//! them at its relayout boundary.
//!
//! [`GenetAppRunner`] retains application state and the view tree. Pointer,
//! hover, keyboard, and wheel events route through [`GenetCtx`] using Meristem's
//! message cycle. Focus, composition, standard form controls, keyed movement,
//! overlays, grids, menus, and searchable action lists are provided here while
//! platform event translation and presentation remain host responsibilities.

use std::cell::RefCell;
use std::rc::Rc;

use genet_scripted_dom::ScriptedDom;
use layout_dom_api::{LocalName, Namespace, QualName};

mod action_list;
mod arrangement;
mod command_surface;
mod context;
mod controls;
mod detail_popover;
mod disclosure;
mod editor;
mod element;
mod event;
mod focus;
mod focus_request;
mod focusable;
mod graph_canvas;
mod grid;
#[cfg(feature = "highlight")]
mod highlight;
mod hover;
mod key;
mod keyed;
mod menu;
mod multi;
mod optional_action;
mod overlay;
mod overlay_surface;
mod pod;
mod pointer;
mod portable;
mod propagation;
mod radio;
mod reorderable_list;
mod runner;
mod select;
mod selection_bar;
mod slider;
mod splice;
mod styled_field;
mod summary_body;
mod tags;
mod text;
mod wheel;

#[cfg(test)]
mod tests;

pub use action_list::{ActionItem, ActionListEvent, ActionListState, action_list};
pub use arrangement::{arrangement, placed, placed_with};
pub use command_surface::{
    CommandEvent, CommandItem, CommandState, CommandSurfaceKind, command_menu, command_palette,
    command_picker, command_surface,
};
pub use context::GenetCtx;
pub use detail_popover::{
    DetailPopoverEvent, DetailPopoverMode, DetailPopoverState, detail_popover,
};
pub use disclosure::{
    AccordionConfig, AccordionItem, AccordionMode, AccordionState, DisclosureState, TreeItem,
    TreeSelectionMode, TreeState, accordion, accordion_with, disclosure, disclosure_with,
    tree_view,
};
pub use editor::{EditHistory, pair_close, wrap_selection};
pub use graph_canvas::{
    GRAPH_CANVAS_SWATCH_CSS, GraphCanvasEdge, GraphCanvasNode, GraphCanvasSubgraph,
    GraphCanvasSwatch, graph_canvas_swatch, graph_canvas_swatch_with_focus,
};
pub use grid::{GridView, data_grid};
pub use menu::{MENU_CLASS, MENU_ROW_ACTIVE_CLASS, MENU_ROW_CLASS, menu};
// Re-export the grid's spec types from Sprigging so a host building a `data_grid`
// needs no second direct `sprigging` dependency. The grid widget's home
// is here; its column model rides along.
pub use controls::{
    Checkbox, TextField, TextInput, button, button_with, checkbox, checkbox_typed, text_field,
    text_field_typed, textarea, textarea_typed, toggle,
};
pub use element::{El, Element, ElementView, el};
pub use event::{OnClick, OnClickState, PointerClick, clickable, on_click};
pub use focus::{FocusEvent, FocusPhase, OnFocus, OnFocusState, on_focus};
pub use focus_request::{FocusRequest, FocusRequestState, request_focus};
pub use focusable::{Focusable, FocusableState, focusable, focusable_if};
#[cfg(feature = "highlight")]
pub use highlight::{
    Highlight, entity_styles, highlighted_text_field, highlighted_textarea, note_styles,
    role_class, styles_for, syntax_css,
};
pub use hover::{HoverEvent, HoverPhase, OnHover, OnHoverState, on_hover};
pub use key::{Key, KeyEvent, Modifiers, NamedKey, OnKey, OnKeyState, on_key};
pub use keyed::Keyed;
pub use multi::{GenetMultiRunner, ProjectionId};
pub use optional_action::{Action, OptionalAction};
pub use overlay::{Placement, anchor_point, anchor_point_clamped, overlay_at, overlay_rect};
pub use overlay_surface::{OverlayDismiss, OverlayRole, OverlaySurface, overlay_surface};
pub use pod::{GenetElement, GenetElementMut};
pub use pointer::{OnPointer, PointerEvent, PointerPhase, on_pointer};
pub use portable::{PortableKeyed, PortableKeyedState};
pub use propagation::Propagation;
pub use radio::{RadioGroup, radio_group};
pub use reorderable_list::{
    ReorderItem, ReorderMove, ReorderState, reorderable_list, reorderable_list_with,
};
pub use runner::GenetAppRunner;
pub use select::{SelectState, select};
pub use selection_bar::{
    Orientation, SelectionBarConfig, SelectionBarKind, SelectionItem, SelectionState,
    TabActivation, filter_chips, segmented_control, selection_bar, tab_bar,
};
pub use slider::{Slider, slider};
pub use splice::GenetChildrenSplice;
pub use sprigging::{GraphCanvas, GraphViewport, GridColumn, GridSpec};
pub use styled_field::{FieldChild, StyleRange, styled_text_field, styled_textarea};
pub use summary_body::{SummaryBody, summary_body};
// Per-tag element-view helpers: `div`, `span`, `p`, `input`, `label`, `a`,
// `h1`/`h2`/`h3`, `ul`/`ol`/`li`. (No `button` here — `controls::button` is the
// button view, with a handler.)
pub use tags::*;
pub use text::text;
pub use wheel::{OnWheel, WheelEvent, on_wheel};

// Compatibility aliases for consumers that still use the pre-extraction
// backend names. New code should use the canonical `Genet*` names above.
#[deprecated(note = "renamed to GenetCtx")]
pub use context::GenetCtx as ServalCtx;
#[deprecated(note = "renamed to GenetMultiRunner")]
pub use multi::GenetMultiRunner as ServalMultiRunner;
#[deprecated(note = "renamed to GenetElement")]
pub use pod::GenetElement as ServalElement;
#[deprecated(note = "renamed to GenetElementMut")]
pub use pod::GenetElementMut as ServalElementMut;
#[deprecated(note = "renamed to GenetAppRunner")]
pub use runner::GenetAppRunner as ServalAppRunner;
#[deprecated(note = "renamed to GenetChildrenSplice")]
pub use splice::GenetChildrenSplice as ServalChildrenSplice;

// The generic, backend-agnostic composition vocabulary from `meristem`. These
// views are parametric over any `Context: ViewPathTracker`, so they work over
// `GenetCtx` without backend-specific adapters; re-exported here so chrome authors can
// reach the whole vocabulary from `cambium` without a second `use`. The
// `View`/`MessageResult` core traits come along so `impl View<…, GenetCtx, …>`
// return types and the action path can be named from this crate alone.
pub use meristem::{
    AnyView, Lens, MessageResult, View, lens, map_action, map_message_result, map_state, memoize,
};

/// The HTML namespace. Cambium views build elements in this namespace, matching
/// `xilem_web`'s `HTML_NS`.
pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// A shared, mutable handle to the document every view in a tree mutates.
///
/// [`GenetAppRunner`] owns scheduling around this shared handle.
pub type DomHandle = Rc<RefCell<ScriptedDom>>;

/// Build an HTML-namespaced [`QualName`] for `local` (no prefix).
pub fn html_qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(HTML_NS), LocalName::from(local))
}

/// Build an unnamespaced [`QualName`] for an attribute named `local`.
///
/// HTML attributes are in the null namespace (mirroring how `set_attribute`
/// works in the browser), distinct from the element's HTML namespace.
pub fn attr_qual(local: &str) -> QualName {
    QualName::new(None, Namespace::from(""), LocalName::from(local))
}
