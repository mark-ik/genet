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

## Browser-render smoke (added 2026-07-04)

Driven from the Woodshed cross-platform question ("would Woodshed-on-xilem_serval
render in a browser?"), a spike now exercises the full engine-core chain on
`wasm32-unknown-unknown` at `examples/serval_web_smoke/` (standalone workspace,
netrender_smoke pattern):

- **Wasm-green on main, verified 2026-07-04:** `xilem-serval`,
  `serval-scripted-dom`, `serval-layout` all `cargo check` clean for wasm32 with
  zero errors — P1's result has landed and holds beyond serval-layout.
- **netrender manifest half done** (netrender commit `6520d74ed`): wgpu backend
  features are now per-target — native keeps dx12/metal/vulkan (identical
  resolved set), wasm32 selects `webgpu`; pollster moved native-only beside the
  blocking `boot()` wrappers. netrender + paint_list_render + netrender_text all
  check green for wasm32. This closes the "add the wgpu webgpu feature" item
  from the 2026-06-24 correction above.
- **Host-font seam added** (`serval_layout::register_host_font`): host-supplied
  TTF/OTF blobs register into every `TextMeasureCtx` under their own family
  names and append to the `sans-serif` generic — the "wasm fonts come from the
  host" hook. Harmless on native (fonts join system discovery).
- The smoke builds a woodshed-flavored view tree through `ServalAppRunner`,
  lays it out, emits a band, lowers via `translate_paint_cmd_stream`, and
  presents through `boot_async` + a WebGPU canvas surface. It mirrors the
  serval-workspace `[patch.crates-io]` entries (stylo/stylo_atoms git-rev,
  taffy + sonic-rs vendored) because patches are per-workspace-root.

**Receipt: PASS (2026-07-04).** The page renders in Chrome over WebGPU — pills
nav, sidebar, rounded panels, note/root dots, Roboto text — captured via CDP
screenshot. Two runtime walls fell on the way, both now fixed in the libraries:

1. **`std::time::Instant` panics on wasm** ("time not implemented"): the
   unconditional diagnostics timer in `lay_out_content` (serval-layout) and the
   frame-profiling / per-frame-diagnostic timers in netrender. Fixed with
   `web-time` (wasm-only dep; native untouched). The `SERVAL_LAYOUT_TIMING`
   probes are env-gated and dead on wasm, left as-is.
2. **Browser swapchains reject vello's storage-texture write**: the fine/compose
   stages bind the render target as an `RGBA8Unorm` storage texture, but a
   canvas swapchain view is RenderAttachment-only (and BGRA). Consumers must
   rasterize into an intermediate `STORAGE_BINDING | TEXTURE_BINDING` texture
   and blit (`wgpu::util::TextureBlitter`) — the meerkat shape, where scenes
   never see the swapchain. The smoke does this; any future web shell must too.

This was the first actual *execution* of serval-layout on wasm, not just a
compile. Remaining for a real web embedding: a resize/rAF loop, input
translation, and the smoke's `[patch]` mirror kept in sync.

## Forward direction + async granularity (added 2026-06-24, grand audit §6)

**Implementation update (2026-06-24):** the Nova/wasm64 spike described below is
now a first-class experimental lane, not a future option. The authoritative build,
worker, fallback, and promotion contract is
[`2026-06-24_nova_memory64_browser_lane_plan.md`](./2026-06-24_nova_memory64_browser_lane_plan.md).

The grand audit (`2026-06-24_grand_audit.md` §6) re-grounded the wasm question and
found the async/event-loop architecture much further along than this plan tracked.
Two threads to fold into the roadmap here:

**Delivery direction (ranked).**

1. **Ship the reader-PWA wasm lane first.** serval-layout is wasm-green (P1) and
   netfetcher binds the browser fetch, so a DOM-only / structured-HTML / smolweb
   reader with a tiny omit-JS bundle is the strongest near-term case. Treat the
   Boa-scripted wasm tier as the next milestone, not the v1 bar.
2. **Finish netrender's wasm port (ordinary work, not a structural gate).** The
   `wasm-portability-checklist.md` is **stale** (corrected 2026-06-24): it
   describes the abandoned `wgpu-backend-0.68-minimal` WebRender/GL branch. On
   `main` the GL code is deleted (vello is the sole rasterizer, `README.md:7-15`),
   library src has zero threading and zero GL, boot is async-first with the
   `pollster::block_on` wrappers `cfg`-gated off wasm32
   (`netrender_device/src/core.rs:80-155`), and the device is embedder-supplied
   (`init.rs:59-64`). Remaining: add the wgpu `webgpu`/`webgl` feature (the
   manifest selects native backends only) and drive `boot_async` from a browser
   executor. Mark or archive the stale checklist.
3. **Orchestrate around Web Workers + postMessage**, not a ported thread pool;
   production wasm threading (nightly + build-std + SharedArrayBuffer + COOP/COEP)
   is an external constraint, consistent with the Isolation-backends direction
   above. Extension nuance: a Chrome MV3 extension gets COOP/COEP via *manifest*
   keys for extension-owned documents (not the MV3 service worker; use an
   offscreen document); Firefox extensions have no such path yet (bug 1673477).
4. **Boa-wasm32 is the portable baseline; Nova-on-wasm64 is the implemented
   experimental Chrome/Firefox mode** *(corrected 2026-06-24; the earlier "Nova native-only /
   don't bet on memory64 timing" framing was wrong).* Nova's only pointer-width
   coupling is `value.rs:398`'s `size_of::<Value>() == size_of::<usize>()` (Value
   is 8 bytes; passes on 64-bit usize) plus usize-typed `1 << 53` const-overflows;
   all dissolve on wasm64. Memory64 is default-on in Chrome/Edge 133 (2025-02-04)
   and Firefox 134 (2025-01-07), and the toolchain landed in 2026 Q2 (wasm-bindgen
   0.2.120 added wasm64 via an f64 pointer ABI, wasm-pack 0.15.0, getrandom 0.4.3
   `wasm_js`). The `ScriptEngine` trait keeps Nova-on-wasm64 a swap. Honest
   implementation now gates USDT, supplies a single-worker atomics backend, and
   links the wasm64 worker with Tier-3 `build-std`. Remaining gates are browser CI,
   bundle/startup/memory measurement, and the known Memory64 performance cost.
   Safari/iOS stays on the Boa/wasm32 fallback.
5. **Worker isolation is the wasm threat model.** Boa has no preemption, so
   hostile remote JS cannot be bounded in-VM (`set_deadline` is not a trust
   boundary); the sandbox story must be Web-Worker isolation + an allow-listed
   capability set.

**Async granularity sub-thread (refine, do not rebuild).** `script-runtime-api`
already models the WHATWG event loop on engine-neutral primitives (microtask
checkpoint, timer task source over a virtual clock, capture/target/bubble
dispatch, and a deferred-async fetch seam `new_host_promise`/`settle_host_promise`),
tested on both Boa and Nova. The honest gaps are granularity, not architecture:
the loop is cooperative (delays order tasks, they do not truly wait), microtask
checkpoints are coarse (around timer batches, not per-task), and ReadableStream is
buffered (no BYOB, no truly-async producers). Remaining work: per-task microtask
checkpoints, true async timers/streams, BYOB readers, broader task sources, and
wiring serval's own WPT runner to a real off-thread fetch source the way meerkat
already does (`repos/mere/crates/meerkat/src/fetch.rs:145-195`).

**Agent-cluster as the Web-Worker / Atomics boundary (added 2026-06-24, from the gterzian/formal-web harvest, `docs/2026-06-24_formal_web_lessons.md`).** formal-web models the spec's `Agent { id, can_block, event_loop_id }` / `AgentCluster` as plain data, decoupled from its OS process. That concept is exactly the isolation primitive this plan's Web-Worker direction needs: adopt **agent-cluster as the SharedArrayBuffer / Atomics boundary**, and bind `EventLoopId` to an armillary actor on native and to a **Web Worker** on wasm (not an OS process). Carry the spec's **`can_block`** flag — a worker agent may block on `Atomics.wait`, a window agent may not — so the same abstraction enforces the rule on both backends. This is the agent-level refinement of the "Web Worker is the browser-relevant isolation backend" point above: the transport-neutral boundary moves *agent clusters*, and `can_block` is per-agent metadata it carries. The deferred BYOB-reader / true-async-stream work is spun out to serval's `2026-06-24_byob_streams_plan.md`; the per-task microtask-checkpoint tightening + scheduler trace validation to `2026-06-24_event_loop_rigor_plan.md`.
  Next: P2 = orrery-host to wasm (the orrery's serval-layout-using path), which will
  surface orrery-host's own deps.
