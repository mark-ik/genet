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
//! Stage 2b of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md`: the
//! backend probe (Stage 1a) plus [`ServalAppRunner`], the serval-native owner
//! of app state + the retained view tree that rebuilds the DOM on state change,
//! plus native click dispatch — an [`on_click`] event view registers a routing
//! path in [`ServalCtx`], and [`ServalAppRunner::dispatch_click`] walks the hit
//! node's ancestor chain and routes a [`PointerClick`] down each registered
//! path via the faithful `xilem_core` message cycle. Still exercised by tests,
//! not a window; the window → hit-test wiring lives in the `pelt-live` host.

use std::cell::RefCell;
use std::rc::Rc;

use layout_dom_api::{LocalName, Namespace, QualName};
use serval_scripted_dom::ScriptedDom;

mod context;
mod element;
mod event;
mod pod;
mod runner;
mod splice;
mod text;

#[cfg(test)]
mod tests;

pub use context::ServalCtx;
pub use element::{Element, El, el};
pub use event::{OnClick, OnClickState, PointerClick, on_click};
pub use pod::{ServalElement, ServalElementMut};
pub use runner::ServalAppRunner;
pub use splice::ServalChildrenSplice;

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
