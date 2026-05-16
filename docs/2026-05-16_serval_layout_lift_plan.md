# serval-layout lift plan (path C)

Implementation plan for path C of the profile ladder: lift the portable parts of dead-on-disk `components/layout/` into a new `serval-layout` workspace crate, with `serval-static-dom` as the first DOM provider plugged in behind a profile-neutral `LayoutDom` trait.

**Anchors:**

- Strategy: [2026-05-12_serval_profile_ladder_plan.md](./2026-05-12_serval_profile_ladder_plan.md) (profile ladder framing — still canonical as strategy).
- State: [2026-05-16_workspace_audit_snapshot.md](./2026-05-16_workspace_audit_snapshot.md) (the audit that moved servo-layout / servo-script to dead-on-disk).
- Phase mechanics that have shipped: see "Implementation checkpoint - 2026-05-12" inside the strategy doc.

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
| Vanilla Windows build (audit baseline) | Hard requirement. Every batch must end with the profile-gate canary (`cargo tree` piped through `rg "mozjs\|script_traits"`) returning empty. |
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

**P2.1 — Crate scaffold.** New `components/serval-layout/` workspace member. Cargo.toml depends on `layout_api`, `paint_api`, `paint_types`, `stylo`, `stylo_atoms`, `stylo_traits`, `taffy`, the math crates (`euclid`, `app_units`, `kurbo`), `servo-base`, `servo-config`, `servo-url`, `fonts`, `selectors`. **Must not depend on** `script`, `script_traits`, `script_bindings`, `mozjs`, `net_traits` (revisit if a load-image path needs it), or any `components/constellation`-shaped types.

Done condition: `cargo check -p serval-layout` succeeds on an empty `lib.rs`, and `cargo tree -p serval-layout | rg "script|mozjs"` is empty.

**P2.2 — `LayoutDom` provider trait.** Define the profile-neutral DOM trait in `layout_api` (or a new `layout_dom_api` if it deserves its own crate — judgment call once the trait shape is known). The existing `components/layout/layout_provider.rs` stub describes the intent but currently re-exports `ServoLayoutNode` from dead-on-disk script — those re-exports become the trait surface. Specifically: `LayoutDom` provides typed access to nodes/elements/text/attributes/computed-style and a tree-traversal cursor, without naming a concrete provider type.

`serval-static-dom`'s `StaticDocument` implements `LayoutDom` immediately as the validation.

Done condition: `serval-static-dom` exposes a `LayoutDom` impl; `cargo check -p serval-static-dom` succeeds; a unit test in `serval-layout` exercises the trait against `StaticDocument` (e.g., walks the tree and reads element names).

**P2.3 — Port layout core, batch by batch.** Files in `components/layout/` move into `components/serval-layout/`, in dependency order, with each file's `script::layout_dom::*` references replaced by `LayoutDom`-shaped trait calls. Suggested order (low-coupling first):

1. `geom.rs`, `layout_box_base.rs`, `cell.rs` — close to pure math/data.
2. `display_list/` — emits `ServalDisplayList`; minimal DOM touch.
3. `fragment_tree/` — fragment representation; references nodes only at boundaries.
4. `flow/` (excluding inline text), `flexbox/`, `formatting_contexts.rs` — box construction.
5. `flow/inline/*` — text layout. *Sidequest probe point for parley.*
6. `dom.rs`, `dom_traversal.rs`, `traversal.rs` — the parts that currently name `ServoLayoutNode` directly.
7. `construct_modern.rs`, `query.rs`, `accessibility_tree.rs` — query/build entry points.
8. `layout_impl.rs`, `lib.rs` — top-level glue.

Each batch is its own commit; each commit ends with `cargo check -p serval-layout` green and `cargo tree -p serval-static-html | rg "script_traits|mozjs"` empty. Bisect-friendly.

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

- **Re-introducing the SpiderMonkey env requirement.** `serval-layout`'s deps must be vetted at each step. Adding a transitive crate that depends on `script_traits` → `mozjs` would silently re-introduce NASM / MOZILLABUILD / clang-cl. The profile-gate check (`cargo tree -p serval-static-html | rg "mozjs"`) is the canary; run it after every batch of crate-additions during the lift. Single line in `workspace.exclude` separates us from the world of NASM.
- **Stylo couplings to script-shaped types.** Servo's `style` crate has historically assumed a JS-reflector-shaped DOM in places (`OriginatingElement` and friends). If `stylo` itself requires `Send`/`Sync` or specific lifetime patterns that `StaticDocument`'s Rc-based tree doesn't satisfy, the port stalls at the style-tree edge. Worth a probe in P2.1: try building a no-op style application against `serval-static-dom` and see what stylo demands.
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

After each P2.3 batch:

```powershell
cargo check -p serval-layout
cargo check -p serval-static-html
cargo tree -p serval-static-html | rg "script|script_traits|script_bindings|mozjs"
# Last command should produce no matches.
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
cargo tree -p serval-static-html | rg "script|script_traits|script_bindings|mozjs"   # empty
cargo test -p serval-layout                                                            # green
cargo test -p serval-static-html --test first_pixel                                    # green
cargo check --workspace                                                                # green
git ls-files components/layout/ components/script/                                     # empty
```

Last command's emptiness is the load-bearing check: the dead crates are *gone from disk*, not just from `workspace.members`.

---

## Open decisions

1. **Where does `LayoutDom` live?** **Resolved (pending review):** new `layout-dom-api` crate (not `layout_api`). Plausible additional consumers — reader-mode/extract head, DOM serialization, querySelector helpers — clear the bar from the lift plan. See [2026-05-16_layout_dom_api_design.md](./2026-05-16_layout_dom_api_design.md).
2. **`LayoutDom` trait shape.** **Resolved (pending review):** hybrid pattern — opaque `NodeId` lookups (pattern C foundation) with a default `walk` impl over a `NodeVisitor` trait (pattern B layered on top), no per-node-kind handle types. Real-world references: `petgraph::visit`, `rustc_hir::intravisit::Visitor`, `tree-sitter`'s cursor API, `html5ever::TreeSink`. See the same design doc for rationale, sketch, and exit criteria.
3. **Parley wiring timing.** P2.3 step 5 (port `flow/inline/*`) or a follow-up phase. Affects whether the first-pixel smoke uses parley or the existing inline-text path.
4. **Stylo version anchor.** Pin to a specific servo-stylo SHA or follow main. Affects upgrade cadence and stability.
5. **`LayoutHostServices` Send + Sync** — keep the bound or relax. Resolved at P4, not before.
6. **`layout_api` shape after the lift** — keep as one crate or split into static-profile-needed vs. fullweb-only. Resolved at the "layout_api cleanup pass" sidequest.
