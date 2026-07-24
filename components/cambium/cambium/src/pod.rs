/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The uniform element type for the Genet backend.
//!
//! `xilem_web` needs `Box<dyn AnyNode>` type erasure because the browser's
//! `web_sys::Element`/`Text`/`HtmlInputElement` are distinct Rust types. In
//! Genet every DOM node is the *same* type — [`NodeId`] — so there is no type
//! erasure here: the element type is uniform, the "any" element is the same
//! type, and the [`SuperElement<Self, GenetCtx>`] impl is the identity.
//!
//! Unlike `xilem_web`'s `Pod`/`PodMut`, props are applied *eagerly* against the
//! `ScriptedDom` (each `set_attribute`/`remove_attribute` records a
//! `DomMutation` immediately). Genet already batches at the
//! `drain_mutations` → relayout boundary, so the deferred-apply-on-drop
//! machinery (`PodMut::drop`) is unnecessary.

use crate::DomHandle;
use crate::context::GenetCtx;
use genet_scripted_dom::NodeId;
use layout_dom_api::LayoutDomMut;
use meristem::{AnyElement, Mut, SuperElement, ViewElement};

/// A retained backend element: a Genet DOM node plus the handle needed to keep
/// mutating it. This is the `View::Element` for every Genet view, and also the
/// element type carried by every [`ViewSequence`](meristem::ViewSequence)
/// over this backend.
pub struct GenetElement {
    /// The live node in the `ScriptedDom`.
    pub node: NodeId,
    /// Shared handle to the document this node lives in.
    pub dom: DomHandle,
}

impl GenetElement {
    /// Wrap a freshly created node together with its document handle.
    pub fn new(node: NodeId, dom: DomHandle) -> Self {
        Self { node, dom }
    }
}

/// The mutable reference form of [`GenetElement`], handed to
/// [`View::rebuild`](meristem::View::rebuild) and
/// [`View::teardown`](meristem::View::teardown).
///
/// It borrows the retained `node` (so a view may, in principle, swap it) and
/// carries the document handle by shared clone — mutating the DOM only needs
/// `&mut ScriptedDom` through the `RefCell`, never a borrow of the element.
pub struct GenetElementMut<'a> {
    /// The live node being edited.
    pub node: &'a mut NodeId,
    /// Shared handle to the document this node lives in.
    pub dom: DomHandle,
    /// The parent node this element is attached under, if any. Threaded in by
    /// whoever holds the parent (the children splice; the runner for the root),
    /// so [`AnyElement::replace_inner`] can swap the node *in place* under its
    /// parent on a type-changing [`AnyView`](meristem::AnyView) rebuild.
    /// `None` for a detached element (no in-place swap possible).
    pub parent: Option<NodeId>,
}

impl GenetElementMut<'_> {
    /// Reborrow this mutable handle for a nested call.
    pub fn reborrow_mut(&mut self) -> GenetElementMut<'_> {
        GenetElementMut {
            node: self.node,
            dom: self.dom.clone(),
            parent: self.parent,
        }
    }
}

impl ViewElement for GenetElement {
    type Mut<'a> = GenetElementMut<'a>;
}

impl GenetElement {
    /// Borrow this element as a [`GenetElementMut`] with no known parent
    /// (a detached / standalone borrow — `replace_inner` cannot swap in place).
    pub fn as_mut(&mut self) -> GenetElementMut<'_> {
        GenetElementMut {
            node: &mut self.node,
            dom: self.dom.clone(),
            parent: None,
        }
    }
}

// The identity `SuperElement`: because there is exactly one element type, the
// sequence element *is* the child element. `upcast` is a move and the downcast
// is the identity. (Contrast `xilem_web`, where `AnyPod` boxes the concrete
// `Pod<N>` and `with_downcast_val` does a real `downcast_mut`.)
impl SuperElement<Self, GenetCtx> for GenetElement {
    fn upcast(_ctx: &mut GenetCtx, child: Self) -> Self {
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

// `AnyElement` lets a `Box<dyn AnyView>` swap its concrete inner view for one of
// a *different* type at rebuild. The element type is still uniform (`NodeId`),
// so there is no boxing/downcast as in `xilem_web`'s `AnyPod` — but the node in
// the DOM does change, so `replace_inner` performs the in-place node swap.
impl AnyElement<Self, GenetCtx> for GenetElement {
    fn replace_inner(this: Self::Mut<'_>, child: Self) -> Self::Mut<'_> {
        // On a type-changing `AnyView` rebuild, the old view was torn down but
        // its node is still attached under `parent`, and `child`'s node was just
        // built detached. Splice `child` into the old node's slot: insert it
        // before the old node, remove the old node, and repoint the reference.
        if let Some(parent) = this.parent {
            let mut dom = this.dom.borrow_mut();
            dom.insert_before(parent, child.node, Some(*this.node));
            dom.remove(*this.node);
        }
        *this.node = child.node;
        this
    }
}
