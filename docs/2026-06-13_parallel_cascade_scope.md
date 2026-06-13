# Parallel cascade — scope and deferral

Status: **deferred, held for a Fedora + ThreadSanitizer session** (decision
2026-06-13). Do not flip this on from the Windows box. Reftests cannot catch a
data race; verification requires a sanitizer, which runs for Rust on the Fedora
44 workstation, not on Windows.

This captures the analysis so the Fedora session executes without re-deriving it.

## Why (and why bounded)

Stylo is a parallel-by-design cascade engine; serval currently drives it
single-threaded. Parallel cascade speeds up style resolution on large content
pages. The payoff is bounded: a size threshold keeps small DOMs (chrome UI)
serial, and inline-text shaping — the larger text-side win — is already
parallelized and verified (`box_tree::shape_inline_leaves`, commit on
2026-06-13). So this is a "large pages only" optimization, not a hot-path
necessity.

## What's already easy

- `style::driver::traverse_dom(&traversal, token, pool)` takes the rayon pool as
  its third arg. Today serval passes `None` (`cascade.rs`, the
  `RecalcStyle::new(context)` / `traverse_dom(..., None)` call). Passing
  `Some(&pool)` is the switch.
- The per-worker TLS problem (`CASCADE_CTX` in `adapter_stylo.rs` is set only on
  the calling thread by `CascadeGuard::enter`; worker threads would see `None`
  and panic in `cascade_ctx()`) is solvable with `pool.broadcast(|_| install
  the same CascadeCtx)` before the traversal and a matching clear after. The
  ctx is Copy pointers to shared immutable data (dom / plane / lock / snapshot),
  valid for the call's duration. `traverse_dom` uses `in_place_scope_fifo`, so
  the calling thread participates too — its existing main-thread `CascadeGuard`
  already covers it.

## The real blocker: data races on `StyleEntry` cells

Stylo's parallel traversal touches per-element state across threads. serval used
`Cell` / `UnsafeCell` as a single-threaded shortcut where Servo's real DOM uses
atomics. The contended fields (`style.rs`):

- `selector_flags: Cell<ElementSelectorFlags>` — **confirmed race.**
  `adapter_stylo.rs` `apply_selector_flags` propagates a child's flags up to its
  *parent's* cell:
  `p_entry.selector_flags.set(p_entry.selector_flags.get() | parent_flags)`.
  Sibling children on different threads read-modify-write the same parent cell
  concurrently. This is the mechanism behind the prior "parallel-only heap
  corruption" noted in `cascade.rs`.
- `dirty_descendants: Cell<bool>` — parent/child propagation; contended.
- `handled_snapshot: Cell<bool>` — incremental path; "Stylo processes a child's
  snapshot while traversing the parent," so a parent worker writes a child's
  bit. Contended in the snapshot path.
- `stylo_data: UnsafeCell<Option<ElementDataWrapper>>` — single-writer-per-element
  under Stylo's traversal discipline. Likely sound, but the `mutate_data` /
  `borrow_data` access must be confirmed race-free under parallel traversal, not
  assumed.

## Path (Fedora session)

1. Convert `selector_flags`, `dirty_descendants`, `handled_snapshot` from `Cell`
   to atomics (e.g. `AtomicU16` over the bits, `fetch_or` for the OR-propagation;
   `AtomicBool` for the flags). Update every reader from `.get()` / `.set()`.
   This step is behavior-preserving in serial and independently verifiable on
   Windows (full suite + a reftest subset hold), so it can land first if wanted.
2. Audit the `UnsafeCell<stylo_data>` single-writer guarantee under parallel
   traversal.
3. `pool.broadcast` the `CascadeCtx` to the pool workers around the parallel
   `traverse_dom`; clear after. Build or borrow a rayon pool.
4. Pass `Some(&pool)` to `traverse_dom`; gate on a DOM-size threshold (small
   trees stay serial, mirroring the shaping pre-pass's `PARALLEL_SHAPE_THRESHOLD`).

## Done conditions

- Cascade output (computed styles) identical serial vs parallel — covered by the
  existing reftests staying put.
- A ThreadSanitizer run on Fedora 44 over a large-DOM cascade reports no data
  race. This is the gate; without it, the change does not land.
- A size threshold keeps chrome-scale DOMs on the serial path.

## Related

- Shape/break split + parallel shaping pre-pass: the text-side half of the
  threading arc, already landed and verified (`text_measure::shape_leaf`,
  `box_tree::shape_inline_leaves`).
- `D::NodeId: Send + Sync` is already threaded through
  `layout` / `render` / `incremental` / `subtree` for the shaping pre-pass; the
  cascade flip reuses it.
