/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `xilem-serval`: a [`xilem_core`] backend that diffs a Xilem view tree into
//! serval's mutable [`ScriptedDom`](serval_scripted_dom::ScriptedDom).
//!
//! This is the third `xilem_core` backend, beside `xilem` (native, → Masonry)
//! and `xilem_web` (→ the browser DOM via `web_sys`), pointed at serval's
//! [`LayoutDomMut`](layout_dom_api::LayoutDomMut). It is `xilem_web`, but
//! native, with serval as the engine: app state → view tree → diff → DOM
//! mutations, doing no layout, paint, or hit-testing.
//!
//! # The key simplification
//!
//! `xilem_web` carries `Box<dyn AnyNode>` because the browser exposes several
//! distinct node Rust types. In serval every DOM node is one type
//! ([`NodeId`](serval_scripted_dom::NodeId)), so the backend has **no type
//! erasure**: a uniform element type, an identity `SuperElement`, and no
//! downcasts. Mutations are applied **eagerly** to the `ScriptedDom` (each
//! `set_attribute` records a `DomMutation`); serval batches at the
//! `drain_mutations` → relayout boundary, so the deferred-apply-on-`Mut`-drop
//! machinery from `xilem_web` is dropped entirely.
//!
//! # Status
//!
//! Stage 3a of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`: the
//! backend probe (Stage 1a) plus [`ServalAppRunner`], the serval-native owner
//! of app state + the retained view tree that rebuilds the DOM on state change,
//! plus native click dispatch — an [`on_click`] event view registers a routing
//! path in [`ServalCtx`], and [`ServalAppRunner::dispatch_click`] walks the hit
//! node's ancestor chain and routes a [`PointerClick`] down each registered
//! path via the faithful `xilem_core` message cycle. Stage 3a adds *component
//! composition*: `xilem_core`'s generic `lens`/`map_state`/`map_action`/
//! `memoize` views work over [`ServalCtx`] unchanged (re-exported here), and
//! [`on_click`] handlers may return an [`OptionalAction`] that bubbles as a
//! [`MessageResult::Action`](xilem_core::MessageResult::Action) and composes up
//! through `map_action`; [`ServalAppRunner::dispatch_click`] returns the actions
//! that reach the root. Stage 3b adds the *keyboard + focus foundation*: an
//! [`on_key`] view registers a key handler (mirroring [`on_click`]) which also
//! marks its element focusable; [`ServalAppRunner`] tracks a focused node
//! ([`focus`](ServalAppRunner::focus)/[`set_focus`](ServalAppRunner::set_focus)),
//! [`dispatch_click`](ServalAppRunner::dispatch_click) sets focus to the nearest
//! focusable ancestor of the click (click-to-focus), and
//! [`dispatch_key`](ServalAppRunner::dispatch_key) bubble-walks a [`KeyEvent`]
//! from the focused node. Stage 3 adds the first *form control* on that
//! foundation: [`text_field`] is a reusable editable text field whose state is
//! its own [`String`], so it composes onto a larger app's field through
//! [`lens`] like the Stage 3a `counter_button`. The backend stays headless; the
//! window → hit-test and winit→[`KeyEvent`] wiring lives in the `pelt-live`
//! host.

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::{LocalName, Namespace, QualName};
use serval_scripted_dom::ScriptedDom;

mod arrangement;
mod context;
mod controls;
mod grid;
mod element;
mod event;
mod focusable;
mod key;
mod keyed;
mod multi;
mod optional_action;
mod overlay;
mod pod;
mod pointer;
mod portable;
mod propagation;
mod radio;
mod runner;
mod select;
mod slider;
mod splice;
mod styled_field;
mod tags;
mod text;
mod wheel;

#[cfg(test)]
mod tests;

pub use arrangement::{arrangement, placed, placed_with};
pub use context::ServalCtx;
pub use grid::{GridView, data_grid};
pub use controls::{
    Checkbox, TextField, TextInput, button, checkbox, checkbox_typed, text_field, text_field_typed,
    textarea, textarea_typed, toggle,
};
pub use element::{El, Element, ElementView, el};
pub use event::{OnClick, OnClickState, PointerClick, clickable, on_click};
pub use focusable::{Focusable, FocusableState, focusable};
pub use key::{Key, KeyEvent, Modifiers, NamedKey, OnKey, OnKeyState, on_key};
pub use keyed::Keyed;
pub use multi::{ProjectionId, ServalMultiRunner};
pub use optional_action::{Action, OptionalAction};
pub use overlay::{Placement, anchor_point, anchor_point_clamped, overlay_at, overlay_rect};
pub use pod::{ServalElement, ServalElementMut};
pub use pointer::{OnPointer, PointerEvent, PointerPhase, on_pointer};
pub use portable::{PortableKeyed, PortableKeyedState};
pub use propagation::Propagation;
pub use radio::{RadioGroup, radio_group};
pub use runner::ServalAppRunner;
pub use select::{SelectState, select};
pub use slider::{Slider, slider};
pub use splice::ServalChildrenSplice;
pub use styled_field::{FieldChild, StyleRange, styled_text_field, styled_textarea};
// Per-tag element-view helpers: `div`, `span`, `p`, `input`, `label`, `a`,
// `h1`/`h2`/`h3`, `ul`/`ol`/`li`. (No `button` here — `controls::button` is the
// button view, with a handler.)
pub use tags::*;
pub use text::text;
pub use wheel::{OnWheel, WheelEvent, on_wheel};

// The generic, backend-agnostic composition vocabulary from `xilem_core`. These
// views are parametric over any `Context: ViewPathTracker`, so they work over
// `ServalCtx` with no serval-side impl; re-exported here so chrome authors can
// reach the whole vocabulary from `xilem_serval` without a second `use`. The
// `View`/`MessageResult` core traits come along so `impl View<…, ServalCtx, …>`
// return types and the action path can be named from this crate alone.
pub use xilem_core::{
    AnyView, Lens, MessageResult, View, lens, map_action, map_message_result, map_state, memoize,
};

/// The HTML namespace. serval views build elements in this namespace, matching
/// `xilem_web`'s `HTML_NS`.
pub const HTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// A shared, mutable handle to the document every view in a tree mutates.
///
/// A serval-native runner (Stage 1b's `ServalAppRunner`) will own scheduling
/// around this; Stage 1a just shares it between the context, the elements, and
/// the splice.
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
