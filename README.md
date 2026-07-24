# genet

genet is a prototype web engine derived from [Servo](https://servo.org),
inspired by [Blitz](https://blitz.is/), and rebuilt on the
[Linebender](https://linebender.org/) ecosystem (vello, parley, peniko, kurbo,
taffy). It keeps Servo's Rust foundation and the Stylo CSS cascade, while
removing the SpiderMonkey scripting stack and the heavy multiprocess Servo
subsystems, then layers its own modular layout, scripting, and rendering paths
on top.

The workspace is a development monorepo, not a published library set. Every
crate is `publish = false`. The default workspace member is `ports/pelt`, a
reference browser and validation viewer (the genet-side analogue of
servoshell).

**Made with AI**

## How genet differs from Servo

- **Scripting.** SpiderMonkey (`mozjs`) and the SpiderMonkey-backed
  `components/script` / `script_bindings` are deleted from the build graph
  (cut 2026-05-20; git history is the reference for the eventual rebuild).
  Scripting is now an engine-neutral seam (`script-engine-api`) with pluggable
  backends: Nova (`script-engine-nova`, the primary native engine, 64-bit only),
  Boa (`script-engine-boa`, pure Rust, the wasm backend and conformance oracle),
  and Piccolo (`script-engine-piccolo`, a stackless-Lua option backend for
  mod-scripting). See `docs/2026-05-20_genet_script_engine_plan.md` and
  `docs/2026-05-25_js_execution_strategy.md`.
- **Layout.** `genet-layout` is a profile-neutral engine that consumes any
  `LayoutDom`-shaped DOM and emits a `GenetPaintList`. Its box tree is laid out
  by Taffy through `stylo_taffy`, styled by the Stylo cascade, with text shaped
  by parley. See `docs/2026-05-17_genet_layout_planes_architecture.md` and
  `docs/2026-06-16_genet_layout_roadmap.md`.
- **Rendering.** genet emits a paint list that netrender (a vello-based
  renderer in a sibling repo) lowers into a `netrender::Scene`. The lowering and
  paint-list contract crates (`paint_list_api`, `paint_list_render`) live in the
  netrender workspace; genet is a consumer.
- **Crypto / build environment.** `aws-lc-rs` was removed so the default build
  needs no NASM. The in-process `ipc-channel` fork drops the multiprocess IPC
  transport, which genet's single-process embedding does not use.

## Repository layout

```
components/   engine crates (layout, rendering, DOM providers, script engines, shared traits),
               plus two families adopted 2026-07-23: cambium/ (the reactive UI toolkit) and
               netfetcher/ (the Fetch network engine)
ports/        runnable hosts and runners (pelt, with its desktop support nested inside;
              genet-wpt)
examples/     standalone smoke binaries (netrender_smoke)
docs/         dated design docs and plans (current state lives here)
support/      vendored patches (taffy, gpu-allocator, ipc-channel) and profile-gate scripts
tests/        unit tests and the vendored web-platform-tests checkout (tests/wpt)
resources/    bundled runtime resources (UA assets, fonts, prefs)
```

### Key component crates

- `genet-layout` — profile-neutral layout engine; styled DOM to fragment tree
  to `GenetPaintList`. The engine of the project (~21k LOC across 27 modules,
  `box_tree.rs` is the core).
- `genet-render` — host render-driver: a `ScriptedDom` / `LayoutDom` to
  `netrender::Scene` assembly (cascade, layout, paint emit, scene), plus host
  spatial queries (hit-test, fragments, caret / selection rects) and
  accessibility-tree emission. GPU-free; separate from presentation.
- `genet-static-dom` — script-free static DOM provider for the low profiles.
- `genet-static-html` — static HTML profile witness for the profile ladder.
- `genet-scripted-dom` — mutable scripted-DOM provider (`LayoutDom` +
  `LayoutDomMut` over a `NodeId` arena, recording `DomMutation`s for layout
  invalidation).
- `genet-scripted` — the scripted tier: binds a script engine to
  `genet-scripted-dom` so JS mutates the DOM via `NodeId` reflectors.
- `genet-winit-host` — shared genet-on-winit plumbing: a wgpu surface +
  netrender renderer (boot / resize / rasterize / acquire) and winit-to-genet
  input mapping. Consumed by pelt and by sibling hosts.
- `genet-host-api` — the light host-facing contract: engine profiles,
  capabilities, resource fetching, and tile composition. Pelt, Mere, and
  Merecat consume it without making the products depend on one another.
- `script-engine-api` — engine-neutral scripting backend contract (names
  capabilities only; engine-native types stay inside each backend).
- `script-engine-nova` / `script-engine-boa` / `script-engine-piccolo` —
  the three backends behind the seam.
- `script-runtime-api` — browser host surface (global scope, `console`,
  `location`, `localStorage`, `window.history`, `element.style`,
  `getComputedStyle`) built on top of the engine-neutral VM primitives.
- `xilem-serval` — a `xilem_core` backend that diffs a Xilem view tree into
  genet's mutable `ScriptedDom`. `xilem-core` is a vendored verbatim mirror of
  upstream xilem's `xilem_core` so a bare clone needs no fork checkout.
- `genet-layout`'s neighbors: `paint`, `xpath`, `webgl-wgpu`, `webgl-essl`,
  `webgpu`, `webxr`, `fonts`, `media`, plus the inherited Servo `shared`
  trait/api crates.

### Ports (runnable)

- `pelt` — the reference browser / validation viewer. Default workspace member;
  plain `cargo build` / `cargo run` target it. Its private desktop support crate
  is nested at `ports/pelt/desktop`.
- `genet-wpt` — genet-native web-platform-tests runner over a selectable
  subset of `tests/wpt`. Phase 1 is a crash-smoke (load each test through
  `genet_static_dom::parse` + `genet_layout::render`, no GPU, no JS).

## Build and run

genet builds with cargo on the pinned toolchain (rust 1.95.0, set by
`rust-toolchain.toml`; `rustup` applies it automatically). The default member
set builds on a stock Windows toolchain (no NASM, MOZILLABUILD, clang-cl, or
vcvars).

```shell
# Build the default member (pelt).
cargo build

# Open the genet-native on-screen document viewer.
# Accepts file://, a bare path, and data: URLs out of the box.
cargo run -p pelt -- --engine static <url-or-file>
```

pelt is feature-gated by profile. The default features are
`viewer-netrender`. Additional run modes:

```shell
# Chrome demo: wrap the viewer in a xilem-serval omnibar + back/forward strip.
cargo run -p pelt --features chrome -- --chrome <url>

# Tile demo: split the window into per-document tiles.
cargo run -p pelt --features tiles -- --tiles <url>...

# Scripted profile (V4): run a page's inline <script> on a JS engine and
# render the mutated DOM. Boa by default; add scripted-nova for Nova.
cargo run -p pelt --features scripted -- --engine scripted <file>
cargo run -p pelt --features scripted-nova -- --engine scripted <file> --js nova

# Headless reftest harness (V3): GPU-free scene snapshot, or a PNG with
# the png-reftest feature.
cargo run -p pelt -- --engine headless --out <file>.scene
cargo run -p pelt --features png-reftest -- --engine headless --out <file>.png

# Remote (http(s)) loading is opt-in behind the netfetch feature.
cargo run -p pelt --features netfetch -- --engine static https://example.com

# Present-backend smokes (platform-gated):
cargo run -p pelt --features windows-present -- --windows-present-smoke
cargo run -p pelt --features macos-present   -- --macos-present-smoke
cargo run -p pelt --features linux-present   -- --wayland-present-smoke
```

`pelt --help` lists every profile, flag, and smoke runner.

Note: `--engine viewer` (the Masonry-era CPU-readback Xilem viewer) was retired
2026-06-12; `--engine static` is the live on-screen viewer.

### Tests

```shell
# Workspace unit/lib tests (genet-layout carries ~205 inline tests).
cargo test --workspace

# WPT crash-smoke over a subset of tests/wpt.
cargo run -p genet-wpt -- run <subset>   # e.g. css/CSS2/floats
```

## Engine profiles

Capabilities are tiered so each build pulls only what its profile needs (see
`docs/2026-05-12_genet_profile_ladder_plan.md`). pelt exposes the tiers as
`--engine` modes: `static` (on-screen document viewer), `scripted` (live,
script-driven DOM), and `headless` (GPU-free snapshot / reftest harness). The
`browser` and `viewer` profile names are accepted by the CLI for compatibility.

## Status

Active prototype. The current state of each subsystem is tracked in `docs/`,
named by date; the most recent docs are authoritative. Notable anchors:

- `docs/2026-06-16_genet_layout_roadmap.md` — the layout-engine map and the
  two open threads (real-web layout fidelity; element view + scripted tier).
- `docs/2026-06-14_engine_capability_audit.md` — the current hit-testing and
  browser-readiness capability ledger, grounded against file:line.
- `docs/2026-05-16_workspace_audit_snapshot.md` — workspace shape, the
  SpiderMonkey re-enable cost, and the dead-on-disk component list.

Layout, hit-testing (including inline boxes and `pointer-events`), document and
nested element scrolling, selection / caret / find-in-page, focus and Tab order,
and the external-texture element view are landed and tested. Open threads
include real-web layout fidelity (UA stylesheet, tables, float text-wrap,
engine-rendered form controls) and the scripted-tier consumer wiring.

## Relationship to sibling repos

- **netrender** (`github.com/mark-ik/netrender`) — the vello-based renderer
  genet emits into. Pulled as a git dependency (`branch = "main"`), and it owns
  the engine-agnostic `paint_list_api` / `paint_list_render` crates.
- **Forks** consumed as git deps: Vano's `nova_vm` package (local checkout at
  `crates/vano`, git fallback `mark-ik/nova`, `genet-embedder` branch) and
  `boa_engine` / `boa_gc` (mark-ik/boa, `genet` branch), each carrying additive
  reflector-liveness patches. The Stylo crates track the servo/stylo v0.18.0
  release tag by git rev.
- **smolweb** (`github.com/mark-ik/smolweb`) — the small-web wire layer.
  `components/errand` (the client integration) and `components/nematic` (the
  document engine) live here and consume `misfin`, `spartan-protocol`,
  `nex-protocol`, and `guppy-protocol` from crates.io. The protocol crates
  moved there on 2026-07-23; their names belong to those protocols'
  communities rather than to this engine.
- genet is consumed as the engine/host layer by the `mere` platform workspace
  and by all four products (`merecat`, `isometry`, `woodshed`, `hocket`). The
  dependency direction is one-way: those consume genet, genet does not depend
  on them.

The 2026-07-23 repo consolidation moved two families **into** this repo, on the
rule that a separate repository needs coherent identity apart from genet, mere,
and the products:

- `components/cambium/{cambium,cambium-nematic,cambium-winit,meristem,sprigging}`
  — the reactive UI toolkit. Absorbing it retired three `[patch.crates-io]`
  entries it carried only because path patches do not transit to git consumers.
- `components/netfetcher` — the Fetch network engine, which had to be
  genet-side to keep mere's dependency direction one-way.

Both keep their published names, versions, and licenses (`meristem` Apache-2.0,
`sprigging` MIT/Apache, the rest MPL-2.0), so they spell out their package
metadata rather than inheriting this workspace's Servo-shaped defaults.

## License

genet is a derivative of Servo and is licensed under MPL-2.0. Upstream Servo:
[servo.org](https://servo.org), [book.servo.org](https://book.servo.org).
