# serval profile ladder plan

**Status (2026-05-17):** This doc remains canonical for the *strategic* framing
of the profile ladder (static-html / interactive-html / scripted / fullweb tier
crates inside Serval). The *implementation* direction has gone through three
reframings since 2026-05-12:

1. **2026-05-15 audit** moved `components/layout/` and `components/script/`
   to dead-on-disk; `serval-layout` becomes a new crate.
2. **2026-05-16 lift plan** ([2026-05-16_serval_layout_lift_plan.md](./2026-05-16_serval_layout_lift_plan.md))
   proposed path C (lift portable layout into `serval-layout`).
3. **2026-05-17 planes architecture** ([2026-05-17_serval_layout_planes_architecture.md](./2026-05-17_serval_layout_planes_architecture.md))
   is the **current authoritative target shape**: Stylo + Taffy + parley +
   selected lifted display-list machinery, with `serval-layout`-owned planes
   (Style / Layout / Fragment) keyed by `D::NodeId` for mutable rendering
   state. Cross-engine architecture in
   [2026-05-17_hekate_lanes_observables.md](./2026-05-17_hekate_lanes_observables.md);
   renderer-input vocabulary in
   [2026-05-17_paintlist_polyglot_renderer.md](./2026-05-17_paintlist_polyglot_renderer.md).

Workspace state: [2026-05-16_workspace_audit_snapshot.md](./2026-05-16_workspace_audit_snapshot.md).

Sections below describe the original P0–P7 phases; their *names* still apply,
but the *mechanics* of P1/P2/P3 are reframed in the planes architecture (not
the lift plan as originally proposed). The P1 fallout addendum at the bottom
is **historical** — the affected adapter
(`ScriptLayoutHostServices` in `components/script/script_thread.rs`) is in
dead-on-disk code and not compiled.

**Reconciliation note (2026-05-21):** the JS-engine slot in the `serval-scripted`
tier is now **Nova** (primary, pure-Rust) with Boa as a conformance oracle — see
[script-engine plan Part 6](./2026-05-20_serval_script_engine_plan.md). This
re-axes one assumption below: **wasm is no longer a property of the *tier* — it's an
orthogonal build target.** P7 ("browser/wasm host profile") is therefore *not* a
low-profile lane; any capability tier can target wasm in principle. The capability
matrix and the `mozjs`-free dependency gates remain valid as written (low tiers carry
no JS engine), but their justification is **attack-surface + bundle-size +
DOM-as-library**, not wasm-safety. Caveat (verified 2026-05-21): Nova does not yet
compile to wasm32 *out of the box* — its unconditional `usdt` (DTrace) dependency
hard-errors off-x86_64/ARM64, and the `array-buffer`/`atomics` features pull
`ecmascript_atomics` (arch-specific asm); the wasm story needs a small upstream patch
+ feature trim. See the script-engine plan's Part 6 / Appendix B for specifics.

---

This plan spins the old C5 / C7 "script-optional" cuts out of
[archive/2026-05-05_serval_netrender_cut_plan.md](./archive/2026-05-05_serval_netrender_cut_plan.md)
(archived 2026-05-17) and reframes them as a profile ladder:

```text
serval-static-html
    html5ever/static document tree -> Servo style/layout -> paint_types
    -> servo-paint -> NetRender

serval-interactive-html
    static HTML plus forms/input/focus/accessibility, still no JS

serval-scripted
    interactive HTML plus script DOM bindings, JS, and script event routing

serval-fullweb
    scripted plus navigation orchestration, workers, storage, media,
    WebGPU/WebGL, devtools, and the broader browser surface
```

The goal is not to bolt Blitz onto a separate Nematic HTML lane. The
goal is to make Serval itself compile from a Blitz-like minimum shape up
to a full-web browser composition, with NetRender as the shared output
path.

Blitz remains useful prior art for the minimum shape: HTML parser, DOM
abstraction, style/layout, paint lowering, shell. It is not the
architecture to transplant.

---

## Relationship to Nematic and routing

This plan changes the earlier "Blitz as Nematic HTML lane" idea.

The revised routing rule is:

- **Nematic** stays the protocol-faithful direct-document engine for
  Gemini, Gopher, Scroll, Markdown, feeds, plain text, and other sources
  where source semantics matter more than browser compatibility.
- **Serval static HTML** handles static/simple HTML and CSS through the
  smallest Serval profile.
- **Serval fullweb** handles JS-heavy pages, browser APIs, workers,
  storage, media, WebGL/WebGPU, and hostile/complex web content.
- **Wry/WebView2** remains the system-webview fallback, not the target
  architecture.

That keeps the Graphshell engine router simple: pick a content engine by
capability need, then let Serval scale internally rather than creating a
separate Blitz-derived browser family.

---

## Why this replaces the old C5 / C7 framing

The old C5 / C7 framing was mechanically correct but too narrow:

- **C5**: remove `script` and `script_traits` from `components/layout`.
- **C7**: remove or gate `script` and `script_traits` from
  `components/servo`.

Those are still required, but they are symptoms. The real target is:

- layout can consume a document through a profile-neutral layout input
  contract;
- script is one provider of that contract, not the owner of layout;
- the all-up browser facade is one composition, not Serval's minimum
  identity;
- low profiles are proven by separate package graphs, not only by
  runtime `EngineProfile` branches.

So this plan treats C5 as **layout/input contract extraction** and C7 as
**facade/profile graph separation**.

---

## Current hard edges

These are the present script-coupled edges that block a Blitz-like
Serval profile:

- `components/layout/Cargo.toml` still depends on `script` because
  `servo-layout` is currently a script-DOM layout implementation, not
  yet a profile-neutral layout engine package.
- `components/layout/Cargo.toml` no longer depends directly on
  `script_traits`.
- `components/shared/layout/Cargo.toml` no longer depends on
  `script_traits`.
- `layout_api::LayoutConfig` no longer contains
  `script_chan: GenericSender<ScriptThreadMessage>`; it now takes
  `Arc<dyn LayoutHostServices>`.
- `ScriptThreadFactory` now lives in `script_traits`, not `layout_api`.
- `components/servo/Cargo.toml` still hard-depends on `script` and
  `script_traits`.
- `components/constellation` is typed around `ScriptThreadMessage`,
  which is fine for fullweb but too heavy for a static document pipeline.

The important observation: `ScriptingProfile` and the Pelt script-free
host lane already exist, but the compile graph still treats script as a
structural dependency below the profile decision.

---

## End-state package shape

Prefer facade/profile packages over only feature flags. Cargo features
unify, so a single accidental default feature can hide a hard dependency
that the low profile was supposed to avoid.

Target package graph:

```text
serval-static-html
    -> serval-static-dom
    -> servo-layout
    -> servo-paint
    -> netrender
    -> pelt-core or a tiny host adapter

serval-interactive-html
    -> serval-static-html
    -> focus/input/accessibility profile services

serval-scripted
    -> serval-interactive-html
    -> script_runtime_api
    -> script
    -> script_traits

serval-fullweb
    -> serval-scripted
    -> constellation
    -> net/storage/media/webgpu/webgl/devtools/etc.
```

The exact crate names can change during implementation. The contract is
that a low-profile package can be checked with no `script`,
`script_bindings`, `mozjs`, or old browser-service crates in its
dependency graph.

---

## Profile capability matrix

| Capability | static-html | interactive-html | scripted | fullweb |
| --- | --- | --- | --- | --- |
| HTML parse | yes | yes | yes | yes |
| CSS style/layout | yes | yes | yes | yes |
| Servo layout engine | yes | yes | yes | yes |
| NetRender output | yes | yes | yes | yes |
| Forms/focus/input | no/minimal | yes | yes | yes |
| Accessibility tree | minimal | yes | yes | yes |
| JS engine | no | no | yes | yes |
| Script DOM bindings | no | no | yes | yes |
| Navigation orchestration | file/string input only | simple loads | browser loads | full |
| Storage/workers/service workers | no | no | optional | yes |
| Media/WebRTC/Bluetooth/etc. | no | no | optional | profile-gated |
| WebGL/WebGPU canvases | no | optional later | optional | yes |
| Devtools/WebDriver | no | no | optional | yes |

The lower profiles are not a security downgrade of fullweb. They are a
different composition with fewer capabilities in the build graph.

---

## Implementation slices

### Implementation checkpoint - 2026-05-12

Completed:

- Added `components/serval-static-html` as the first profile witness
  package.
- Added `components/serval-static-dom` as the first script-free HTML5
  parser-backed document provider package.
- Added `support/profile-gates/check-static-html.ps1`.
- Added `LayoutHostServices` and `NoOpLayoutHostServices` to
  `layout_api`.
- Replaced layout's direct script callback channel with
  `LayoutHostServices::web_font_loaded`.
- Added a script-backed `ScriptLayoutHostServices` adapter in
  `components/script/script_thread.rs`.
- Moved `Painter`, `PaintWorkletError`, and
  `DrawAPaintImageResult` into `layout_api`, with `script_traits`
  re-exporting them for existing script callers.
- Moved `ScriptThreadFactory` from `layout_api` to `script_traits`.
- Removed direct `script_traits` dependencies from `servo-layout` and
  `servo-layout-api`.
- Wired `serval-static-html` to parse HTML into `StaticDocument`.

Receipts:

```powershell
cargo check -p serval-static-dom
cargo test -p serval-static-dom
cargo check -p serval-static-html
powershell -ExecutionPolicy Bypass -File support/profile-gates/check-static-html.ps1 -SkipCargoCheck
cargo check -p servo-layout-api
cargo check -p servo-script-traits
```

All six pass.

P2 remaining blocker:

`servo-layout` still depends on `script` through the concrete
`script::layout_dom::{ServoLayoutNode, ServoLayoutElement, ...}`
implementation used throughout layout. Removing that edge is not a
message-channel cleanup; it is the next real split: make the layout DOM
provider profile-neutral, then let the script DOM and static DOM both
implement that contract.

Current dependency evidence:

```powershell
cargo tree -p servo-layout -i servo-script --edges normal
cargo tree -p servo-layout -i servo-script-traits --edges normal
```

The direct edge is still `servo-layout -> servo-script`. The remaining
`servo-script-traits` path is now indirect through `servo-script`, not a
direct layout/layout_api dependency.

Next mechanical cut:

- Introduce a layout-provider construction seam that can convert a
  profile-owned document node handle into the relevant
  `LayoutDomTypeBundle`.
- Keep the current script DOM as one provider.
- Make `serval-static-dom` implement the provider as the second provider.
- Only then remove `script` from `servo-layout`; until layout stops naming
  `ServoLayoutNode` and `ServoDangerousStyleElement` directly, removing
  the dependency would be fake.

### P0 - Add dependency-gate scaffolding

Status: implemented for `serval-static-html`.

Add the profile ladder names and build-graph gates before moving large
code.

Work:

- Add placeholder package targets or documented `cargo check` aliases for
  `serval-static-html`, `serval-interactive-html`, `serval-scripted`, and
  `serval-fullweb`.
- Add a local script or documented commands that fail if the static
  profile pulls `script`, `script_bindings`, `mozjs`, `media`,
  `storage`, or full browser service crates.
- Keep the default browser/fullweb build untouched.

Initial gates:

```powershell
cargo check -p serval-static-html
cargo tree -p serval-static-html | rg "script|script_bindings|mozjs|servo-media|storage"
cargo check -p serval-fullweb
```

The middle command should produce no matches for the static profile.
Once automated, invert it so a match fails the check.

Done condition:

- The repo has named profile gates even if the first static package is a
  thin shell.
- The default browser/fullweb build still checks.

---

### P1 - Split layout host services from script messages

Status: **partially implemented and not yet compile-verified end-to-end**.
The script-backed `ScriptLayoutHostServices` adapter does not satisfy the
`Send + Sync` bound on the new `LayoutHostServices` trait. See
[2026-05-13 P1 fallout addendum](#p1-fallout-the-script-host-impl-is-not-sync)
below.

This is the real first half of old C5.

Work:

- Replace `LayoutConfig::script_chan: GenericSender<ScriptThreadMessage>`
  with a profile-neutral layout host service handle.
- Define a minimal `LayoutHostServices` or `LayoutEventSink` trait in
  `layout_api`.
- Move only the messages layout actually needs behind that trait:
  webfont loaded, image invalidation, paint metric, accessibility
  invalidation, scroll state updates, and any required embedder-facing
  notifications.
- Keep script-backed implementations in the script/fullweb layer.
- Provide a no-op or capture implementation for static HTML.

Non-goal:

- Do not extract the whole DOM yet.
- Do not make layout own navigation, storage, or browser lifecycle.

Done condition:

- `layout_api` can be compiled conceptually without knowing
  `ScriptThreadMessage`.
- Script-backed fullweb still implements the same notifications.

---

### P2 - Remove script from layout and layout_api

Status: partially implemented. `layout_api` and `servo-layout` are free
of direct `script_traits` dependencies, but `servo-layout` still depends
on `script` for the concrete script DOM provider.

This completes old C5.

Work:

- Remove `script = { workspace = true }` from
  `components/layout/Cargo.toml`.
- Remove `script_traits = { workspace = true }` from
  `components/layout/Cargo.toml` and
  `components/shared/layout/Cargo.toml`.
- Replace direct `use script::*` / `use script_traits::*` imports in
  layout with:
  - existing layout traits (`LayoutDom`, `LayoutNode`, `LayoutElement`);
  - new narrow layout API types;
  - profile-neutral host service traits from P1.
- Move `ScriptThreadFactory` out of `layout_api` into a script runtime
  crate or constellation-local script adapter.

Done condition:

```powershell
cargo check -p servo-layout
cargo tree -p servo-layout | rg "script|script_traits|script_bindings|mozjs"
```

The second command should produce no matches for the normal library
dependency graph.

---

### P3 - Add a static HTML document provider

Status: started. `serval-static-dom` now parses HTML through html5ever
into a script-free `StaticDocument`; it does not yet implement the
layout input traits.

This is where Serval becomes Blitz-like without using Blitz as the
runtime architecture.

Work:

- Add a `serval-static-dom` package or module.
- Parse HTML with `html5ever`.
- Build a lightweight immutable or mostly-immutable document tree that
  implements the layout input traits needed by `servo-layout`.
- Support the first narrow element/text/style/attribute surface required
  for static HTML layout.
- Feed the result into `servo-layout`, then `paint_types`,
  `servo-paint`, and NetRender.
- Keep this provider independent of JS reflectors, GC, WebIDL bindings,
  and script DOM mutation.

First receipt:

```text
HTML string/file
    -> static document tree
    -> Servo style/layout
    -> Serval display list / paint_types
    -> servo-paint
    -> NetRender readback
```

Done condition:

- `cargo check -p serval-static-html` succeeds without `script` or
  `mozjs`.
- A smoke renders simple HTML/CSS text/box content through NetRender.

Pitfall:

- Do not start by extracting Servo's full DOM. That DOM is
  JS/GC/reflector-shaped. Extract the layout input contract first, then
  make static DOM one provider of it.

---

### P4 - Move script runtime spawning out of layout/facade internals

This revises C6 so it serves the profile ladder rather than only the
existing browser facade.

Work:

- Keep `ScriptingProfile`, but extend the concept from runtime branch to
  compile profile:
  - `None` / `StaticHtml`;
  - `InteractiveHtml`;
  - `Scripted`;
  - `FullWeb`.
- Move `ScriptThreadFactory` and service-worker factory selection into a
  script runtime adapter layer.
- Keep the existing browser behavior as the `FullWeb` implementation.
- Keep no-op script/service-worker factories available for low profiles,
  but do not let those no-op factories mask a hard dependency on
  `script`.

Done condition:

- Fullweb still spawns real script.
- Static/intermediate profiles do not instantiate script and do not need
  `script` in the package graph.

---

### P5 - Split the servo facade into profile packages

This is the real C7.

Work:

- Stop treating `components/servo` as the only public composition.
- Choose one of two shapes:
  - **Preferred implementation shape:** create profile facade packages
    (`serval-static-html`, `serval-fullweb`, later
    `serval-interactive-html` / `serval-scripted`) and let
    `components/servo` become or remain the fullweb facade until the
    rename is worth doing.
  - **Fallback shape:** cfg-gate `components/servo` internally. This is
    faster but easier to pollute with accidental script deps.
- Move or gate script-only files such as JavaScript evaluation, devtools
  attach, script-backed WebView methods, and service-worker plumbing
  behind the scripted/fullweb facade.

Done condition:

```powershell
cargo check -p serval-static-html
cargo check -p servo
cargo check -p pelt
```

The static profile must not pull `script`, `script_bindings`, or `mozjs`.
The default fullweb/browser profile must keep existing behavior.

---

### P6 - Split low-profile document pipeline from full constellation

The current `components/constellation` is fullweb-shaped and heavily
typed around `ScriptThreadMessage`. That is not wrong; it is just the
wrong minimum profile.

Work:

- Let `serval-static-html` use a direct document pipeline at first:
  parse -> layout -> paint -> NetRender.
- Do not force static HTML through the full constellation just to reuse
  lifecycle code.
- After the static pipeline works, extract a shared `pipeline-core` only
  if both static and fullweb truly need the same routing/lifecycle logic.

Done condition:

- Static HTML can render without `components/constellation`.
- Fullweb still uses constellation.
- Any shared pipeline core is extracted from proven overlap, not guessed
  in advance.

---

### P7 - Browser/wasm host profile, later

The browser-wasm host is a later proof lane, not a prerequisite for
splitting the Serval profiles.

Expected shape:

- `pelt-web`, not `pelt-desktop`;
- `wasm-bindgen` browser canvas binding;
- async WebGPU device acquisition;
- externally supplied `WgpuHandles`;
- no `pollster::block_on` startup;
- no native-only wgpu backends.

First proof gate:

```text
browser canvas renders the same 64x64 NetRender smoke scene
without pelt-desktop, pollster boot, or native-only backends
```

---

## C5 / C7 mapping

| Old cut | New slices | Meaning |
| --- | --- | --- |
| C5: cut script dep from layout | P1, P2, P3 | layout contract extraction, script-free layout graph, static document provider |
| C6: route script creation through profile factory | P4 | runtime selector remains useful, but must not hide compile deps |
| C7: cut script dep from servo facade | P5, P6 | profile facade packages plus low-profile pipeline split |

C5 and C7 should remain labels in the old netrender cut plan for
continuity, but this document is the canonical implementation plan for
those cuts.

---

## Validation ladder

Minimum recurring checks:

```powershell
cargo check -p servo-layout
cargo check -p servo-paint
cargo test -p servo-paint --test paint_render_e2e
cargo check -p serval-static-html
cargo check -p servo
```

Dependency absence checks:

```powershell
cargo tree -p servo-layout | rg "script|script_traits|script_bindings|mozjs"
cargo tree -p serval-static-html | rg "script|script_traits|script_bindings|mozjs"
```

Those should produce no matches for the target low-profile graphs. Once
the commands stabilize, wrap them in a small script that exits nonzero on
matches.

Fullweb regression checks:

```powershell
cargo check -p servo
cargo check -p pelt
cargo test -p servo-paint --test paint_render_e2e
```

Presentation checks stay in the compositor/C4 lane:

```powershell
cargo run -p pelt --features windows-present -- --engine viewer --windows-present-surfaces-smoke about:blank
cargo run -p pelt --features macos-present -- --engine viewer --macos-present-surfaces-smoke about:blank
```

---

## Pitfalls

- **Cargo feature unification.** A low profile hidden behind
  `--no-default-features` is not enough if another crate re-enables
  browser defaults. Use profile packages as dependency witnesses.
- **No-op script threads that still compile script.** Runtime no-ops are
  useful only after the package graph is clean.
- **Extracting Servo DOM first.** The full DOM is script-shaped. Start
  with the layout input contract and make a static tree implement it.
- **Forcing static HTML through constellation.** The low profile should
  not inherit fullweb lifecycle machinery before it needs it.
- **Letting NetRender split.** Static HTML and fullweb should share the
  same paint/NetRender output path.
- **Treating static HTML as a universal fallback.** Full web apps, JS,
  browser APIs, workers, and complex origin behavior still belong to the
  scripted/fullweb profiles.

---

## P1 fallout: the script host impl is not Sync

Discovered 2026-05-13 while preparing P2 by clearing working-tree merge
conflicts and running `cargo check -p servo-layout` end-to-end for the
first time.

### Symptom

```text
error[E0277]: `RefCell<Option<Vec<u8>>>` cannot be shared between threads safely
  --> components/script/script_thread.rs:229:29
   |
229 | impl LayoutHostServices for ScriptLayoutHostServices {
   |                              ^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: required because it appears within (NamespaceIndex<BlobIndex>, BlobImpl)
           required by a bound in `LayoutHostServices`
   --> components/shared/layout/lib.rs:245:38
   |
245 | pub trait LayoutHostServices: Send + Sync {
```

### Sync chain

1. `components/shared/fonts/lib.rs:121` defines
   `StylesheetWebFontLoadFinishedCallback = Arc<dyn Fn(bool) + Send + Sync>`.
   The fonts subsystem requires `Sync` on the callback because the
   stylesheet callback can fire on multiple font-loader threads.
2. `components/layout/layout_impl.rs:831` constructs that callback by
   capturing `Arc::clone(&self.host_services)`. For the resulting closure
   to satisfy `Send + Sync`, the captured `Arc<dyn LayoutHostServices>`
   must be `Sync`, which means `dyn LayoutHostServices: Send + Sync`.
3. `ScriptLayoutHostServices` holds
   `GenericSender<ScriptThreadMessage>`. `ScriptThreadMessage` includes
   variants that carry `BlobImpl::File(FileBlob)`, and `FileBlob.cache`
   is `RefCell<Option<Vec<u8>>>` — not `Sync`.
4. The error therefore fires on `impl LayoutHostServices for
   ScriptLayoutHostServices`.

### Why this slipped past P1

- P1 added the `Send + Sync` bound on `LayoutHostServices`.
- The only validating gate that exercises the trait is
  `NoOpLayoutHostServices` (trivially `Sync`), which `serval-static-html`
  uses.
- Broad checks (`cargo check -p servo-layout`) were blocked by
  working-tree merge conflict markers in `components/script/dom/*`
  inherited from an in-progress upstream merge.
- P1 is **uncommitted** — `git log -S 'LayoutHostServices'` returns
  nothing. The bound was never compile-verified against the script-backed
  impl.

### Fix options (architectural decision needed)

1. **Cache field**: change `FileBlob.cache` from `RefCell<Option<Vec<u8>>>`
   to `Mutex<Option<Vec<u8>>>` with `#[serde(skip)]`. ~5-line edit in
   `components/shared/constellation/structured_data/serializable.rs`.
   Local, but workaround-shaped — any other non-Sync type elsewhere in
   `ScriptThreadMessage`'s variant tree could reintroduce the same
   problem.
2. **Sync-clean bridge sender**: have `ScriptLayoutHostServices` hold an
   `IpcSender<PipelineId>` (or a `crossbeam` channel with a `PipelineId`
   payload) instead of the full `ScriptThreadMessage` sender. A small
   router on the script side maps incoming `PipelineId`s back into
   `ScriptThreadMessage::WebFontLoaded`. Servo already uses
   `Router::add_route` for similar single-thread message conversions.
3. **Relax the font callback contract**: change
   `StylesheetWebFontLoadFinishedCallback` from
   `Arc<dyn Fn(bool) + Send + Sync>` to `Arc<Mutex<dyn FnMut(bool) +
   Send>>` (or a `Box<dyn FnOnce>` if only one fire is needed). Removes
   the `Sync` requirement at the source. Larger blast radius — every
   caller of the type changes.
4. **Polling instead of callbacks**: layout polls the font subsystem for
   completion, no callback into `host_services`. Largest refactor.

Recommended: option 2. The fonts subsystem's `Sync` requirement is
legitimate; the script-side workaround is small and consistent with
existing Servo IPC routing patterns. Option 1 is faster but trades
correctness for a workaround at a single field — fragile if other
non-Sync types appear in the message tree.

### Until P1 is finished

`cargo check -p servo-layout` is blocked. The meaningful validation
surface is:

```powershell
cargo check -p serval-static-html
cargo check -p serval-static-dom
cargo check -p servo-layout-api
powershell -ExecutionPolicy Bypass -File support/profile-gates/check-static-html.ps1 -SkipCargoCheck
```

P2 Step 1 (layout-provider re-export) is mechanically complete and
visible in the working tree, but its full validation depends on P1's
Sync fallout being resolved.
