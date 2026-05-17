# Serval workspace audit — state snapshot (2026-05-16)

Point-in-time snapshot of the serval workspace after the audit landed on 2026-05-15 and the follow-on commits on 2026-05-16. Companion to [archive/2026-05-05_serval_netrender_cut_plan.md](./archive/2026-05-05_serval_netrender_cut_plan.md) (strategy; archived 2026-05-17) and [archive/2026-05-13_p2_layout_dom_provider_design.md](./archive/2026-05-13_p2_layout_dom_provider_design.md) (next phase; superseded by [planes architecture](./2026-05-17_serval_layout_planes_architecture.md); archived 2026-05-17).

## Live workspace shape

After the SpiderMonkey-opt-in + `components/servo` + aws-lc-rs + example-bins trim:

- **Renderer/text deps**: vello 0.9 + wgpu 29 + skrifa 0.42 + peniko 0.6. No `[patch]` table, no fork. vello 0.9 dropped 2026-05-15 — fresh release; watch for upstream regressions before linebender does.
- **Build status**: `cargo check --workspace` is green on vanilla Windows — no NASM / MOZILLABUILD / clang-cl / vcvars required for the default member set.
- **Workspace members**: ~15 live crates (4 pelt ports + 6 components + 4 tests + 1 bin; read [Cargo.toml](../Cargo.toml) for the authoritative list).
- **`tests/unit/script` is excluded** in `workspace.exclude`. That single line is the gate: re-adding it pulls `mozjs_sys` back in along with all its build-env requirements.
- **aws-lc-rs removed** from `workspace.dependencies`. `components/net/Cargo.toml` now uses rustls's `ring` feature. If crypto comes back through a revived components/net, `ring` (pre-built asm) or `rustls-rustcrypto` (pure Rust) are the canonical choices.
- **2 demo bins removed**: `examples/wgpu-embedder`, `examples/non-presenting-wgpu-embedder`.
- **~56 `servo-*` entries remain** in `workspace.dependencies` — mostly `*-traits` / `*-api` interface crates still reached. The `package = "servo-..."` lines on otherwise serval-friendly crates leak fork origin even where the workspace-local name is clean.

## Dead-on-disk components (next deletion-pass candidates)

No live consumer in the current `workspace.members` as of 2026-05-16. **This is a starting set, not a green-lit deletion list** — verify the no-live-consumer claim against each crate's reverse deps before deleting:

- `components/net`
- `components/devtools`
- `components/layout` (the *old* layout — distinct from the live one)
- `components/storage`
- `components/webdriver_server`
- `components/bluetooth`
- `components/canvas`
- `components/constellation`
- `components/media/backends/ohos`
- `components/background_hang_monitor`

## Build env: SpiderMonkey re-enable cost

If anyone removes `tests/unit/script` from `workspace.exclude`, the full mozjs build env returns. The captured env script lives at `.cargo-check-logs/cargo-check-env.ps1` and requires:

- NASM on `PATH` (for aws-lc-sys)
- `CFLAGS=-utf-8` / `CXXFLAGS=-utf-8` (fmt 11.x `static_assert`)
- `CC=clang-cl` / `CXX=clang-cl` (mozjs 140 has unprotected `__attribute__((__packed__))`)
- `MOZILLABUILD=C:/mozilla-build` (moztools lookup)
- VS 2022 vcvars64 environment (ATL/MFC for v143 + Windows SDK)

Flag any plan that re-enables JS-engine work so this cost is on the table.

## Strategic anchors

- **Blitz/Serval convergence is now feasible to evaluate side-by-side.** The trim was the precondition; serval's shape is finally narrow enough to compare against linebender's `blitz` `packages/*` and read the overlap. Don't defer the audit further; propose it when next relevant.
- **W3C capability knockout pattern**: serval cuts deliberately delete or stub W3C-coupled features (WebXR, WebGL service workers in the viewer profile, etc.) rather than migrating them through every refactor. The dead-components list above is the next pass of the same pattern. Surface the tradeoff explicitly when proposing a delete-or-stub.
- **Three-head Hekate**: the planned evolution is serval as a smolweb-extract / middlenet / fullweb negotiator over the same HTML input — three render modes, one engine. Design only as of 2026-05-16; no implementation. HTML-reader-mode work belongs *inside serval* (any depth), not in [nematic](../../mere/) (smolweb-only protocol engine).

## Sidequests on the table

1. **Continue the dead-components deletion** for the 10 listed above (verify per-crate first).
2. **Rename pass on `servo-*` package names** — even when workspace-local names are friendly, the `package = "servo-..."` lines leak fork origin.
3. **Three-head Hekate implementation** — exists as design framing only.

## Pitfalls to keep in view

- **vello 0.9 freshness**: shipped 2026-05-15. If linebender finds a crash, expect to hear about it before they do; be ready to roll forward.
- **Re-enabling SpiderMonkey is sticky**: it's one `workspace.exclude` edit, but it brings back the full Windows env requirement. Don't do it casually.
- **components/net revival** would force a TLS-provider decision (ring vs rustls-rustcrypto vs aws-lc revival). Pure-Rust `ring` is the current default; aws-lc would re-introduce NASM.
