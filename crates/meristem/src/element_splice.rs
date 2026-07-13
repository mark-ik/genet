// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use alloc::vec::{Drain, Vec};

use crate::ViewElement;

/// A temporary "splice" to add, update and delete in an (ordered) sequence of elements.
/// It is mainly intended for view sequences.
pub trait ElementSplice<Element: ViewElement> {
    /// Run a function with access to the associated [`AppendVec`].
    ///
    /// Each element [pushed](AppendVec::push) to the provided vector will be logically
    /// [inserted](ElementSplice::insert) into `self`.
    fn with_scratch<R>(&mut self, f: impl FnOnce(&mut AppendVec<Element>) -> R) -> R;
    /// Insert a new element at the current index in the resulting collection.
    fn insert(&mut self, element: Element);
    /// Mutate the next existing element.
    fn mutate<R>(&mut self, f: impl FnOnce(Element::Mut<'_>) -> R) -> R;
    /// Don't make any changes to the next n existing elements.
    fn skip(&mut self, n: usize);
    /// How many elements you would need to [`skip`](ElementSplice::skip) from when this
    /// `ElementSplice` was created to get to the current element.
    ///
    /// Note that in using this function, previous views will have skipped.
    /// Values obtained from this method may change during any `rebuild`, but will not change
    /// between `build`/`rebuild` and the next `message`
    fn index(&self) -> usize;
    /// Delete the next existing element, after running a function on it.
    fn delete<R>(&mut self, f: impl FnOnce(Element::Mut<'_>) -> R) -> R;
    /// Move the pending (not yet processed) element at relative offset `n`
    /// (`0` = the next pending element) to the front of the pending queue,
    /// repositioning its backing node so the underlying store observes one
    /// atomic move rather than a delete + re-insert; the displaced elements
    /// keep their relative order. A splice that cannot move elements returns
    /// `false` (the default), and the caller falls back to tearing down and
    /// rebuilding the affected children.
    fn hoist_pending(&mut self, n: usize) -> bool {
        let _ = n;
        false
    }
    /// Take the next pending element out of this splice **without** destroying
    /// its backing node — the node stays where it is in the underlying store
    /// until whoever parked the element either adopts it elsewhere
    /// ([`adopt_pending`](Self::adopt_pending)) or tears it down for real.
    /// `None` (the default) when this splice cannot extract; the caller falls
    /// down to an ordinary teardown.
    fn extract_pending(&mut self) -> Option<Element> {
        None
    }
    /// Adopt a foreign element — one extracted from another splice — as this
    /// splice's next pending element, moving its backing node into place
    /// atomically (one move, never a delete + re-insert). The caller then
    /// consumes it with the ordinary [`mutate`](Self::mutate)-based rebuild.
    /// `Err(element)` (the default) hands the element back when this splice
    /// cannot adopt; the caller falls back to building the child fresh.
    fn adopt_pending(&mut self, element: Element) -> Result<(), Element> {
        Err(element)
    }
}

/// An append only `Vec`.
///
/// This will be passed to [`ViewSequence::seq_build`](crate::ViewSequence::seq_build) to
/// build the list of initial elements whilst materializing the sequence.
#[derive(Debug)]
pub struct AppendVec<T> {
    inner: Vec<T>,
}

impl<T> AppendVec<T> {
    /// Convert `self` into the underlying `Vec`
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.inner
    }
    /// Add an item to the end of the vector.
    pub fn push(&mut self, item: T) {
        self.inner.push(item);
    }
    /// [Drain](Vec::drain) all items from this `AppendVec`.
    pub fn drain(&mut self) -> Drain<'_, T> {
        self.inner.drain(..)
    }
    /// Equivalent to [`ElementSplice::index`].
    pub fn index(&self) -> usize {
        // If there are no items, to get here we need to skip 0
        // if there is one, we need to skip 1
        self.inner.len()
    }
    /// Returns `true` if the vector contains no elements.
    ///
    /// See [`Vec::is_empty`] for more details
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T> From<Vec<T>> for AppendVec<T> {
    fn from(inner: Vec<T>) -> Self {
        Self { inner }
    }
}

impl<T> Default for AppendVec<T> {
    fn default() -> Self {
        Self {
            inner: Vec::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppendVec, ElementSplice};
    use crate::ViewElement;

    #[derive(Debug, PartialEq)]
    struct TestElement(u8);

    impl ViewElement for TestElement {
        type Mut<'a> = &'a mut Self;
    }

    struct MinimalSplice;

    impl ElementSplice<TestElement> for MinimalSplice {
        fn with_scratch<R>(&mut self, f: impl FnOnce(&mut AppendVec<TestElement>) -> R) -> R {
            f(&mut AppendVec::default())
        }

        fn insert(&mut self, _element: TestElement) {}

        fn mutate<R>(&mut self, _f: impl FnOnce(&mut TestElement) -> R) -> R {
            unreachable!()
        }

        fn skip(&mut self, _n: usize) {}

        fn index(&self) -> usize {
            0
        }

        fn delete<R>(&mut self, _f: impl FnOnce(&mut TestElement) -> R) -> R {
            unreachable!()
        }
    }

    #[test]
    fn move_extensions_default_to_unsupported_without_consuming_elements() {
        let mut splice = MinimalSplice;

        assert!(!splice.hoist_pending(1));
        assert_eq!(splice.extract_pending(), None);
        assert_eq!(splice.adopt_pending(TestElement(7)), Err(TestElement(7)));
    }
}
