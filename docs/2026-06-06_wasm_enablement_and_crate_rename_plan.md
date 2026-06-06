# serval wasm enablement: de-IPC + crate rename (de-servo-fication)

Two intertwined efforts, planned together because they touch the same ~33 crates
and would otherwise double the churn:

1. **De-IPC for wasm.** serval-layout (and eventually the serval engine) cannot
   build for `wasm32-unknown-unknown` because the inherited Servo foundation drags
   multiprocess machinery (`ipc-channel`, `IpcSharedMemory`, `generic_channel`,
   `tikv-jemalloc-sys`) plus a wasm-hostile randomness dep (`uuid` v4). None of it
   runs in serval's single-process model; on wasm it does not even compile.
2. **De-servo-fication (rename).** 33 crates still carry the `servo-*` package
   prefix. They are serval's now (cut, slimmed, or about to be). The prefix is
   fork residue. Rename to a serval-owned namespace, **except** where a crate is a
   thin fork tracking an upstream Servo/Mozilla crate (lineage matters there).

The de-IPC pass touches every foundational crate's Cargo.toml + code; the rename
touches every package name + every consumer's `use`. They are one coordinated
pass, not two.

## Why this is the actor-constellation thesis, one layer down

Related: the [actor constellation plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-06-03_actor_constellation_plan.md).
That plan is "Servo's constellation done **in-process**: scenes travel as messages
rather than IPC-serialized surfaces." Servo's `ipc-channel` / `IpcSharedMemory` /
`generic_channel` is message-passing between **processes**; the constellation is
message-passing between in-process **actors**. So removing Servo's IPC from the
foundation is not wasm hackery, it is finishing the architecture the constellation
already chose: a `pixels` `IpcSharedMemory` buffer becomes the `Scene`'s owned CPU
pixel data a content actor moves to the kernel.

The constraint that plan puts on **how** we de-IPC, corrected 2026-06-06 (with the
other agent; the earlier "preserve the OS-multiprocess toggle" framing overvalued
Servo's IPC):

- **Preserve the transport-neutral actor boundary, not Servo's IPC.** The reusable
  asset is armillary's actor discipline, not `ipc-channel` — and it is not yet a
  cross-process toggle. armillary requires only `Send` command/update types, not
  wire-safe `Serialize` ones (`crates/armillary/src/actor.rs`), and meerkat's
  content messages are plain Rust enums, not serde DTOs
  (`crates/meerkat/src/content.rs`). Servo's own `generic_channel` (IPC vs
  crossbeam, switched by Servo runtime opts) and `GenericSharedMemory`
  (`IpcSharedMemory` vs `Arc<Vec<u8>>`) are Servo plumbing, not an armillary
  backend. So gating Servo's IPC native-only is a **low-risk wasm enabler**, not
  the preservation of a working toggle; the gated native code is
  dead-pending-removal.
- **The OS-multiprocess "toggle" is a native-only, deferred special case**, not a
  feature being protected. The browser/PWA target (primary) cannot spawn OS
  processes / `fork` / native supervisors at all — but it *can* run multiple wasm
  instances in Web Workers, each with its own linear memory, talked to by
  `postMessage`. So the browser-relevant isolation backend is the **Web Worker**,
  on armillary's boundary, owing nothing to `ipc-channel`. The semi-trusted threat
  model also defers the one reason for process-level confidentiality. See
  "Isolation backends" below.

## Isolation backends: the transport-neutral boundary

The cross-context capability lives in armillary's message boundary, not Servo's
IPC. The follow-up (armillary work, tracked in the actor constellation plan) is a
transport-neutral boundary where *selected* command/update DTOs are
`Send + Serialize + Deserialize`, with pluggable backends:

- **In-process `mpsc`** (today): `Send`-only messages, no serialization, threads.
- **Web Worker `postMessage`** (next, the browser/PWA backend): serialized DTOs, a
  separate wasm instance + linear memory. The browser's actor-isolation primitive,
  and it degrades to an in-process thread on native (same abstraction).
- **Native subprocess** (later, only if the threat model crosses into genuinely
  untrusted content): a serializing transport, re-importing an IPC layer *then* —
  not by keeping Servo's alive now.

Trajectory: **threads now, Web Workers next for PWA/browser, OS processes only if
untrusted.** Keep the seam serializable; do not keep Servo's IPC museum alive for
its own sake. The serval de-IPC pass below simply stops pretending the gated
native IPC preserves a toggle.

## Findings (verified 2026-06-06, isolated worktree `wasm-fonts-cut`)

- **serval-layout reaches the runtime only through `servo-layout-api`** (two edges:
  via `servo-fonts`, and direct). Both are **vestigial**: serval-layout's text path
  is parley/fontique, and its source never references `layout_api`, `base`,
  `fonts`, `net_traits`, `embedder_traits`, `paint_api`, or `paint_types`. Removing
  the `layout_api` + direct `servo-base` deps compiles clean (native) and kills the
  entire `servo-net-traits → servo-fonts → hyper → mio` chain.
- **After that cut, the wasm blockers, in build order:**
  1. `uuid` v4 needs a wasm randomness backend (`js` / `rng-getrandom`). Pulled
     pervasively (accesskit, malloc-size-of, paint-types, base). Fix: enable on
     wasm, cfg-gated, per the existing orrery-host / gyre pattern.
  2. `tikv-jemalloc-sys` (C jemalloc) via `servo-base → servo-allocator`. Does not
     build for wasm.
  3. Then `ipc-channel` via `servo-base` (Cargo.toml L21, unconditional) +
     `servo-malloc-size-of` (`IpcSharedMemory` `MallocSizeOf` impls) + `servo-pixels`
     (`base::generic_channel`).
- **These foundational crates are NOT cruft.** Stylo *requires* `malloc_size_of`;
  serval-layout uses `pixels`. They are genuine substrate shot through with Servo's
  multiprocess machinery, which is dead on single-process wasm. (Contrast the
  vestigial `layout_api`, which serval simply never used.)

## The approach decision (per crate, do not presume)

For each crate the de-IPC touches, choose deliberately: **gate** (cfg/feature the
IPC code native-only, keep the crate), **slim** (rip the IPC machinery out as a
serval cut), or **drop** (serval-layout should not pull it at all).

| Crate | Native-only machinery | Proposed disposition | Note |
|---|---|---|---|
| `servo-layout-api` | net_traits, fonts | **drop from serval-layout** (done) | vestigial; serval uses parley |
| `servo-allocator` | jemalloc `#[global_allocator]` | **drop from the lib chain** | a library must not set the global allocator; that is the final binary's call. Find why `servo-base` pulls it and sever |
| `servo-base` | ipc-channel (L21), generic_channel | **gate** (`multiprocess` feature / cfg(not wasm)) | the IPC foundation; preserve for the native multiprocess toggle |
| `servo-malloc-size-of` | `IpcSharedMemory` impls, ipc_channel | **gate** the IpcSharedMemory `MallocSizeOf` impl native-only | Stylo-required; keep, slim the IPC impl |
| `servo-pixels` | `base::generic_channel` | **gate** the shared-memory path native-only | wasm uses owned buffers (the constellation Scene path) |

Recommended default: **gate**, honoring "do not foreclose multiprocess." **Drop**
only where the dep is wrong-for-a-lib (allocator) or vestigial (layout-api).
**Slim** (hard-remove) only once the native multiprocess toggle is confirmed
unwanted.

## The rename (de-servo-fication)

33 `servo-*` package names. The rename is mechanical but wide: package name + every
consumer Cargo.toml + every `use base::` / `net_traits::` / etc. in code.

**Settle this before the mechanical pass (do not presume one namespace):** the
crates are not uniform.
- **serval-owned cuts** (`servo-base`, `servo-net-traits`, `servo-layout-api`,
  `servo-fonts`, `servo-embedder-traits`, `servo-paint*`, ...): serval has cut or
  slimmed these. Rename to a serval namespace.
- **Thin forks tracking upstream Servo/Mozilla crates** (`servo-malloc-size-of`,
  `servo_arc`, `servo-url`, `servo-geometry`): these mirror published crates;
  renaming complicates pulling upstream fixes. Keep the name (lineage) unless
  serval has diverged enough to own them outright.

Open: the target namespace (`serval-*`? folded into existing serval names?) and the
per-crate keep-vs-rename call.

## Coordination risk (load-bearing)

serval is under active concurrent work (webgl-wgpu, the deferred fetch-seam). A
mass crate rename rewrites nearly every Cargo.toml + many `use` lines across the
repo and would collide catastrophically with any in-flight branch. **The rename
must be a coordinated, serialized event** (a quiet tree, everyone rebased), an
atomic pass of its own. The de-IPC gating is more localized (the foundational
crates) and can land incrementally ahead of it.

## Phases (done-conditions, not dates)

- **P0 (done): archaeology + vestigial cut.** Worktree proves serval-layout → the
  runtime is via vestigial `servo-layout-api`; removing it + the direct `servo-base`
  dep kills net/fonts/hyper/mio, native-green. uuid + jemalloc + ipc blockers
  mapped.
- **P1: de-IPC serval-layout to wasm. DONE 2026-06-06.**
  `cargo build -p serval-layout --target wasm32-unknown-unknown --lib` succeeds
  (exit 0) and `cargo build -p serval-layout` (native) is unchanged. The build-loop
  found the real blockers, far narrower than the dependency archaeology implied: the
  IPC machinery (`ipc-channel`, `tokio`, `generic_channel`) **compiles for wasm
  unchanged and never needed gating**. The actual gates, all `cfg(not(target_arch =
  "wasm32"))` (gate-not-delete; native untouched):
  1. `uuid` `js` feature for wasm randomness (workspace dep; final form should
     cfg-gate it to wasm per orrery-host/gyre rather than enable unconditionally).
  2. `servo-malloc-size-of`: its `servo-allocator` (jemalloc) dep made native-only.
     The API was unused (MallocSizeOf is allocator-agnostic via stylo's
     `MallocSizeOfOps`), so a dep-only gate, no code change.
  3. `servo-url`: `to_file_path` / `from_file_path` gated to filesystem targets
     (mirroring the `url` crate's own gate).
  4. `servo-paint-types`: a unit-placeholder `NativeFontHandle` for non-native
     targets (wasm fonts come from the host).
  5. `servo-base`: a wasm arm for `cross_process_instant`'s platform clock.
  6. `serval-layout` `host_loader`: the local-file resolution (`from_directory_path`
     / `to_file_path`) gated to filesystem targets; wasm resources come from the
     host loader.
  Lesson: the runtime build-loop beat the static dependency analysis. The "fat IPC
  machinery" fear overstated it; the real wall was jemalloc (a C build) + uuid
  randomness + a handful of native-API code spots.
- **P2: the wasm tail.** Rebuild surfaces the next tier (accesskit, Stylo
  internals, font/system). Gate or replace until serval-layout is wasm-green, then
  the orrery's serval-layout-using path.
- **P3: the rename (coordinated).** De-servo-fy package names per the settled
  namespace decision, in one serialized pass when the serval tree is quiet.
  *Done:* no `servo-*` package names remain except deliberate upstream-tracking
  forks.

## Decisions (resolved 2026-06-06, Mark)

- **Scope: orrery-first.** P1/P2 target serval-layout to wasm (enough to run the
  orrery in a browser). The full serval engine to wasm is a later deliberate pass.
- **Disposition: gate native-only (a low-risk wasm enabler, not toggle-preservation).**
  `cfg(not(target_arch = "wasm32"))` for every IPC/jemalloc dep: the cheap way to
  get wasm building without disturbing native. The gated native code is
  dead-pending-removal; the real cross-context future is the transport-neutral
  armillary boundary + Web Workers (see "Isolation backends"). Slimming is the
  eventual end-state once the wasm pass is green and nothing native exercises the
  IPC path.
- **Rename: serval-owned cuts only.** De-servo the crates serval has cut/slimmed;
  keep the thin upstream-tracking fork names (`malloc_size_of`, `servo_arc`, `url`,
  `geometry`) for lineage.

## Open questions (still need a call)

- **Rename namespace:** the serval-owned prefix that replaces `servo-` (e.g.
  `serval-*`, or folded into existing serval names). Mark's call before P3.
- **Timing** of the rename (P3) vs the other agent's in-flight serval work: a
  serialized, coordinated event on a quiet tree.

## Progress

- **2026-06-06.** Wrote this plan. Worktree `wasm-fonts-cut` (branch off HEAD,
  isolated from the other agent's tree) holds the P0 state: the tier-2 vestigial
  cut applied (`layout_api` + direct `servo-base` removed from serval-layout,
  native build green), uuid `js` feature added experimentally, and the
  jemalloc / ipc-channel blockers mapped to `servo-base` / `servo-allocator` /
  `malloc-size-of` / `pixels`. No foundational gating and no rename started:
  both are gated on the disposition + namespace decisions above.
- **2026-06-06.** Decisions resolved (Mark): scope orrery-first (serval-layout to
  wasm), disposition gate-native-only (preserve the multiprocess toggle), rename
  serval-owned cuts only. P1 (gate the foundational crates native-only) begins;
  rename namespace + timing remain open.
- **2026-06-06.** Reframed the disposition rationale (Mark + the other agent
  concur): gating Servo IPC native-only is a low-risk wasm enabler, NOT preservation
  of a multiprocess toggle. The toggle is not real today: armillary requires only
  `Send` (not `Serialize`) messages, meerkat content messages are plain enums, and
  Servo's `generic_channel` / `GenericSharedMemory` are Servo plumbing, not an
  armillary backend. Added the "Isolation backends" direction (transport-neutral
  armillary boundary; mpsc / Web Worker postMessage / later native-subprocess;
  threads -> Web Workers -> OS-processes-only-if-untrusted), tracked in the actor
  constellation plan. Corrected the browser framing: the browser runs many wasm
  instances in Web Workers (separate memories); the real constraint is no OS
  processes / fork.
- **2026-06-06. P1 DONE.** serval-layout builds for `wasm32-unknown-unknown`
  (exit 0); native unchanged. Six `cfg(not(wasm32))` gates (uuid js / malloc_size_of
  servo-allocator / servo-url file-paths / paint-types NativeFontHandle / servo-base
  cross-process clock / serval-layout host_loader file-paths) plus the P0 vestigial
  cut. `ipc-channel` / `tokio` compiled for wasm unchanged, so no IPC gating was
  needed at all: the real blockers were jemalloc + uuid + native-API code spots, not
  the IPC substrate. The build-loop (runtime verification) beat the dependency-graph
  analysis. Worktree `wasm-fonts-cut` holds it, uncommitted; the uuid gate is still
  the unconditional-`js` experiment (cfg-gate it to wasm for the landable form).
  Next: P2 = orrery-host to wasm (the orrery's serval-layout-using path), which will
  surface orrery-host's own deps.
