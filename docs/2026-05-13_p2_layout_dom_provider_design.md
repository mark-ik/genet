# P2 Layout DOM Provider Design

**Status (2026-05-16):** historical. Describes the layout-provider seam inside
`components/layout/`, which became dead-on-disk in the 2026-05-15 audit. The
profile-neutral DOM trait now lives in a new `layout-dom-api` crate per path C
of the lift plan. Active docs:

- [2026-05-16_serval_layout_lift_plan.md](./2026-05-16_serval_layout_lift_plan.md) — path C implementation plan.
- [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md) — `LayoutDom` trait location and shape.

Below is preserved as the prior-art record of how the seam was framed when the
implementation target was the live `servo-layout` crate. Don't use it as a
forward reference.

---

Companion to [2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md).
Tracks the concrete implementation of P2 ("remove script from servo-layout") from that plan.

---

## Current state

`servo-layout/Cargo.toml` still lists `script = { workspace = true }`.
Layout code names concrete script DOM types directly throughout:
`ServoLayoutNode`, `ServoLayoutElement`, `ServoDangerousStyleElement`, `ServoDangerousLayoutElement`.

Checkpoint:

- Merge conflicts in `components/layout/display_list/mod.rs`,
  `components/layout/flow/inline/text_run.rs`, and 7 files under
  `components/script/dom/*` are resolved (2026-05-13).
- `cargo check -p serval-static-html`, `-p serval-static-dom`, and
  `-p servo-layout-api` pass.
- **P2 Step 1 (layout_provider re-export) is mechanically complete.** All
  17 `use script::layout_dom::*` sites in `components/layout` now go
  through `crate::layout_provider`. Only `layout_provider.rs` itself
  names `script::layout_dom`.
- **P2 Step 2+ blocked**: `cargo check -p servo-layout` end-to-end is
  blocked by an unfinished P1 issue — `ScriptLayoutHostServices` does
  not satisfy the `Send + Sync` bound on `LayoutHostServices`. See
  [profile ladder plan, P1 fallout addendum](./2026-05-12_serval_profile_ladder_plan.md#p1-fallout-the-script-host-impl-is-not-sync).

---

## The seam: `LayoutDomTypeBundle`

`shared/layout/layout_dom.rs` already defines `LayoutDomTypeBundle<'dom>` — an associated-type
bundle whose docstring reads: "other types (specifically the implementation of the Layout trait)
can be parameterized over a single type rather than all of the various Layout DOM trait
implementations." This is the seam. P2 makes it real.

The wrong alternative (the `LayoutDataProvider` approach drafted by another model) adds a
parallel method-delegating trait and proposes moving `InnerDOMLayoutData` to `shared/layout`.
That does not work: `InnerDOMLayoutData::self_box` is `ArcRefCell<Option<LayoutBox>>`, and
`LayoutBox` wraps `BlockLevelBox`, `FlexLevelBox`, `InlineItem`, `TableLevelBox`, `TaffyItemBox`,
`TextRun` — all defined in `components/layout`. Moving the struct would drag the entire box-tree
type hierarchy with it, or require type parameters, or trait objects per node. None of those is
a narrow cut.

---

## The storage contract: `NodeExt` made public

`components/layout/dom.rs:327` defines `pub(crate) trait NodeExt<'dom>` with the full storage
contract that layout needs from each node:

- `ensure_inner_layout_data`, `inner_layout_data`, `inner_layout_data_mut`
- `box_slot() -> BoxSlot<'dom>`
- `unset_all_boxes`, `rendering_type`, `fragments_for_pseudo`
- `with_layout_box_base_including_pseudos`, `repair_style`
- `isolates_damage_for_damage_propagation`, `rebuild_box_tree_from_independent_formatting_context`

`InnerDOMLayoutData`, `BoxSlot`, and `LayoutBox` all stay in `components/layout` — they are
layout-internal types. `NodeExt` becomes `pub` (not pub-in-shared, just pub in `servo-layout`),
and the hot-path functions carry a `where T::ConcreteLayoutNode: NodeExt<'dom>` bound locally.

The dependency arrow after P2:

```text
servo-script  →  servo-layout  (ServoLayoutNode implements NodeExt from servo-layout)
servo-layout  →  shared/layout (LayoutDomTypeBundle, LayoutNode, LayoutElement)
servo-layout  ↛  servo-script  (target)
```

No cycle: `servo-layout` defines `NodeExt`; `servo-script` imports `servo-layout` to implement it.

---

## Implementation order

### Step 1 — Localize script DOM imports (no semantic change)

Create `components/layout/layout_provider.rs` that re-exports the concrete script DOM types:

```rust
// components/layout/layout_provider.rs
pub(crate) use script::layout_dom::{
    ServoLayoutElement, ServoLayoutNode, ServoDangerousLayoutElement,
    ServoDangerousStyleElement,
};
```

Change every `use script::layout_dom::*` site in `components/layout` to `use crate::layout_provider::*`.
After this step, all script DOM names are imported from one place. `cargo check -p servo-layout`
behavior is unchanged (still fails without MOZILLABUILD/NASM).

Done condition: no `use script::` imports remain in `components/layout/**/*.rs` other than
`layout_provider.rs` itself.

### Step 2 — Make `NodeExt` public and generify hot paths

- Rename `NodeExt` to something that signals its public status, or simply change `pub(crate)`
  to `pub`. Export it from `servo-layout`'s pub API (or a well-named submodule).
- Make `BoxSlot<'dom>` pub-exported from `servo-layout`.
- Change `LayoutThread::handle_reflow` and `dom_traversal` to be generic:

```rust
fn handle_reflow<'dom, T>(...)
where
    T: LayoutDomTypeBundle<'dom>,
    T::ConcreteLayoutNode: NodeExt<'dom>,
{ ... }
```

The bound on `ConcreteLayoutNode: NodeExt<'dom>` is local to the `servo-layout` crate, so
no visibility issue.

Done condition: layout functions compile against the generic bound; `ServoLayoutNode` still
satisfies `NodeExt` via its existing impl.

### Step 3 — Implement `NodeExt` for `StaticLayoutNode`

In `serval-static-dom`, add a `NodeExt<'dom>` impl for `StaticLayoutNode<'dom>`:

- `ensure_inner_layout_data` / `inner_layout_data*`: return a private no-op `DOMLayoutData`
  (empty `InnerDOMLayoutData` wrapped in the required borrow type).
- `box_slot`: return a slot backed by an `ArcRefCell<Option<LayoutBox>>` held on the node.
- `unset_all_boxes`, `rendering_type`, `fragments_for_pseudo`: sensible no-ops for a static DOM
  (no boxes yet, `NotRendered`, empty vec).
- `isolates_damage_*` / `rebuild_box_tree_*`: return `false` (no incremental layout yet).

`serval-static-dom` will need `servo-layout` in its dependencies for this impl.

Done condition: `cargo check -p serval-static-dom` passes with the impl added.

### Step 4 — Implement `LayoutDomTypeBundle` for static DOM

Add a `ServalStaticDomBundle` type in `serval-static-dom` (or `serval-static-html`) that
sets:

- `ConcreteLayoutNode = StaticLayoutNode<'dom>`
- `ConcreteLayoutElement = StaticLayoutElement<'dom>`
- `ConcreteDangerousStyleNode`, `ConcreteDangerousStyleElement` — static-dom implementations.

Done condition: the bundle type compiles; `LayoutThread` can be instantiated with it
(even if it panics at runtime — P3 concern).

### Step 5 — Remove `script` from `servo-layout/Cargo.toml`

Remove `script = { workspace = true }` and delete `layout_provider.rs`.
Replace its contents with: imports from `layout_api`/`shared/layout` only.
At this point every concrete `ServoLayoutNode` ref in `components/layout` is gone.

Done condition:

```powershell
cargo check -p servo-layout   # no MOZILLABUILD or NASM required
cargo tree -p servo-layout | rg "servo-script|mozjs|aws-lc"   # no matches
cargo check -p serval-static-html   # gate still passes
powershell -ExecutionPolicy Bypass -File support/profile-gates/check-static-html.ps1
```

---

## What stays out of scope (P3)

- Wiring `StaticLayoutNode` through the actual layout + paint + NetRender pipeline.
- Making layout produce any output for static DOM (no-op impls are enough for P2).
- Moving `InnerDOMLayoutData` or `LayoutBox` to `shared/layout`.
- Removing `mozjs` from the fullweb/scripted profile — script still uses it, rightly.

---

## aws-lc-sys note

`aws-lc-rs` enters via two paths: WebCrypto in `components/script`
(`dom/webcrypto/subtlecrypto/*`) and TLS in `components/net` (rustls feature `aws-lc-rs`).
Neither belongs in `servo-layout`. After Step 5, `cargo check -p servo-layout` no longer
requires NASM. Swapping the crypto backend for fullweb is a separate decision and not worth
fighting upstream conventions for.
