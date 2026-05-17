# serval-layout lift plan (path C)

Implementation plan for path C of the profile ladder: lift the portable parts of dead-on-disk `components/layout/` into a new `serval-layout` workspace crate, with `serval-static-dom` as the first DOM provider plugged in behind a profile-neutral `LayoutDom` trait.

**Anchors:**

- Strategy: [2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md) (profile ladder framing — still canonical as strategy).
- State: [2026-05-16_workspace_audit_snapshot.md](./2026-05-16_workspace_audit_snapshot.md) (the audit that moved servo-layout / servo-script to dead-on-disk).
- Phase mechanics that have shipped: see "Implementation checkpoint - 2026-05-12" inside the strategy doc.

---

## 2026-05-17 planes-architecture update (supersedes much of this doc)

After reading Blitz's `packages/blitz-dom/` and synthesizing with the path-C goals, the architecture has been rewritten as a **planes** design. The authoritative references (read together):

- [2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md) — `serval-layout`'s piece (Style + Layout + Fragment + Paint planes for HTML, publishing the FragmentQuery + InteractionQuery surfaces).
- [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md) — the **cross-engine** architecture. Hekate is a router + document-intelligence layer, not a renderer; lanes are Nematic / Middlenet / Serval fullweb / system-webview-fallback; extract is Hekate's own work with E0–E4 tiers; observable-plane vocabulary is shared across lanes. Serval is **one lane** in this system.

Read both first; this lift plan is updated to align.

**Headline changes** vs. the path-C lift framing below:

- **No more "lift Servo's layout machinery."** Servo's flow / flexbox / inline / formatting-context / box-tree / fragment-tree code is **dropped** (~30k lines). We use **Stylo for cascade + Taffy for layout + parley for inline text + lifted Servo `display_list/` for emission** (plus small selected lifts for list markers, quotes, WM geometry, replaced sizing, style helpers).
- **No more `layout_api` LayoutNode / LayoutElement / DangerousStyleNode / DangerousStyleElement bridge.** Blitz proved this scaffolding is Servo-internal; nothing else consumes it. `NodeRef<'a, D>` impls Stylo's trait family directly (~1000 lines), no four-type bundle.
- **No more "generic propagation through layout."** The lifted code consumes plane data (`StylePlane`, `LayoutPlane`, `FragmentPlane`), not LayoutNode-trait constraints. Generic parameterization is bounded to the planes' `D: LayoutDom` boundary.
- **Mutable rendering state lives in planes**, not embedded on DOM nodes. Each plane is keyed by `D::NodeId` and owned by `serval-layout`. Multi-DOM-provider goal preserved (static / scripted / reader-mode all use the same planes); DOM crates stay clean.

**Lift size revised:** ~5.5k lines lifted (display_list + style helpers + WM geometry + list markers + replaced sizing + quotes), plus ~3-4k lines of new plane construction / Stylo adapter / Taffy glue / parley wiring. Net `serval-layout` size: ~8-10k lines, down from the path-C plan's ~30k.

**Next executable slice (replaces the "adapter compile gate" below):** smallest end-to-end that wires `NodeRef` + minimal Stylo adapter + StylePlane skeleton + tiny `construct.rs` that builds a Taffy tree for one `<p>` + Taffy run + log the rect. ~500 lines. Validates Taffy + parley + plane storage choices on a real surface. See the planes doc for the full slice description.

The 2026-05-16 review section that previously lived here is **preserved below** as historical context — it documents the path-C framing the planes architecture supersedes. Don't follow its file-by-file lift order; use the planes doc's "lift now / lift later / drop" categorization instead.

---

## 2026-05-16 review update — first-step implications (historical, superseded 2026-05-17)

The first steps changed this from a speculative port plan into an adapter-first
lift:

- `components/shared/layout-dom/` exists and `serval-static-dom::StaticDocument`
  implements `layout_dom_api::LayoutDom`.
- `components/serval-layout/` exists, builds, and has a structural
  `LayoutDomAdapter<'a, D>` plus three adapter smoke tests.
- `adapter_stylo.rs` exists as a deliberately unwired draft. That is the right
  place for the next hard proof, but it should not be hand-written from memory
  again.
- The old per-file port order was too optimistic. The failed bulk move proved
  that the real cost is generic propagation away from concrete
  `Servo*Layout*` types, not file movement.
- The current dependency gate should be package-name-specific. The broad
  `rg "script|..."` form is wrong because clean graphs contain crates like
  `unicode-script`.

Critical correction: do **not** bulk-move more layout files until the adapter
compile gate below is green. Without a compiling `LayoutDomAdapter` bridge into
the existing `layout_api` / Stylo trait world, the next bulk move only recreates
the same unresolved concrete-type collapse.

**[2026-05-17 update: this critical correction is itself superseded. The adapter compile gate's target shrinks under the planes architecture — only the Stylo traits need impl'ing, not the layout_api bridge. The "next bulk move" framing is moot because we're not bulk-moving Servo's layout machinery anymore.]**

---

## Why path C (vs. A and B)

The 2026-05-12 plan assumed `servo-layout` and `servo-script` would remain live workspace members while their `script` couplings were sliced out. The 2026-05-15 audit moved both crates out of `workspace.members`. `components/layout/Cargo.toml` still names `script = { workspace = true }` (line 48), but neither the crate nor that dep entry compiles — the audit also removed `script` from `workspace.dependencies`.

That invalidates the plan's premise. Three options were on the table when the audit landed:

- **A. Revive `servo-layout`** as a workspace member with `serval-static-dom` as its sole DOM provider. Resurrect; complete P1/P2/P3 in place. *Re-introduces the build-graph footprint the audit removed; the script-DOM couplings in `components/layout/` are not just at `Cargo.toml` — `ServoLayoutNode` / `ServoDangerousStyleElement` are named throughout the box-construction, traversal, and query code.*
- **B. Build a fresh minimal layout path** from `StaticDocument` to `ServalDisplayList`. *Discards a working CSS layout engine we already wrote. Even Blitz uses Stylo + Taffy — reinventing from zero is not the scope.*
- **C. Lift portable layout into a new `serval-layout` crate** — the DOM-neutral parts of `components/layout/` (style integration, box tree, fragment tree, display list emission), with the `script::layout_dom::*` couplings dropped as the move-cost and replaced by a `LayoutDom` trait. *Pro: keeps the working work, lands the script-free graph the audit was building toward, naturally supports two DOM providers (static now, scripted later). Con: real porting work, judgment-call per file.*

**Chosen: C.** It's what the strategy doc originally intended; the only thing it didn't anticipate is that the lift is a port out of a dead crate, not an edit of a live one. C also lines up with the broader strategic direction: a Blitz-shaped pipeline that can stack the JS engine and constellation back on top to climb the profile ladder.

---

## How path C lines up with broader serval goals

Cross-checking the lift against the strategic anchors:

| Goal | Path C alignment |
| --- | --- |
| Profile ladder (static → interactive → scripted → fullweb) | Direct. `serval-layout` becomes the shared middle layer above static-dom and (future) scripted-dom providers. |
| Three-head Hekate (extract / middlenet / fullweb) | Middle and full heads share `serval-layout`. Extract head bypasses it for readability-style text flow. Mark divergence points during the lift; don't refactor later. |
| Shared NetRender output across profiles | `serval-layout` emits `ServalDisplayList`. `servo-paint` consumes it. Output target unchanged. |
| Blitz convergence | The end shape (Stylo + Taffy + serval-internal box/fragment trees + display list emission) is the same general shape as Blitz's `packages/dom + packages/stylo + packages/taffy_layout + packages/paint`. A side-by-side audit becomes meaningful once the lift is done. |
| Glass-HQ/gpui host via PlatformSurface | Orthogonal — layout emits display lists; PlatformSurface is downstream of paint. No coupling. |
| Wasm/browser target (eventually) | Lift must stay wasm-friendly. Don't pull native-only sync primitives or threading assumptions into `serval-layout`. Audit `Send + Sync` decisions explicitly. |
| Vanilla Windows build (audit baseline) | Hard requirement. Every batch must end with the package-name canary returning empty for `servo-script`, `servo-script-traits`, `servo-script-bindings`, `mozjs`, and `mozjs_sys`. Do not use a broad `script` substring check; it false-positives on crates like `unicode-script`. |
| Mere ecosystem fit (inker routes to serval) | Strengthened — inker can carry a "profile preference" payload to serval per route once the profiles are real packages. |
| W3C-capability-knockout pattern | Apply during lift: paint worklets (CSS Houdini Painter), WebXR layout integration, service-worker hooks all get stubbed or deleted, not ported. |

---

## Phase plan (path C)

The phase numbering from the 2026-05-12 plan stays, but the content changes:

### P1 — Layout host services (already shipped at trait level)

`LayoutHostServices` + `NoOpLayoutHostServices` exist in `layout_api`. Trait carries `Send + Sync`. The only impl in the live graph is `NoOpLayoutHostServices` (trivially `Sync`). The script-side `ScriptLayoutHostServices` adapter referenced in the P1 fallout addendum is in dead-on-disk code (`components/script/script_thread.rs:229`).

**Status: no work needed.** Revisit when a scripted DOM provider lands and a real `LayoutHostServices` impl needs to satisfy the `Send + Sync` bound. At that point the decision is: route script-thread messages through a Sync-clean IPC sender (the originally-recommended option 2 from the P1 fallout addendum), or relax the bound on `LayoutHostServices`. Decide then, not now.

### P2 — Stand up `serval-layout`

This is the bulk of the work. Subdivided:

**P2.1 — Crate scaffold.** New `components/serval-layout/` workspace member. Current scaffold dependencies include the adapter/probe surface (`layout_dom_api`, `layout_api`, `paint_types`, `stylo`, `stylo_atoms`, `stylo_dom`, `stylo_traits`, `selectors`, math/Servo shared types). **Must not depend on** `script`, `script_traits`, `script_bindings`, `mozjs`, or any `components/constellation`-shaped types.

`net_traits` needs a more precise rule. The current graph already reaches
`servo-net-traits` through `layout_api`, and `serval-layout/Cargo.toml` also
has a direct `net_traits` entry while the adapter work is still incomplete.
That is not a SpiderMonkey/JS regression, but it is still dependency debt:
remove the direct `net_traits` edge unless the first real layout batch proves
it is needed, and move the stricter "no net traits in the static profile" goal
to the later `layout_api` cleanup pass.

Done condition: `cargo check -p serval-layout` succeeds, and the package-name
canary for `serval-layout` returns no `servo-script`, `servo-script-traits`,
`servo-script-bindings`, `mozjs`, or `mozjs_sys` packages.

**P2.2 — `LayoutDom` provider trait.** Define the profile-neutral DOM trait in `layout_api` (or a new `layout_dom_api` if it deserves its own crate — judgment call once the trait shape is known). The existing `components/layout/layout_provider.rs` stub describes the intent but currently re-exports `ServoLayoutNode` from dead-on-disk script — those re-exports become the trait surface. Specifically: `LayoutDom` provides typed access to nodes/elements/text/attributes/computed-style and a tree-traversal cursor, without naming a concrete provider type.

`serval-static-dom`'s `StaticDocument` implements `LayoutDom` immediately as the validation.

Done condition: `serval-static-dom` exposes a `LayoutDom` impl; `cargo check -p serval-static-dom` succeeds; a unit test in `serval-layout` exercises the trait against `StaticDocument` (e.g., walks the tree and reads element names).

**P2.3 — Port layout core.** Files in `components/layout/` move into `components/serval-layout/`, with each file's `script::layout_dom::*` references replaced by `LayoutDom`-shaped trait calls. The original low-coupling order below is now retained as a cautionary record, not as the live execution rule:

1. `geom.rs`, `layout_box_base.rs`, `cell.rs` — close to pure math/data.
2. `display_list/` — emits `ServalDisplayList`; minimal DOM touch.
3. `fragment_tree/` — fragment representation; references nodes only at boundaries.
4. `flow/` (excluding inline text), `flexbox/`, `formatting_contexts.rs` — box construction.
5. `flow/inline/*` — text layout. *Sidequest probe point for parley.*
6. `dom.rs`, `dom_traversal.rs`, `traversal.rs` — the parts that currently name `ServoLayoutNode` directly.
7. `construct_modern.rs`, `query.rs`, `accessibility_tree.rs` — query/build entry points.
8. `layout_impl.rs`, `lib.rs` — top-level glue.

The failed first attempt below invalidated the per-file green-commit rule. The
live rule is: adapter compile gate first, then layer/bulk moves with green
checkpoints at meaningful boundaries.

Done condition for P2.3: `serval-layout`'s public API covers what `serval-static-html` needs to layout a parsed `StaticDocument`.

#### Reality check 2026-05-16: file-order was based on names, not contents

After landing batch 1a (`cell.rs`, 90 lines, the one genuinely portable file) and reading the next candidates' actual contents:

- `geom.rs` (692 lines) names `crate::ContainingBlock` and `crate::sizing::Size` in real `impl` blocks, plus `style::*` / `style_traits::CSSPixel`. Not pure data.
- `layout_box_base.rs` (328 lines) has the deepest internal coupling of any layout file: 10+ `crate::` imports including `dom`, `flow`, `formatting_contexts`, `fragment_tree`, `positioned`, `sizing`. Worst possible "first batch."
- `sizing.rs` (852 lines, where `Size` lives) depends on `crate::style_ext`, `crate::layout_box_base`, `crate::ConstraintSpace`. Its own dep chain into the rest of layout.

Only `cell.rs` is genuinely leaf in the live dep graph. Past that, everything is interconnected through `crate::` paths. The file-order I wrote above was based on file *names* (sounds like math/data → must be portable); the real dep graph is denser.

This invalidates the "each batch is a separate commit with `cargo check` green at every step" sub-rule, because most single-file batches would require stubbing or forward-declaring half of layout — producing increasingly contorted intermediate states. Three viable strategies:

**Strategy 1 — Cluster by layer.** Port multiple files together as one logical layer per commit (e.g., "geometry + ContainingBlock + Size as a single 'core geometry/sizing types' batch"). Each commit still has `cargo check -p serval-layout` green; commits are larger but the dependency-graph reality is honored. Maybe 6–10 commits total for P2.3. Bisect-friendly *between layers*, not between files.

**Strategy 2 — Jump-ship single-commit port.** Move all of `components/layout/` to `components/serval-layout/` in one operation; fix what breaks; one commit when done. The "rip the parallel codepath, fix what breaks" model the netrender cut used. Mark's documented preference for prototypes ([feedback_jump_ship_over_migration_for_prototypes]). One big commit; less bisect-able mid-port, but the audit canary is binary — either the final state is script-free or it isn't. Faster.

**Strategy 3 — Selective port: minimum static layout only.** Port only the files static-profile layout needs (block + inline + text + float; skip table/, flexbox/, grid where possible; skip `query.rs`, `accessibility_tree.rs` for now). Smaller surface than 1 or 2; the cut decisions become real work. Aligns with the W3C-knockout pattern from the audit. Lands less code, but each piece is more deliberately scoped.

**Recommended: strategy 3 with strategy-2 mechanics for the porting itself.** Move the files we want; delete the rest; fix what breaks. One commit for the bulk port; follow-up commits for finishing touches (test wiring, naming cleanups). Rationale: the static profile genuinely doesn't need table/grid/flexbox/etc., and the W3C-knockout pattern says delete-now-rebuild-later. The bulk-port mechanic matches the prototype-stage feedback.

Outstanding choice: do we port `flexbox/` (modern CSS-essential, even for "simple" pages with `display: flex`) or skip it as a P2.3 cut? Block + inline + float is the genuinely minimum substrate; flex is heavily used in real-world pages. Likely answer: port flex, skip table + grid + taffy.

Per-file `cargo check`-green-at-each-step is **abandoned** in favor of "cargo check green at the end of the bulk port" plus "audit canary green at the end." Bisect granularity moves from per-file to per-layer-or-per-port-stage.

#### P2.3 batch checkpoints (status)

- ✅ **Batch 1a (2026-05-16):** `cell.rs` ported. `serval-layout` builds; audit canary empty.
- 🔴 **Strategy 3 + jump-ship mechanics + flex-in + W3C-scavenge picked (Mark, 2026-05-16). First attempt reverted — see "P2.3 attempt findings" below.**
- ✅ **P2.3 step 0 (2026-05-16):** `LayoutDomAdapter<'a, D>` scaffold landed. Structural methods backed by `LayoutDom`; three smoke tests pass; audit canary clean. Trait impls deferred.
- 🟡 **Adapter Stylo-trait impls (2026-05-16):** first-pass `adapter_stylo.rs` written from memory; signatures partly wrong (made-up methods, wrong return types, missing `Hash` / `AttributeProvider` impls, etc.). File preserved in repo as in-progress draft, **not mod-declared** so the build stays green. See its header for the exact errors and the next-session strategy (read script-side reference impls in full, adapt method-by-method).
- 🟢 **Review validation (2026-05-17):** `cargo check -p serval-layout`, `cargo test -p serval-layout`, `cargo check -p serval-static-dom`, and `cargo check -p serval-static-html` pass. The corrected package-name canary returns no script / mozjs packages for both `serval-layout` and `serval-static-html`.

#### P2.3 next executable slice — adapter compile gate (superseded 2026-05-17)

**[2026-05-17: this slice's target shrinks under the planes architecture.](./2026-05-17_serval_layout_planes_architecture.md) The Stylo adapter still gets written (now living in `serval-layout/src/stylo_adapter/`), but the four `layout_api` trait impls (LayoutNode / LayoutElement / DangerousStyleNode / DangerousStyleElement) and `LayoutDomBundle` are no longer needed — Blitz's read showed that scaffolding is Servo-internal and serves no consumer outside Servo's layout. The probe slice now also wires Taffy + parley + a StylePlane skeleton. See the planes doc for the full slice scope. The section below is preserved as the original gate framing.]**

Before any further bulk file movement, make the adapter compile against the
real trait surface. This is the smallest proof that path C is still viable.

1. **Remove accidental direct dependency debt first.** If `serval-layout` does
   not currently need a direct `net_traits` dependency, remove it. If an adapter
   impl needs image/media types before the first layout batch, document the
   exact method forcing it. Either way, keep `servo-net-traits` out of the
   SpiderMonkey canary; it is a different cleanup problem.
2. **Split adapter work into two files.**
   - `adapter_layout_api.rs`: `NodeInfo`, `LayoutNode`, `LayoutElement`,
     `DangerousStyleNode`, `DangerousStyleElement`, and `LayoutDomBundle`.
   - `adapter_stylo.rs`: Stylo / selectors traits only
     (`TNode`, `TElement`, `TDocument`, `TShadowRoot`,
     `selectors::Element`, `AttributeProvider`, `Hash`).
   This keeps the Serval-facing adapter boundary readable instead of burying it
   inside 1000 lines of foreign trait boilerplate.
3. **Use script-side reference impls as the signature oracle.** Read the six
   `components/script/layout_dom/servo_*` adapter files side by side and port
   signatures method-by-method. Do not infer trait signatures from memory.
4. **Prefer explicit static-profile stubs over fake behavior.** Methods for
   paint worklets, incremental restyle dirty bits, animation snapshots,
   shadow DOM, and pseudo-element mutation can return `false`, `None`, or
   `unimplemented!()` with a precise reason. Structural methods
   (`parent`, siblings, children, node kind, attrs, text) must delegate to
   `LayoutDom`.
5. **Add a compile-test smoke.** A tiny test should prove that
   `LayoutDomBundle<StaticDocument>` satisfies the layout/type-bundle bounds.
   It does not need to run style/layout yet; it just proves the adapter bridge
   is a real bridge.

Exit gate for this slice:

```powershell
cargo check -p serval-layout
cargo test -p serval-layout
cargo tree -p serval-layout --edges normal | rg '(^|[[:space:]])(servo-script|servo-script-traits|servo-script-bindings|mozjs|mozjs_sys)([[:space:]]|$)'
# Last command should produce no matches. Exit code 1 from rg means clean.
```

Only after this gate is green should the plan resume the layout-core file
movement / generic-propagation work.

#### P2.3 attempt findings (2026-05-16) — the real refactor is generic propagation

First attempt: bulk-moved all of `components/layout/` (minus `table/`, `taffy/`, `tests/tables.rs`, the `layout_provider.rs` stub) into `components/serval-layout/`, ported the Cargo.toml (drop `script` + `taffy`, add `layout_dom_api`), dropped the `mod table` / `mod taffy` / `mod layout_provider` declarations from `lib.rs`, replaced the layout_provider stub with an empty placeholder.

**`cargo check -p serval-layout` after the bulk move: 25 errors.** Sorted by class:

- 15× `unresolved import crate::layout_provider::*` (the `Servo*Layout*` re-export stub was deleted; every file that imported through it now fails).
- 5× `unresolved import crate::table::*` (table module is gone; references remain).
- 4× `unresolved import crate::taffy::*` (taffy module is gone; references remain).
- 1× unrelated borrow error in `replaced.rs` (likely pre-existing latent issue).

**Surface-level error count is small. The cascading work behind it is large.** The `crate::layout_provider::*` imports name four concrete types — `ServoLayoutNode<'dom>`, `ServoLayoutElement<'dom>`, `ServoDangerousStyleElement<'dom>`, `ServoDangerousStyleDocument<'dom>` — which the layout crate uses as **concrete types** throughout: function signatures (`fn foo(node: &ServoLayoutNode<'dom>)`), trait impl heads (`impl<'dom> NodeExt<'dom> for ServoLayoutNode<'dom>`), and field types.

The path-C design says these become a `LayoutDomAdapter<'a, D> { dom: &'a D, id: D::NodeId }` wrapper over `layout_dom_api::LayoutDom`. But that wrapper is **generic over `D`**. Every layout type that currently mentions `ServoLayoutNode` either:

1. Becomes generic over `D: LayoutDom` (and its associated `LayoutNode` impl), propagating `D` through `Layout<D>`, `BoxTree<D>`, `Fragment<D>`, etc. Stylo's `TElement` shim is a per-D adapter struct in `serval-layout`.
2. Or: stays concrete by picking a single `D` at lift time (e.g., `serval-static-dom::StaticDocument`), which defeats the abstraction (layout becomes coupled to one DOM impl; reader-mode head can't reuse it).

**Choice (1) is the path-C commitment.** It's mechanical but invasive: every `ServoLayoutNode<'dom>` reference becomes `N: LayoutNode<'dom>` generic, every `impl<'dom> Trait for ServoLayoutNode<'dom>` becomes `impl<'dom, N: LayoutNode<'dom>> Trait for N`. The trait surface from `layout_api::LayoutNode` (which `layout_dom_api::LayoutDom`-backed adapters will satisfy via `LayoutDomAdapter`'s impl) is what we're constraining over.

**Why the bulk-port-then-fix model alone isn't enough.** Once the four `Servo*Layout*` type names disappear, the per-file edits to swap them for generic parameters are not just 15 import fixes — they're sweeping changes to function and impl signatures throughout the crate. Without doing those, `unimplemented!()` stubs only push the cascade onto the trait impls, which themselves call methods that need real bodies.

Today's attempt was reverted to keep `main` audit-clean. File moves can be reproduced quickly next session; the deep work is the generic propagation sweep.

#### P2.3 strategy revision — paired moves

The revised plan for P2.3, paired with the file moves:

1. **Pre-move sweep**: in `components/layout/` (or right after the move, before re-attempting cargo check), do a search-and-replace pass on the four `Servo*Layout*` type names:
   - `ServoLayoutNode<'dom>` → `N` where N is a new generic parameter `N: LayoutNode<'dom>`
   - `ServoLayoutElement<'dom>` → `E: LayoutElement<'dom>`
   - `ServoDangerousStyleElement<'dom>` / `ServoDangerousStyleDocument<'dom>` → corresponding trait constraints
   - Every function and impl head that mentions them gets the generic parameter added.
   This is the bulk of the refactor work. Mechanical, search-and-replace-amenable with care.
2. **`LayoutDomAdapter` in `serval-layout`**: define `LayoutDomAdapter<'a, D: LayoutDom>` that impls `layout_api::LayoutNode<'dom>` / `LayoutElement<'dom>` / etc. by delegating to `LayoutDom` primitives. This is the bridge from `layout_dom_api` to `layout_api`. Stylo's `TElement` adapter (`StyleElement<'a, D>`) is the same pattern applied to the foreign Stylo traits.
3. **`Layout<D>` top-level type**: the public layout entry point becomes parameterized over D. `serval-static-html` picks `D = StaticDocument` and constructs `Layout<StaticDocument>`.
4. **Dead-code cleanup**: cut/stub paint worklets, animation rules, restyle dirty-bits where the static profile won't reach them (per W3C-knockout pattern and the Stylo paper probe findings).
5. **`accessibility_tree.rs` + `query.rs`** are kept as "W3C goods" per Mark's direction — they get the same generic-propagation treatment.

Scope estimate: 1–2 focused days. Single-session ambition is unrealistic given the breadth of touch sites (17 layout files reference `Servo*Layout*` types, ~98 total references). Realistic next session: pick **either** the bulk move + generic-propagation sweep as one large WIP commit, **or** start with `LayoutDomAdapter` definition + a small subset of layout files (e.g., just `fragment_tree/` since it's output-shaped and less DOM-touching).

#### LayoutDomAdapter trait-impl approach (lesson from 2026-05-16)

When wiring `LayoutDomAdapter` to satisfy `layout_api`'s `LayoutNode<'dom>` / `LayoutElement<'dom>` / `DangerousStyleNode<'dom>` / `DangerousStyleElement<'dom>` (which transitively requires `style::dom::{NodeInfo, TNode, TDocument, TShadowRoot, TElement, AttributeProvider}` and `selectors::Element`):

**Don't write from memory.** The trait surface is ~125 methods across 8+ traits. Writing from memory produces high error rates on:

- Made-up methods that don't exist on the trait.
- Wrong return types (`Option<&AtomIdent>` vs. `Option<&WeakAtom>` for `id`; `style::data::AtomicRef` (private) vs. `ElementDataRef` for `borrow_data`).
- Wrong crate paths (`dom::ElementState` is actually `stylo_dom::ElementState`; `ElementSelectorFlags` lives in `selectors::matching`, not `style::dom`).
- Missing super-trait impls (TElement requires `Hash + AttributeProvider`, not just `SelectorsElement`).
- Wrong associated-type references (`Self::ConcreteShadowRoot` should be `<Self::ConcreteNode as TNode>::ConcreteShadowRoot`).

**Do read the reference impls side-by-side.** The script-side reference impls are:

- `components/script/layout_dom/servo_layout_node.rs` (332 lines) — LayoutNode + TNode impls.
- `components/script/layout_dom/servo_layout_element.rs` (258 lines) — LayoutElement impl.
- `components/script/layout_dom/servo_dangerous_style_node.rs` (151 lines) — DangerousStyleNode + TNode impls.
- `components/script/layout_dom/servo_dangerous_style_element.rs` (933 lines) — DangerousStyleElement + TElement + selectors::Element + AttributeProvider impls. The big one.
- `components/script/layout_dom/servo_dangerous_style_document.rs` (100 lines) — TDocument impl.
- `components/script/layout_dom/servo_dangerous_style_shadow_root.rs` (56 lines) — TShadowRoot impl.

The script-side reference takes ~1830 lines total. Our `LayoutDomAdapter` will be shorter because:

- Single type for all four bundle slots (script splits into ServoLayoutNode / Element / DangerousNode / DangerousElement; our adapter dispatches on `kind()`).
- Static profile stubs many cascade-side methods with `unimplemented!()` (paint worklets, atom-interned id/class, restyle dirty bits) until the cascade lights up later.

Realistic length for `adapter_stylo.rs` + `adapter_layout_api.rs`: 800–1200 lines combined, of which ~70% is signature boilerplate matching the reference, ~25% is structural method bodies dispatching through `LayoutDom`, ~5% is `unimplemented!()` panics.

**Per-trait order matters because of associated-type cycles.** TNode requires `ConcreteElement: TElement`, TElement requires `ConcreteNode: TNode<ConcreteElement = Self>`. Both must be impl'd before either trait bound resolves. Write all the trait impls in one file, run `cargo check`, fix the resulting errors against the reference impl. Don't try to land traits one at a time.

**In-progress draft.** `components/serval-layout/adapter_stylo.rs` exists from the 2026-05-16 first-pass attempt; it's in the repo but **not mod-declared in `lib.rs`** (so the build stays green). Its file header enumerates the specific signature errors from the first pass. Next session can either: (a) treat it as a starting structural template (the file *shape* — which traits are impl'd, the LayoutDom-dispatch pattern for structural methods — is mostly right even where signatures aren't), and rewrite each impl block against the script reference; or (b) delete it and start fresh from the script reference. Either is valid.

#### Per-session checkpoint protocol

To make P2.3 sessions resumable:

- Each session starts from a known-green `main` (audit-clean, `cargo check --workspace` green).
- Each session ends by either:
  - Landing a green commit on `main` (substantive progress).
  - Committing a WIP branch (`wip/p2.3-…`) with notes on what's broken and what was done. Never land broken state on `main`.
- The lift plan's status block + `wip/` branch list is the source of truth for "where are we."

**P2.4 — Delete `components/layout/` and `components/script/` from disk.** Same commit series. No "keep around just in case" — two layout implementations is worse than one. The serval audit snapshot's dead-on-disk list (`components/net`, `components/devtools`, `components/storage`, etc.) can come along in a sweep, but `layout/` and `script/` are the load-bearing ones for this phase.

### P3 — Static HTML first-pixel smoke

`serval-static-dom` parses HTML. `serval-static-html` is currently a witness that constructs a `StaticHtmlProfile` and parses; it does not invoke layout. After P2.3, wire the pipeline end-to-end:

```text
HTML string
  → serval_static_dom::StaticDocument::parse(html)
  → serval_layout::layout(&doc, &host_services, viewport)   // emits ServalDisplayList
  → servo_paint::Paint::render(&display_list)
  → NetRender readback to RGBA8 buffer
```

The smoke test renders a single CSS-styled paragraph and asserts the readback has non-background pixels in the expected region. Lives in `components/serval-static-html/tests/first_pixel.rs`.

Done condition: the smoke test passes locally on Windows + macOS (already-validated targets per audit snapshot).

### P4 — Scripted DOM provider scaffold

Design (not implement) the `LayoutDom` impl that a future scripted profile will use. The implementation will be a thin adapter over Servo's existing script DOM — but that script DOM doesn't live in the workspace today, so this phase is a stub:

- A `serval-scripted-dom` placeholder crate that documents the intended shape.
- A `serval-scripted` profile facade placeholder (analog to `serval-static-html`).
- A note in the strategy doc about which Servo script-DOM commit/tag the future port will start from.

No code paths from `serval-scripted` get wired up. This phase exists to make the eventual reintroduction non-mysterious and to surface the `LayoutHostServices` `Send + Sync` decision before it bites.

### P5 — Profile facade packages

Already shipped for `serval-static-html`. Add `serval-interactive-html` and `serval-fullweb` as placeholder crates with documented intent and `cargo check`-green empty libs. Wire `support/profile-gates/` to assert that each profile's `cargo tree` doesn't carry capabilities above its level (e.g., `serval-interactive-html` doesn't carry `mozjs`).

### P6 — Low-profile pipeline split

After P3 lands, `serval-static-html` has a direct pipeline that doesn't need `components/constellation/` lifecycle. Confirm this. Don't extract a shared `pipeline-core` until/unless fullweb is alive and we can prove the overlap.

### P7 — Wasm host (later)

Unchanged from strategy doc. Kept here so the lift's wasm-friendliness assumptions are pinned: no native-only sync primitives in `serval-layout`, no `pollster::block_on` startup in any path that wasm builds reach.

---

## Sidequests worth picking up alongside path C

These are not blockers but produce dividends if done while the porter's hands are already in the files:

- **Blitz side-by-side audit.** Now feasible per the audit snapshot — serval's shape is finally narrow enough to lay next to `linebender/blitz`'s `packages/*` and read the overlap. Specifically, their `packages/dom` (DOM abstraction), `packages/stylo` (Stylo integration), `packages/taffy_layout` (Taffy wrapping), and `packages/paint` (display-list lowering) map to where `serval-layout` is heading. Likely surfaces pieces of Blitz worth pulling rather than porting; informs the per-file judgment of what counts as "portable."
- **Parley plug-in point.** Per ecosystem decision, text layout should be parley (host-agnostic embedder layer), not the iced-coupled cosmic-text path. The lift's natural decision point is when `flow/inline/*` moves (P2.3 step 5). Mark the parley seam even if the wiring happens in a follow-up; the alternative is a second pass through the same code months later.
- **Stylo pin discipline.** Stylo isn't on crates.io. It comes through `workspace.dependencies` as `stylo = { ... }` — likely via path or git dep through servo-stylo. During the lift, audit how `stylo` resolves and pin the source explicitly so `cargo update` can't silently move serval onto a Servo branch we didn't expect. (Workspace pins drive probes; the rule applies here.)
- **Three-head Hekate touchpoints.** Mark in code (comments or `#[doc]`) which layout entry points are middle/full-only, and where a future smolweb-extract head would bypass the layout pipeline for readability-style text flow. Even if extract-head implementation is months away, knowing the divergence saves a refactor.
- **`layout_api` cleanup pass.** The interface crate still carries `Painter` / `DrawAPaintImageResult` / `PaintWorkletError` that the static profile will never need. Decide whether to feature-gate these or split them into a fullweb-only API crate. Affects whether `serval-layout` can be built with no paint-worklet code paths at all (W3C-knockout pattern).
- **`servo-` package prefix rename.** Not blocking, but when introducing `serval-layout`, set the precedent: `name = "serval-layout"`, not `name = "servo-layout-v2"`. The 56 remaining `package = "servo-..."` workspace entries can follow in a subsequent rename pass.
- **Reach-readability decision.** The static profile renders raw HTML; the smolweb-extract head will use readability heuristics. Sometime during P3, decide whether readability extraction lives in `serval-static-dom` (preprocessing) or in a separate `serval-reader-dom` crate. Affects how `nematic` and `serval` divide work for reader-mode requests.

---

## Pitfalls to keep in view

- **Re-introducing the SpiderMonkey env requirement.** `serval-layout`'s deps must be vetted at each step. Adding a transitive crate that depends on `servo-script` / `servo-script-traits` / `servo-script-bindings` / `mozjs` would silently re-introduce NASM / MOZILLABUILD / clang-cl. The canary must match package names, not the substring `script`, because clean graphs contain crates like `unicode-script`. Single line in `workspace.exclude` separates us from the world of NASM.
- **Stylo couplings to script-shaped types.** Servo's `style` crate has historically assumed a JS-reflector-shaped DOM in places (`OriginatingElement` and friends). The finished `StaticDocument` is Vec-backed; the `Rc<RefCell<...>>` only exists in the parser sink, so the current concern is not "StaticDocument is Rc and therefore not Sync." The real concern is whether the adapter can satisfy Stylo's trait/lifetime expectations without smuggling script-DOM assumptions into `layout_dom_api`.
- **Paint Worklet / CSS Houdini Painter.** `Painter` / `PaintWorkletError` already live in `layout_api`, but only fullweb implements the trait. The static profile must never invoke a worklet. Make sure `serval-layout` either feature-gates the worklet code path or stubs it to `unreachable!()` — the canary is `cargo tree | rg paint_worklet`.
- **`Send + Sync` bound on `LayoutHostServices`** (carry-over from P1 fallout). The bound currently has no real cost because `NoOpLayoutHostServices` is trivially Sync. The cost lands when P4's scripted DOM lands. The P1 fallout addendum recommended option 2 (Sync-clean IPC router) — that recommendation is preserved. The other options (Mutex-wrap, polling, etc.) remain viable; decide at P4, not before.
- **Ship of Theseus during the port.** "Portable" is a judgment call per file. Make each port a separate commit with `cargo check -p serval-layout` green and the profile-gate canary empty at every step. Bisect-friendly history is the only way to catch a silent script-coupling reintroduction during a multi-week port.
- **Keeping `components/layout/` alive "just in case."** Two layout implementations on disk are worse than one: contributors land changes in the wrong file, documentation drifts, and the dead crate looks alive enough to trick a `cargo check` if someone accidentally re-adds it to `workspace.members`. Delete `components/layout/` and `components/script/` from disk in the **same commit series** that finishes the port — not later.
- **NetRender shape divergence.** `serval-layout` must emit the same `ServalDisplayList` shape `servo-paint` consumes today. Don't let "we're starting fresh, let's improve the display list" sneak in during the port. Display-list redesign is a separate decision worth making on its own merits, not riding shotgun on the layout lift.
- **vello 0.9 freshness.** Per audit snapshot, vello 0.9 dropped 2026-05-15. If linebender ships a hotfix that changes a display-list-facing API during the lift, absorb it forward — don't pin to 0.8 for stability and miss the convergence window.
- **stylo / web_atoms / icu version skew.** `components/layout/Cargo.toml` pins `icu_locid`, `icu_properties`, `icu_segmenter`, `selectors`, `stylo_atoms`, `web_atoms`. During the lift, these come along, but their versions interact with whatever `stylo` itself wants. A version mismatch in this region is usually opaque — the error surfaces as a trait-resolution failure on `Element` or `Atom`, not as a version-mismatch message. If P2.3 step 4 (formatting contexts) gets stuck on a strange selector-trait error, suspect this region.
- **Don't conflate "profile-neutral DOM" with "Servo's DOM minus script."** Servo's DOM is JS/reflector/GC-shaped at its core. The `LayoutDom` trait should be designed *outward* from what `serval-layout` actually consumes, not *inward* from what Servo's DOM happens to expose. If the trait looks like Servo's DOM, the scripted-DOM impl will be trivial but the static-DOM impl will have absorbed Servo-DOM-shaped complexity for nothing.

---

## Validation ladder (path C)

After each P2.3 checkpoint:

```powershell
cargo check -p serval-layout
cargo test -p serval-layout
cargo check -p serval-static-html
cargo tree -p serval-layout --edges normal | rg '(^|[[:space:]])(servo-script|servo-script-traits|servo-script-bindings|mozjs|mozjs_sys)([[:space:]]|$)'
cargo tree -p serval-static-html --edges normal | rg '(^|[[:space:]])(servo-script|servo-script-traits|servo-script-bindings|mozjs|mozjs_sys)([[:space:]]|$)'
# The cargo tree commands should produce no matches. Exit code 1 from rg means clean.
```

After P3 lands:

```powershell
cargo test -p serval-static-html --test first_pixel
cargo check --workspace
```

End-to-end done condition (the entire path C complete):

```powershell
cargo check -p serval-layout
cargo check -p serval-static-html
cargo tree -p serval-layout --edges normal | rg '(^|[[:space:]])(servo-script|servo-script-traits|servo-script-bindings|mozjs|mozjs_sys)([[:space:]]|$)'        # empty
cargo tree -p serval-static-html --edges normal | rg '(^|[[:space:]])(servo-script|servo-script-traits|servo-script-bindings|mozjs|mozjs_sys)([[:space:]]|$)'   # empty
cargo test -p serval-layout                                                            # green
cargo test -p serval-static-html --test first_pixel                                    # green
cargo check --workspace                                                                # green
git ls-files components/layout/ components/script/                                     # empty
```

Last command's emptiness is the load-bearing check: the dead crates are *gone from disk*, not just from `workspace.members`.

---

## Open decisions

1. **Where does `LayoutDom` live?** **Resolved and landed:** new `layout-dom-api` crate (not `layout_api`). Plausible additional consumers — reader-mode/extract head, DOM serialization, querySelector helpers — clear the bar from the lift plan. See [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md).
2. **`LayoutDom` trait shape.** **Resolved and landed:** hybrid pattern — opaque `NodeId` lookups (pattern C foundation) with a default `walk` impl over a `NodeVisitor` trait (pattern B layered on top), no per-node-kind handle types. Real-world references: `petgraph::visit`, `rustc_hir::intravisit::Visitor`, `tree-sitter`'s cursor API, `html5ever::TreeSink`. See the same design doc for rationale, sketch, and exit criteria.
3. **Parley wiring timing.** **Resolved 2026-05-17:** parley is the inline text engine from the start, behind a Taffy `measure_function` that delegates to a `TextMeasure` trait. See planes doc `inline/` module sketch.
4. **Stylo version anchor.** Pin to a specific servo-stylo SHA or follow main. Affects upgrade cadence and stability. (Still open; current workspace pin is `572ecba2d160`.)
5. **`LayoutHostServices` Send + Sync** — keep the bound or relax. Resolved at P4, not before.
6. **`layout_api` shape after the lift** — under planes architecture, `layout_api`'s LayoutNode / LayoutElement / DangerousStyleNode / DangerousStyleElement become unused (no consumer post-Blitz-style adapter). Decide whether to delete those traits entirely or leave them as dead-on-disk in `layout_api`. Lean delete; smaller maintenance surface. Embedder-integration types (`LayoutHostServices`, `LayoutConfig`, `Layout` trait) stay.
7. **`net_traits` boundary** — current graph reaches `servo-net-traits` through `layout_api`, and `serval-layout` currently has a direct entry too. Decide whether the direct edge is removable now; decide later whether `layout_api` should split image/media-facing pieces so the static profile can become net-traits-free.
8. **Capability traits — crate location.** **New (2026-05-17):** `ReplacedElementProvider` / `FormControlProvider` / `EmbeddedContentProvider` live in `layout_dom_api`? Sibling `layout_capabilities_api`? Inside `serval-layout`? See planes doc; lean separate `layout_capabilities_api`.
9. **Plane visibility.** **New (2026-05-17):** are planes (`StylePlane`, `LayoutPlane`, `FragmentPlane`) public read-only API of `serval-layout` (so mere/apparatus/a11y can read FragmentPlane directly), or `pub(crate)`? Lean `pub` reads — "planes as reusable observables" framing depends on it.
10. **Display-list lift mechanism.** **New (2026-05-17):** Servo's `display_list/` consumes fragment-tree-shaped types (`Fragment`, `BoxFragment`). Define equivalents in `serval-layout::fragment` for high lift fidelity, or refactor the lifted display-list to read plane data directly? Lean equivalents — preserves lift value at small cost.
