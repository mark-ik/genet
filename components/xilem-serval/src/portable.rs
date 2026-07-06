/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`PortableKeyed`]: a keyed sequence whose children survive **cross-parent
//! moves** (moveBefore plan S5, the tear-out case).
//!
//! Where [`Keyed`](crate::Keyed) preserves children across membership changes
//! and (for single-element children) same-parent reorders, `PortableKeyed`
//! additionally lets a child leave one sequence and arrive in another within
//! one rebuild pass, keeping its element, view state, and DOM node:
//!
//! - A key that **leaves** the list parks its child in the
//!   [`ServalCtx`] nursery ([`park_portable`](crate::ServalCtx::park_portable))
//!   instead of tearing it down. The DOM node stays attached under the old
//!   parent for the moment.
//! - A **new** key first checks the nursery: on a hit, the parked element is
//!   adopted ([`adopt_pending`](xilem_core::ElementSplice::adopt_pending) — one
//!   atomic `Moved`, the node never detaches) and the child **rebuilds** from
//!   its parked view + state under the new position, which also re-registers
//!   its event handlers at the new routing path.
//! - Unclaimed parked children are torn down for real by the runner's
//!   end-of-rebuild [`drain_nursery`](crate::ServalCtx::drain_nursery).
//!
//! **Ordering caveat**: preservation requires the source sequence to rebuild
//! before the target within the pass (view tree order). Target-before-source
//! degrades safely to fresh-build + parked-teardown — correct, no leak, no
//! preservation. The multi-projection runner (one-state-N-windows design,
//! step 2) makes this host-controllable by rebuilding the source window first.
//!
//! Children are single `View`s (`Count::One` by construction), which is the
//! tile/card shape tear-out needs, and must be `Clone` (the parked previous
//! view is what the adopting rebuild diffs against, and what a drain teardown
//! runs with).

use std::hash::Hash;

use layout_dom_api::{LayoutDom, LayoutDomMut};
use xilem_core::{
    AppendVec, Count, ElementSplice, MessageCtx, MessageResult, View, ViewMarker, ViewPathTracker,
    ViewSequence,
};

use crate::keyed::{
    assert_unique_keys, bump_generation, create_generational_view_id, view_id_to_slot_generation,
};
use crate::{ServalCtx, ServalElement};

/// An ordered keyed sequence of portable single-`View` children. See the
/// module docs for the movement contract.
#[derive(Debug, Default)]
pub struct PortableKeyed<K, V> {
    items: Vec<(K, V)>,
}

impl<K, V> PortableKeyed<K, V> {
    pub fn new(items: Vec<(K, V)>) -> Self {
        Self { items }
    }
}

impl<K, V> From<Vec<(K, V)>> for PortableKeyed<K, V> {
    fn from(items: Vec<(K, V)>) -> Self {
        Self::new(items)
    }
}

impl<K, V> FromIterator<(K, V)> for PortableKeyed<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

#[derive(Debug)]
struct PortableEntry<K, InnerState> {
    key: K,
    slot: usize,
    inner_state: InnerState,
}

#[derive(Debug, Default)]
pub struct PortableKeyedState<K, InnerState> {
    entries: Vec<PortableEntry<K, InnerState>>,
    generations: Vec<u32>,
    free_slots: Vec<usize>,
}

fn alloc_slot<K, S>(state: &mut PortableKeyedState<K, S>) -> usize {
    state.free_slots.pop().unwrap_or_else(|| {
        let slot = state.generations.len();
        state.generations.push(0);
        slot
    })
}

impl<State, Action, K, V> ViewSequence<State, Action, ServalCtx, ServalElement>
    for PortableKeyed<K, V>
where
    State: 'static,
    Action: 'static,
    K: Clone + Eq + Hash + 'static,
    V: View<State, Action, ServalCtx, Element = ServalElement> + ViewMarker + Clone + 'static,
    V::ViewState: 'static,
{
    type SeqState = PortableKeyedState<K, V::ViewState>;

    const ELEMENTS_COUNT: Count = Count::Many;

    fn seq_build(
        &self,
        ctx: &mut ServalCtx,
        elements: &mut AppendVec<ServalElement>,
        app_state: &mut State,
    ) -> Self::SeqState {
        assert_unique_keys(&self.items);
        let mut state = PortableKeyedState {
            entries: Vec::with_capacity(self.items.len()),
            generations: Vec::with_capacity(self.items.len()),
            free_slots: Vec::new(),
        };
        for (key, view) in &self.items {
            let slot = alloc_slot(&mut state);
            let generation = state.generations[slot];
            let inner_state = ctx.with_id(create_generational_view_id(slot, generation), |ctx| {
                // A nursery hit at build time is possible when a whole pane is
                // rebuilt around a surviving portable child; adopt is not
                // available on an AppendVec, so build claims degrade to fresh
                // builds via the ordinary path below.
                let (element, view_state) = view.build(ctx, app_state);
                elements.push(element);
                view_state
            });
            state.entries.push(PortableEntry {
                key: key.clone(),
                slot,
                inner_state,
            });
        }
        state
    }

    fn seq_rebuild(
        &self,
        prev: &Self,
        seq_state: &mut Self::SeqState,
        ctx: &mut ServalCtx,
        elements: &mut impl ElementSplice<ServalElement>,
        app_state: &mut State,
    ) {
        assert_unique_keys(&self.items);
        let mut old_entries: Vec<Option<PortableEntry<K, V::ViewState>>> =
            std::mem::take(&mut seq_state.entries)
                .into_iter()
                .map(Some)
                .collect();
        let mut remaining_old = std::collections::HashMap::with_capacity(old_entries.len());
        for (index, entry) in old_entries.iter().enumerate() {
            let key = &entry
                .as_ref()
                .expect("old portable entry present before rebuild")
                .key;
            remaining_old.insert(key.clone(), index);
        }
        let new_keys: std::collections::HashSet<&K> =
            self.items.iter().map(|(key, _)| key).collect();
        let mut new_entries: Vec<PortableEntry<K, V::ViewState>> =
            Vec::with_capacity(self.items.len());
        let mut old_cursor = 0usize;

        for (key, child) in &self.items {
            // 1. Skip out-of-order-consumed entries; park leading departures
            //    (or tear them down when the splice cannot extract). A parked
            //    child's view state and element stay alive, its DOM node stays
            //    attached, ready for adoption elsewhere in this same pass.
            //    (moveBefore S5, cross-parent.)
            loop {
                match old_entries.get(old_cursor).map(Option::as_ref) {
                    Some(None) => old_cursor += 1,
                    Some(Some(entry)) if !new_keys.contains(&entry.key) => {
                        let mut removed = old_entries[old_cursor]
                            .take()
                            .expect("departing portable entry present");
                        remaining_old.remove(&removed.key);
                        let generation = seq_state.generations[removed.slot];
                        match elements.extract_pending() {
                            Some(element) => {
                                ctx.park_portable::<State, Action, K, V>(
                                    removed.key.clone(),
                                    prev.items[old_cursor].1.clone(),
                                    removed.inner_state,
                                    element,
                                );
                            },
                            None => {
                                ctx.with_id(
                                    create_generational_view_id(removed.slot, generation),
                                    |ctx| {
                                        prev.items[old_cursor].1.seq_teardown(
                                            &mut removed.inner_state,
                                            ctx,
                                            elements,
                                        );
                                    },
                                );
                            },
                        }
                        bump_generation(&mut seq_state.generations, removed.slot);
                        seq_state.free_slots.push(removed.slot);
                        old_cursor += 1;
                    },
                    _ => break,
                }
            }

            // 2. Direct match: rebuild in place.
            let direct = old_entries
                .get(old_cursor)
                .and_then(Option::as_ref)
                .is_some_and(|entry| entry.key == *key);
            if direct {
                let mut entry = old_entries[old_cursor]
                    .take()
                    .expect("direct portable match present");
                remaining_old.remove(key);
                let generation = seq_state.generations[entry.slot];
                ctx.with_id(create_generational_view_id(entry.slot, generation), |ctx| {
                    child.seq_rebuild(
                        &prev.items[old_cursor].1,
                        &mut entry.inner_state,
                        ctx,
                        elements,
                        app_state,
                    );
                });
                new_entries.push(entry);
                old_cursor += 1;
                continue;
            }

            // 3. Later match in this sequence: hoist (same-parent move).
            if let Some(&match_index) = remaining_old.get(key) {
                let offset = old_entries[old_cursor..match_index]
                    .iter()
                    .filter(|entry| entry.is_some())
                    .count();
                if elements.hoist_pending(offset) {
                    let mut entry = old_entries[match_index]
                        .take()
                        .expect("hoisted portable entry present");
                    remaining_old.remove(key);
                    let generation = seq_state.generations[entry.slot];
                    ctx.with_id(create_generational_view_id(entry.slot, generation), |ctx| {
                        child.seq_rebuild(
                            &prev.items[match_index].1,
                            &mut entry.inner_state,
                            ctx,
                            elements,
                            app_state,
                        );
                    });
                    new_entries.push(entry);
                    continue;
                }
                // Non-hoisting splice: Keyed's old contract — tear down the
                // intervening entries, then consume the match directly.
                while old_cursor < match_index {
                    let Some(mut removed) = old_entries[old_cursor].take() else {
                        old_cursor += 1;
                        continue;
                    };
                    remaining_old.remove(&removed.key);
                    let generation = seq_state.generations[removed.slot];
                    ctx.with_id(
                        create_generational_view_id(removed.slot, generation),
                        |ctx| {
                            prev.items[old_cursor].1.seq_teardown(
                                &mut removed.inner_state,
                                ctx,
                                elements,
                            );
                        },
                    );
                    bump_generation(&mut seq_state.generations, removed.slot);
                    seq_state.free_slots.push(removed.slot);
                    old_cursor += 1;
                }
                let mut entry = old_entries[old_cursor]
                    .take()
                    .expect("matched portable entry present after deletions");
                remaining_old.remove(key);
                let generation = seq_state.generations[entry.slot];
                ctx.with_id(create_generational_view_id(entry.slot, generation), |ctx| {
                    child.seq_rebuild(
                        &prev.items[old_cursor].1,
                        &mut entry.inner_state,
                        ctx,
                        elements,
                        app_state,
                    );
                });
                new_entries.push(entry);
                old_cursor += 1;
                continue;
            }

            // 4. Cross-sequence arrival: claim from the nursery and adopt.
            if let Some((parked_view, parked_state, parked_element)) =
                ctx.claim_portable::<State, Action, K, V>(key)
            {
                match elements.adopt_pending(parked_element) {
                    Ok(()) => {
                        let slot = alloc_slot(seq_state);
                        let generation = seq_state.generations[slot];
                        let mut inner_state = parked_state;
                        ctx.with_id(create_generational_view_id(slot, generation), |ctx| {
                            // The adopted element is the next pending one; the
                            // ordinary mutate-based rebuild consumes it, and
                            // the (node, path) handler reconciliation inside
                            // re-registers its handlers at this new position.
                            child.seq_rebuild(
                                &parked_view,
                                &mut inner_state,
                                ctx,
                                elements,
                                app_state,
                            );
                        });
                        new_entries.push(PortableEntry {
                            key: key.clone(),
                            slot,
                            inner_state,
                        });
                        continue;
                    },
                    Err(mut element) => {
                        // Non-adopting splice: the parked child cannot cross;
                        // tear it down for real (its node is still attached
                        // under the old parent) and build fresh below.
                        let mut state = parked_state;
                        let dom = ctx.dom();
                        let node = element.node;
                        let parent = dom.borrow().parent(node);
                        let el = crate::ServalElementMut {
                            node: &mut element.node,
                            dom: dom.clone(),
                            parent,
                        };
                        parked_view.teardown(&mut state, ctx, el);
                        dom.borrow_mut().remove(node);
                    },
                }
            }

            // 5. Fresh build.
            let slot = alloc_slot(seq_state);
            let generation = seq_state.generations[slot];
            let mut built_state = None;
            elements.with_scratch(|scratch| {
                let inner_state =
                    ctx.with_id(create_generational_view_id(slot, generation), |ctx| {
                        let (element, view_state) = child.build(ctx, app_state);
                        scratch.push(element);
                        view_state
                    });
                built_state = Some(PortableEntry {
                    key: key.clone(),
                    slot,
                    inner_state,
                });
            });
            new_entries.push(built_state.expect("portable insert built state"));
        }

        // Leftovers: every remaining unconsumed entry departed this list —
        // park each (or tear down on a non-extracting splice), same as the
        // leading-departure path above.
        while old_cursor < old_entries.len() {
            let Some(mut removed) = old_entries[old_cursor].take() else {
                old_cursor += 1;
                continue;
            };
            remaining_old.remove(&removed.key);
            let generation = seq_state.generations[removed.slot];
            match elements.extract_pending() {
                Some(element) => {
                    ctx.park_portable::<State, Action, K, V>(
                        removed.key.clone(),
                        prev.items[old_cursor].1.clone(),
                        removed.inner_state,
                        element,
                    );
                },
                None => {
                    ctx.with_id(
                        create_generational_view_id(removed.slot, generation),
                        |ctx| {
                            prev.items[old_cursor].1.seq_teardown(
                                &mut removed.inner_state,
                                ctx,
                                elements,
                            );
                        },
                    );
                },
            }
            bump_generation(&mut seq_state.generations, removed.slot);
            seq_state.free_slots.push(removed.slot);
            old_cursor += 1;
        }

        seq_state.entries = new_entries;
    }

    fn seq_teardown(
        &self,
        seq_state: &mut Self::SeqState,
        ctx: &mut ServalCtx,
        elements: &mut impl ElementSplice<ServalElement>,
    ) {
        // Parent-driven teardown is real teardown, never parking: the whole
        // sequence is going away.
        for ((_, view), entry) in self.items.iter().zip(&mut seq_state.entries) {
            let generation = seq_state.generations[entry.slot];
            ctx.with_id(create_generational_view_id(entry.slot, generation), |ctx| {
                view.seq_teardown(&mut entry.inner_state, ctx, elements);
            });
        }
    }

    fn seq_message(
        &self,
        seq_state: &mut Self::SeqState,
        message: &mut MessageCtx,
        elements: &mut impl ElementSplice<ServalElement>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let start = message
            .take_first()
            .expect("Id path has elements for PortableKeyed");
        let (slot, generation) = view_id_to_slot_generation(start);
        let Some(stored_generation) = seq_state.generations.get(slot) else {
            return MessageResult::Stale;
        };
        if *stored_generation != generation {
            return MessageResult::Stale;
        }
        let Some((index, entry)) = seq_state
            .entries
            .iter_mut()
            .enumerate()
            .find(|(_, entry)| entry.slot == slot)
        else {
            return MessageResult::Stale;
        };
        // Single-element children: the entry's element offset is its index.
        elements.skip(index);
        self.items[index]
            .1
            .seq_message(&mut entry.inner_state, message, elements, app_state)
    }
}
