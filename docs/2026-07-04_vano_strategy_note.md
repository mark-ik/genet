# Vano strategy note

**Date**: 2026-07-04.
**Scope**: the strategic frame for **vano**, Mark's fork of trynova/nova (the
data-oriented JS engine), consumed by `script-engine-nova` as
`nova_vm = { git = "mark-ik/nova", branch = "genet-embedder" }` (the repo now
answers as `mark-ik/vano`; local override at `Code/crates/nova`). Until now
this lived only in Cargo.toml comments and conversation.

## Posture

Willing to diverge from upstream; currently tracking it closely. Both halves
are deliberate. Consequences:

- The internal crate stays `nova_vm` while tracking. A crate rename is a
  permanent merge conflict against upstream; pay that cost when real
  divergence is chosen, not before.
- The branch keeps two piles separated: spec-correctness fixes
  (Promise.all/race observable `.then`, IteratorClose on abrupt, the
  stack-pointer recursion guard) are upstreamable diff hygiene, Mark's call
  whether to PR them; embedder work (snapshot clone, waitAsync timeouts, wasm
  enablement) is the fork identity.
- Fork-boundary discipline mirrors the wgpu sibling repos: genet-specific
  behavior lives in `script-engine-nova` (the adapter), never in vano; vano's
  embedder API stays generic.
- Cheap insurance to add: a test262 subset run against the genet-embedder
  branch on some cadence, so a rebase that regresses spec semantics is caught
  by the suite rather than by a hung page. (The Promise.race fix was an
  infinite-loop hang; that class of bug wants conformance coverage.)

## Engine map (settled 2026-07-04)

Gating is by **pointer width**, exactly as `script-engine-nova`'s Cargo.toml
already does:

- **vano**: 64-bit native + wasm64 (Memory64).
- **boa**: every 32-bit target, including wasm32.

This is the durable production shape, not a transition. Nova's `Value` is
word-size-asserted (`value.rs:398`, "must never be removed") with 7-byte
payloads (`SmallInteger` is `[u8; 7]`), so `Value` is 8 bytes everywhere and
wasm32 (4-byte usize) fails the assert at compile time. The alternatives
(24-bit handles, or a two-word `Value`) are poor trades while boa covers
32-bit.

Browser reality for the wasm64 lane (checked 2026-07-04): Chrome 133 and
Firefox 134 shipped Memory64 in early 2025. **Safari/WebKit has not**, through
at least the Safari 27 beta (WWDC26), with nothing announced. Since every iOS
browser is WebKit, boa is the durable primary engine for the whole WebKit
family, not a fallback. The genet-embedder wasm commit's substance is wasm64
support plus wasm-family platform glue (probe gating, RNG backends,
single-threaded atomics, Temporal/SAB feature gates).

Silver lining on the boa lane: Safari 27 adds JSPI, so suspending wasm for
async host calls converges across all three browser families.

## Snapshotting and dormancy (scoped 2026-07-04)

The stack's serialization house style is: never persist the live in-memory
representation; define a plain-data mirror and reconstruct via full snapshot
(only for already-flat data, e.g. Scene) or delta-log replay onto empty
(graph kernel, DOM). That pattern does not extend to Nova's heap, and the
reason is categorical: Scene/graph/DOM hold data; the JS heap holds
executable state (closures over live environments, builtin fn pointers,
pending jobs). There is no portable mirror type for a closure, and replaying
"the same script" is not resuming the same live state (Promise chains,
WeakMap identity). This is the same wall V8/SpiderMonkey hit; real browsers
discard the process and reload rather than serialize tab heaps.

The irreducible part is specifically **host entanglement** (embedder objects,
pending jobs, external references into the host world). Bytecode is data and
serializes in principle; builtin fn pointers could be made
intrinsics-table-relative. That nuance is what makes the checkpoint idea
below honest.

**The dormancy ladder** (three tiers, different honesty contracts):

1. **Live**: the content actor runs.
2. **Heap-clone suspend** (same process): `snapshot_clone`. Exact resume;
   Promise chains and identity intact. Nova's index-shaped heap makes this a
   handful of `Vec` memcpys, so unlike V8-family engines the warm-suspended
   tier is cheap enough to be the *default* dormancy state; many
   suspended-but-warm pages in one process is affordable. This is the fork's
   differentiator here.
3. **Discarded** (survives restart): persist the plain-data session mirror
   the stack already owns pieces of (visual snapshot data-URI, DOM-as-HTML
   snapshot per the capture/replay plan, native session store, DocumentScript
   session overrides), drop the heap, and **re-execute** on thaw. Per the
   no-placebo rule the UI must present tier-3 thaw as a reload, never as a
   resume.

Tier 3 fires under real memory pressure or restart; tier 2 covers everything
else.

**Shelf item, not a plan: controlled-checkpoint startup snapshots.** V8 and
SpiderMonkey serialize heaps only at a checkpoint before user code runs; the
WPT harness's `snapshot_clone` right after testharness.js loads is the same
move. Two graded uses, in order of reach:

- In-process realm stamping: clone one warmed realm (intrinsics + prelude
  initialized) per tile instead of cold-booting each realm. Needs no
  serialization at all.
- Cross-restart startup snapshot: serialize *that checkpoint*, where no
  closures over user state exist yet. The only heap-serialization project
  with bounded scope. Keep on the shelf until spin-up time measurably hurts.

## Cross-refs

- `script-engine-nova/Cargo.toml` (pointer-width gating comment; the icu
  note: nova's temporal_rs pulls icu 2.x, so migrating `components/fonts` off
  icu 1.5 unifies a currently-duplicated icu tree; see the mere
  dependency-footprint brief 2026-07-04).
- mere `2026-07-04_dependency_footprint_brief.md` (fork/gate rationales).
- genet `2026-07-02_dom_mutation_capture_replay_plan.md` (the DOM mirror
  half of tier 3).
- mere memory/plan trail: gnode-pool and native-surface-compositing own the
  visual-snapshot half of tier 3.
