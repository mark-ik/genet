/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The uniform element type for the serval backend.
//!
//! `xilem_web` needs `Box<dyn AnyNode>` type erasure because the browser's
//! `web_sys::Element`/`Text`/`HtmlInputElement` are distinct Rust types. In
//! serval every DOM node is the *same* type — [`NodeId`] — so there is no type
//! erasure here: the element type is uniform, the "any" element is the same
//! type, and the [`SuperElement<Self, ServalCtx>`] impl is the identity.
//!
//! Unlike `xilem_web`'s `Pod`/`PodMut`, props are applied *eagerly* against the
//! `ScriptedDom` (each `set_attribute`/`remove_attribute` records a
//! `DomMutation` immediately). serval already batches at the
//! `drain_mutations` → relayout boundary, so the deferred-apply-on-drop
//! machinery (`PodMut::drop`) is unnecessary.

use crate::DomHandle;
use crate::context::ServalCtx;
use serval_scripted_dom::NodeId;
use xilem_core::{Mut, SuperElement, ViewElement};

/// A retained backend element: a serval DOM node plus the handle needed to keep
/// mutating it. This is the `View::Element` for every serval view, and also the
/// element type carried by every [`ViewSequence`](xilem_core::ViewSequence)
/// over this backend.
pub struct ServalElement {
    /// The live node in the `ScriptedDom`.
    pub node: NodeId,
    /// Shared handle to the document this node lives in.
    pub dom: DomHandle,
}

impl ServalElement {
    /// Wrap a freshly created node together with its document handle.
    pub fn new(node: NodeId, dom: DomHandle) -> Self {
        Self { node, dom }
    }
}

/// The mutable reference form of [`ServalElement`], handed to
/// [`View::rebuild`](xilem_core::View::rebuild) and
/// [`View::teardown`](xilem_core::View::teardown).
///
/// It borrows the retained `node` (so a view may, in principle, swap it) and
/// carries the document handle by shared clone — mutating the DOM only needs
/// `&mut ScriptedDom` through the `RefCell`, never a borrow of the element.
pub struct ServalElementMut<'a> {
    /// The live node being edited.
    pub node: &'a mut NodeId,
    /// Shared handle to the document this node lives in.
    pub dom: DomHandle,
}

impl ServalElementMut<'_> {
    /// Reborrow this mutable handle for a nested call.
    pub fn reborrow_mut(&mut self) -> ServalElementMut<'_> {
        ServalElementMut {
            node: self.node,
            dom: self.dom.clone(),
        }
    }
}

impl ViewElement for ServalElement {
    type Mut<'a> = ServalElementMut<'a>;
}

impl ServalElement {
    /// Borrow this element as a [`ServalElementMut`].
    pub fn as_mut(&mut self) -> ServalElementMut<'_> {
        ServalElementMut {
            node: &mut self.node,
            dom: self.dom.clone(),
        }
    }
}

// The identity `SuperElement`: because there is exactly one element type, the
// sequence element *is* the child element. `upcast` is a move and the downcast
// is the identity. (Contrast `xilem_web`, where `AnyPod` boxes the concrete
// `Pod<N>` and `with_downcast_val` does a real `downcast_mut`.)
impl SuperElement<Self, ServalCtx> for ServalElement {
    fn upcast(_ctx: &mut ServalCtx, child: Self) -> Self {
        child
    }

    fn with_downcast_val<R>(
        mut this: Mut<'_, Self>,
        f: impl FnOnce(Mut<'_, Self>) -> R,
    ) -> (Self::Mut<'_>, R) {
        let r = f(this.reborrow_mut());
        (this, r)
    }
}
