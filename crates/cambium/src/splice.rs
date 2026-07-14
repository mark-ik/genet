/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The [`ElementSplice`] that turns a view-sequence diff into ordered child
//! mutations on a serval node.
//!
//! Modelled on `xilem_web`'s `DomChildrenSplice`, but:
//!   * the retained child collection is `Vec<GenetElement>` (no `AnyPod`
//!     boxing — the element type is uniform);
//!   * inserts/removes are applied *eagerly* against the `ScriptedDom`
//!     (`insert_before` / `remove`), since serval batches at the
//!     `drain_mutations` boundary rather than at a `Mut` drop;
//!   * there is no `DocumentFragment`: serval has no detached-batch primitive,
//!     so `with_scratch` inserts each built child individually (the recorded
//!     `DomMutation`s are identical to inserting them one by one anyway).

use crate::pod::{GenetElement, GenetElementMut};
use layout_dom_api::LayoutDomMut;
use meristem::{AppendVec, ElementSplice, Mut};
use serval_scripted_dom::NodeId;

/// An append-only cursor over a retained `Vec`, ported from `xilem_web`'s
/// `VecSplice`. Elements before the cursor are processed; elements at/after it
/// are pending and (once the tail is cleared) held reversed on `scratch` so the
/// next pending element is `scratch.last()`.
struct VecSplice<'v, 's, T> {
    v: &'v mut Vec<T>,
    scratch: &'s mut Vec<T>,
    ix: usize,
}

impl<'v, 's, T> VecSplice<'v, 's, T> {
    fn new(v: &'v mut Vec<T>, scratch: &'s mut Vec<T>) -> Self {
        Self { v, scratch, ix: 0 }
    }

    fn skip(&mut self, n: usize) {
        let v_len = self.v.len();
        let new_ix = self.ix + n;
        if v_len < new_ix {
            let s_len = self.scratch.len();
            if v_len + s_len < new_ix {
                unreachable!("This is a bug, please report an issue about `ElementSplice::skip`");
            }
            let new_scratch_len = s_len - (new_ix - v_len);
            self.v.extend(self.scratch.splice(new_scratch_len.., []));
        }
        self.ix += n;
    }

    fn delete_next(&mut self) -> T {
        self.clear_tail();
        self.scratch
            .pop()
            .expect("This is a bug, please report an issue about `ElementSplice::delete`")
    }

    fn insert(&mut self, value: T) {
        self.clear_tail();
        self.v.push(value);
        self.ix += 1;
    }

    /// The next *pending* (not-yet-processed) element, i.e. the one currently
    /// at the logical cursor. Pending elements live at `v[ix..]` until the tail
    /// is cleared, after which they sit reversed on `scratch` (so the next one
    /// is `scratch.last()`).
    ///
    /// (`xilem_web`'s `VecSplice::next_mut` returns `v[ix + 1]` — the element
    /// *after* the cursor — because the web splice batches inserts through a
    /// `DocumentFragment` and reads the reference differently. Here we insert
    /// eagerly one node at a time, so the reference is the element *at* the
    /// cursor.)
    fn next_pending(&mut self) -> Option<&mut T> {
        if self.ix < self.v.len() {
            self.v.get_mut(self.ix)
        } else {
            self.scratch.last_mut()
        }
    }

    fn mutate(&mut self) -> &mut T {
        if self.v.len() == self.ix {
            self.v.push(self.scratch.pop().unwrap());
        }
        let ix = self.ix;
        self.ix += 1;
        &mut self.v[ix]
    }

    fn clear_tail(&mut self) {
        if self.v.len() > self.ix {
            self.scratch.extend(self.v.splice(self.ix.., []).rev());
        }
    }

    /// The pending element at relative offset `n` (`0` = the next pending one).
    /// Normalizes the pending queue onto `scratch` (reversed) first.
    fn pending_at(&mut self, n: usize) -> Option<&T> {
        self.clear_tail();
        self.scratch
            .len()
            .checked_sub(1 + n)
            .and_then(|i| self.scratch.get(i))
    }

    /// Reorder the pending queue so the element at relative offset `n` becomes
    /// the next pending one; the displaced elements keep their relative order.
    /// `false` when there is no pending element at `n`.
    fn hoist_pending(&mut self, n: usize) -> bool {
        self.clear_tail();
        let Some(i) = self.scratch.len().checked_sub(1 + n) else {
            return false;
        };
        // Pending order is `scratch` reversed, so moving the element to the
        // END of `scratch` makes it the next pending one.
        let hoisted = self.scratch.remove(i);
        self.scratch.push(hoisted);
        true
    }

    /// Take the next pending element out of the queue (the vec bookkeeping half
    /// of [`ElementSplice::extract_pending`] — no store side effects here).
    fn take_next_pending(&mut self) -> Option<T> {
        self.clear_tail();
        self.scratch.pop()
    }

    /// Push a foreign element in as the next pending one (the vec bookkeeping
    /// half of [`ElementSplice::adopt_pending`]).
    fn push_next_pending(&mut self, value: T) {
        self.clear_tail();
        self.scratch.push(value);
    }
}

/// [`ElementSplice`] managing the children of one serval node in place.
///
/// `parent` is the node whose children are being diffed; `dom` is the shared
/// document handle. The retained children live in `children`; `scratch` is the
/// `VecSplice`'s reversed-tail scratch space.
pub struct GenetChildrenSplice<'a, 'b, 'c> {
    scratch: &'a mut AppendVec<GenetElement>,
    children: VecSplice<'b, 'c, GenetElement>,
    ix: usize,
    parent: NodeId,
    dom: crate::DomHandle,
    /// When the parent itself is being torn down we skip the per-child DOM
    /// removal — the parent's subtree is dropped wholesale by the caller.
    parent_was_removed: bool,
}

impl<'a, 'b, 'c> GenetChildrenSplice<'a, 'b, 'c> {
    /// Create a splice over `parent`'s children.
    pub fn new(
        scratch: &'a mut AppendVec<GenetElement>,
        children: &'b mut Vec<GenetElement>,
        vec_splice_scratch: &'c mut Vec<GenetElement>,
        parent: NodeId,
        dom: crate::DomHandle,
        parent_was_removed: bool,
    ) -> Self {
        Self {
            scratch,
            children: VecSplice::new(children, vec_splice_scratch),
            ix: 0,
            parent,
            dom,
            parent_was_removed,
        }
    }

    /// The DOM node of the next pending retained child, used as the
    /// `insert_before` reference (append if there is none).
    fn next_sibling(&mut self) -> Option<NodeId> {
        self.children.next_pending().map(|p| p.node)
    }
}

impl ElementSplice<GenetElement> for GenetChildrenSplice<'_, '_, '_> {
    fn with_scratch<R>(&mut self, f: impl FnOnce(&mut AppendVec<GenetElement>) -> R) -> R {
        let ret = f(self.scratch);
        if !self.scratch.is_empty() {
            // serval has no DocumentFragment, so insert each built child at the
            // current cursor in order. The reference node is recomputed each
            // time because previously inserted children become preceding
            // siblings, not the reference.
            let drained: Vec<GenetElement> = self.scratch.drain().collect();
            for element in drained {
                let reference = self.next_sibling();
                self.dom
                    .borrow_mut()
                    .insert_before(self.parent, element.node, reference);
                self.ix += 1;
                self.children.insert(element);
            }
        }
        ret
    }

    fn insert(&mut self, element: GenetElement) {
        let reference = self.next_sibling();
        self.dom
            .borrow_mut()
            .insert_before(self.parent, element.node, reference);
        self.ix += 1;
        self.children.insert(element);
    }

    fn mutate<R>(&mut self, f: impl FnOnce(Mut<'_, GenetElement>) -> R) -> R {
        let dom = self.dom.clone();
        let parent = self.parent;
        let child = self.children.mutate();
        let ret = f(GenetElementMut {
            node: &mut child.node,
            dom,
            parent: Some(parent),
        });
        self.ix += 1;
        ret
    }

    fn skip(&mut self, n: usize) {
        self.children.skip(n);
        self.ix += n;
    }

    fn index(&self) -> usize {
        self.ix
    }

    fn delete<R>(&mut self, f: impl FnOnce(Mut<'_, GenetElement>) -> R) -> R {
        let dom = self.dom.clone();
        let parent = self.parent;
        let mut child = self.children.delete_next();
        let node = child.node;
        let ret = f(GenetElementMut {
            node: &mut child.node,
            dom,
            parent: Some(parent),
        });
        // Eagerly remove from the DOM unless the whole parent subtree is being
        // dropped (an up-traversal would otherwise remove it redundantly).
        if !self.parent_was_removed {
            self.dom.borrow_mut().remove(node);
        }
        ret
    }

    fn hoist_pending(&mut self, n: usize) -> bool {
        if n == 0 {
            return true; // already the next pending element; nothing moves
        }
        // The current front of the pending queue is what the hoisted node
        // moves before — read it before the reorder.
        let Some(reference) = self.next_sibling() else {
            return false;
        };
        let Some(node) = self.children.pending_at(n).map(|e| e.node) else {
            return false;
        };
        if !self.children.hoist_pending(n) {
            return false;
        }
        // One atomic move (`DomMutation::Moved`), never a remove + insert —
        // the node stays attached, so retained per-node state survives.
        // (moveBefore plan S5.)
        self.dom
            .borrow_mut()
            .move_before(self.parent, node, Some(reference));
        true
    }

    fn extract_pending(&mut self) -> Option<GenetElement> {
        // The element leaves this splice's bookkeeping, but its DOM node stays
        // attached where it is: parking is not removal. The node moves when the
        // element is adopted elsewhere (`adopt_pending` → one `Moved`), or is
        // removed for real when the nursery drain tears the orphan down.
        // (moveBefore plan S5, cross-parent.)
        self.children.take_next_pending()
    }

    fn adopt_pending(&mut self, element: GenetElement) -> Result<(), GenetElement> {
        // Move the foreign element's node into place first (before the current
        // front pending, append when none), then queue it as next pending so
        // the caller's ordinary mutate-based rebuild consumes it. The node was
        // still attached under its former parent (extract does not detach), so
        // this is one atomic `Moved` — the never-detach contract.
        let reference = self.next_sibling();
        self.dom
            .borrow_mut()
            .move_before(self.parent, element.node, reference);
        self.children.push_next_pending(element);
        Ok(())
    }
}
