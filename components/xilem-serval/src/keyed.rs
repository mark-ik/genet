/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use xilem_core::{
    AppendVec, Count, ElementSplice, MessageCtx, MessageResult, ViewElement, ViewId,
    ViewPathTracker, ViewSequence,
};

/// An ordered keyed child sequence.
///
/// Membership changes preserve retained child state when the surviving keys stay
/// in the same relative order. For **single-element children**
/// (`Seq::ELEMENTS_COUNT == Count::One`) over a splice that supports
/// [`hoist_pending`](ElementSplice::hoist_pending), a reorder is a real move:
/// the surviving child keeps its element and view state, and the DOM observes
/// one atomic [`Moved`](layout_dom_api::DomMutation::Moved), never a remove +
/// re-insert (moveBefore plan S5). Multi-element children and non-hoisting
/// splices keep the old contract: a true reorder degrades to teardown + build
/// for the moved children.
#[derive(Debug, Default)]
pub struct Keyed<K, V> {
    items: Vec<(K, V)>,
}

#[derive(Debug)]
struct KeyedEntryState<K, InnerState> {
    key: K,
    slot: usize,
    child_skip: usize,
    inner_state: InnerState,
}

#[derive(Debug, Default)]
pub struct KeyedViewState<K, InnerState> {
    entries: Vec<KeyedEntryState<K, InnerState>>,
    generations: Vec<u32>,
    free_slots: Vec<usize>,
}

impl<K, V> Keyed<K, V> {
    pub fn new(items: Vec<(K, V)>) -> Self {
        Self { items }
    }
}

impl<K, V> From<Vec<(K, V)>> for Keyed<K, V> {
    fn from(items: Vec<(K, V)>) -> Self {
        Self::new(items)
    }
}

impl<K, V> FromIterator<(K, V)> for Keyed<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

pub(crate) fn create_generational_view_id(slot: usize, generation: u32) -> ViewId {
    let slot_low: u32 = slot
        .try_into()
        .expect("Views in a keyed sequence must be indexable by u32");
    let slot_low: u64 = slot_low.into();
    let generation_high: u64 = u64::from(generation) << 32;
    ViewId::new(generation_high | slot_low)
}

pub(crate) fn view_id_to_slot_generation(view_id: ViewId) -> (usize, u32) {
    let view_id = view_id.routing_id();
    let slot_low = view_id as u32;
    let generation_high = (view_id >> 32) as u32;
    (slot_low as usize, generation_high)
}

pub(crate) fn bump_generation(generations: &mut [u32], slot: usize) {
    generations[slot] = generations[slot].checked_add(1).unwrap_or(0);
}

fn alloc_slot<K, InnerState>(state: &mut KeyedViewState<K, InnerState>) -> usize {
    state.free_slots.pop().unwrap_or_else(|| {
        let slot = state.generations.len();
        state.generations.push(0);
        slot
    })
}

pub(crate) fn assert_unique_keys<K, V>(items: &[(K, V)])
where
    K: Eq + Hash,
{
    let mut seen = HashMap::with_capacity(items.len());
    for (index, (key, _)) in items.iter().enumerate() {
        if seen.insert(key, index).is_some() {
            panic!("duplicate key in Keyed<ViewSequence>");
        }
    }
}

impl<State, Action, Context, Element, K, Seq> ViewSequence<State, Action, Context, Element>
    for Keyed<K, Seq>
where
    State: 'static,
    Context: ViewPathTracker,
    Element: ViewElement,
    K: Clone + Eq + Hash + 'static,
    Seq: ViewSequence<State, Action, Context, Element>,
{
    type SeqState = KeyedViewState<K, Seq::SeqState>;

    const ELEMENTS_COUNT: Count = Seq::ELEMENTS_COUNT.multiple();

    fn seq_build(
        &self,
        ctx: &mut Context,
        elements: &mut AppendVec<Element>,
        app_state: &mut State,
    ) -> Self::SeqState {
        assert_unique_keys(&self.items);
        let start_idx = elements.index();
        let mut state = KeyedViewState {
            entries: Vec::with_capacity(self.items.len()),
            generations: Vec::with_capacity(self.items.len()),
            free_slots: Vec::new(),
        };
        for (key, seq) in &self.items {
            let slot = alloc_slot(&mut state);
            let child_skip = elements.index() - start_idx;
            let generation = state.generations[slot];
            let inner_state = ctx.with_id(create_generational_view_id(slot, generation), |ctx| {
                seq.seq_build(ctx, elements, app_state)
            });
            state.entries.push(KeyedEntryState {
                key: key.clone(),
                slot,
                child_skip,
                inner_state,
            });
        }
        state
    }

    fn seq_rebuild(
        &self,
        prev: &Self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) {
        assert_unique_keys(&self.items);
        let start_idx = elements.index();
        let mut old_entries: Vec<Option<KeyedEntryState<K, Seq::SeqState>>> =
            std::mem::take(&mut seq_state.entries)
                .into_iter()
                .map(Some)
                .collect();
        let mut remaining_old = HashMap::with_capacity(old_entries.len());
        for (index, entry) in old_entries.iter().enumerate() {
            let key = &entry
                .as_ref()
                .expect("old keyed entry present before rebuild")
                .key;
            remaining_old.insert(key.clone(), index);
        }
        // The surviving key set, for the single-element move path: a leading
        // old entry whose key left the list is torn down eagerly (it must die
        // anyway, and deleting it first keeps a hoist from moving a survivor
        // over soon-to-die siblings). (moveBefore plan S5.)
        let hoistable = Seq::ELEMENTS_COUNT == Count::One;
        let new_keys: HashSet<&K> = if hoistable {
            self.items.iter().map(|(key, _)| key).collect()
        } else {
            HashSet::new()
        };
        let mut new_entries = Vec::with_capacity(self.items.len());
        let mut old_cursor = 0usize;

        for (key, child) in &self.items {
            let child_skip = elements.index() - start_idx;
            if hoistable {
                // Advance past entries a hoist consumed out of order, and tear
                // down a prefix of removals (keys absent from the new list).
                loop {
                    match old_entries.get(old_cursor).map(Option::as_ref) {
                        Some(None) => old_cursor += 1,
                        Some(Some(entry)) if !new_keys.contains(&entry.key) => {
                            let mut removed = old_entries[old_cursor]
                                .take()
                                .expect("leading removed keyed entry present");
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
                        },
                        _ => break,
                    }
                }
            }
            let direct_match = old_entries
                .get(old_cursor)
                .and_then(Option::as_ref)
                .is_some_and(|entry| entry.key == *key);
            if direct_match {
                let mut entry = old_entries[old_cursor]
                    .take()
                    .expect("direct keyed match entry present");
                remaining_old.remove(key);
                let generation = seq_state.generations[entry.slot];
                entry.child_skip = child_skip;
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

            if let Some(&match_index) = remaining_old.get(key) {
                if hoistable {
                    // Every unconsumed entry between the cursor and the match
                    // is a single-element child, so the match's pending offset
                    // is a plain count. Hoist it to the cursor (one atomic
                    // move — element and view state survive) instead of
                    // tearing the intervening entries down; they stay pending
                    // and are consumed by later iterations. (moveBefore S5.)
                    let offset = old_entries[old_cursor..match_index]
                        .iter()
                        .filter(|entry| entry.is_some())
                        .count();
                    if elements.hoist_pending(offset) {
                        let mut entry = old_entries[match_index]
                            .take()
                            .expect("hoisted keyed entry present");
                        remaining_old.remove(key);
                        let generation = seq_state.generations[entry.slot];
                        entry.child_skip = child_skip;
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
                        // The cursor stays: the intervening survivors are
                        // still pending in their old order.
                        continue;
                    }
                }
                while old_cursor < match_index {
                    // Out-of-order consumption (a prior hoist) can leave holes.
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
                    .expect("matched keyed entry present after deletions");
                remaining_old.remove(key);
                let generation = seq_state.generations[entry.slot];
                entry.child_skip = child_skip;
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

            let slot = alloc_slot(seq_state);
            let generation = seq_state.generations[slot];
            let outer_idx = elements.index();
            let mut built_state = None;
            elements.with_scratch(|scratch| {
                let this_skip = scratch.index() + outer_idx - start_idx;
                let inner_state = ctx
                    .with_id(create_generational_view_id(slot, generation), |ctx| {
                        child.seq_build(ctx, scratch, app_state)
                    });
                built_state = Some(KeyedEntryState {
                    key: key.clone(),
                    slot,
                    child_skip: this_skip,
                    inner_state,
                });
            });
            new_entries.push(built_state.expect("keyed insert built state"));
        }

        while old_cursor < old_entries.len() {
            // Out-of-order consumption (a hoist) can leave holes here too.
            let Some(mut removed) = old_entries[old_cursor].take() else {
                old_cursor += 1;
                continue;
            };
            remaining_old.remove(&removed.key);
            let generation = seq_state.generations[removed.slot];
            ctx.with_id(
                create_generational_view_id(removed.slot, generation),
                |ctx| {
                    prev.items[old_cursor]
                        .1
                        .seq_teardown(&mut removed.inner_state, ctx, elements);
                },
            );
            bump_generation(&mut seq_state.generations, removed.slot);
            seq_state.free_slots.push(removed.slot);
            old_cursor += 1;
        }

        seq_state.entries = new_entries;
    }

    fn seq_teardown(
        &self,
        seq_state: &mut Self::SeqState,
        ctx: &mut Context,
        elements: &mut impl ElementSplice<Element>,
    ) {
        for ((_, seq), entry) in self.items.iter().zip(&mut seq_state.entries) {
            let generation = seq_state.generations[entry.slot];
            ctx.with_id(create_generational_view_id(entry.slot, generation), |ctx| {
                seq.seq_teardown(&mut entry.inner_state, ctx, elements);
            });
        }
    }

    fn seq_message(
        &self,
        seq_state: &mut Self::SeqState,
        message: &mut MessageCtx,
        elements: &mut impl ElementSplice<Element>,
        app_state: &mut State,
    ) -> MessageResult<Action> {
        let start = message
            .take_first()
            .expect("Id path has elements for Keyed<ViewSequence>");
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
        elements.skip(entry.child_skip);
        self.items[index]
            .1
            .seq_message(&mut entry.inner_state, message, elements, app_state)
    }
}
