# Serval → Genet: the engine rename

**Date:** 2026-07-13
**Status:** executing.
**Decision:** Mark, 2026-07-13 — *serval* "doesn't have enough internal
resonance beyond 'fork of servo'," and undersells what the engine became
(data-oriented doctrine, pure Rust, modularity, tree diffing most of all).
**Genet** replaces it: the arboreal tree-cat, and — in ecology — the single
genetic individual behind a clonal colony (one genet, many *ramets*; the
original is the *ortet*). One state, N window lenses. The cat lane keeps
its continuity (meerkat, pelt); the name finally says what the thing does.

Namespaces claimed on crates.io 2026-07-13: `genet`, `genet-stylo`, plus
`cambium`, `meristem`, `sprigging` for the GUI extraction.

## Coordination with the Cambium extraction

[2026-07-13_cambium_extraction_and_genet_boundary_plan.md](./2026-07-13_cambium_extraction_and_genet_boundary_plan.md)
is live (C2 partial; C3–C5 pending) and its lane owns `repos/cambium`.
Verified before starting: **the two lanes are decoupled.** Cambium's only
Serval dependency is the published `serval-scripted-dom = "0.1.0"` from
crates.io — an immutable artifact — so renaming Serval's *source* packages
cannot break Cambium's build. Cambium's own HEAD already says
"Align Cambium with the Genet engine boundary."

Consequences for scope:

- **`serval-chisel`, `serval-xilem`, `serval-xilem-core` are NOT renamed.**
  They are dead crates walking: C5 deletes them from this workspace once
  they live as `sprigging`, `cambium`, and `meristem` in `repos/cambium`.
  Renaming them to `genet-*` would mint names that will never be published.
  Their package names and the `xilem_serval` lib name are *protected* through
  the sweep; only their dependency edges move to the new `genet-*` names so
  the workspace keeps compiling.
- The Chisel-specific DOM vocabulary (`<chisel-leaf>`, `chisel_leaf_key`)
  stays untouched — neutralizing it to `<custom-leaf>` is C3b's job, not
  this rename's.

## Rename map

Packages, directories, lib names, and identifiers all move together. Prose
`Serval` → `Genet`, `serval` → `genet`, `SERVAL` → `GENET`.

| Old package | Old lib | New package | New lib |
| --- | --- | --- | --- |
| `serval-layout` | `serval_layout` | `genet-layout` | `genet_layout` |
| `serval-render` | `serval_render` | `genet-render` | `genet_render` |
| `serval-extract` | `serval_extract` | `genet-extract` | `genet_extract` |
| `serval-documents` | `serval_documents` | `genet-documents` | `genet_documents` |
| `serval-scripted` | `serval_scripted` | `genet-scripted` | `genet_scripted` |
| `serval-scripted-dom` | `serval_scripted_dom` | `genet-scripted-dom` | `genet_scripted_dom` |
| `serval-scripted-worker` | `serval_scripted_worker` | `genet-scripted-worker` | `genet_scripted_worker` |
| `serval-static-dom` | `serval_static_dom` | `genet-static-dom` | `genet_static_dom` |
| `serval-static-html` | `serval_static_html` | `genet-static-html` | `genet_static_html` |
| `serval-winit-host` | `serval_winit_host` | `genet-winit-host` | `genet_winit_host` |
| `serval-paint-types` | `paint_types` (kept) | `genet-paint-types` | `paint_types` |
| `serval-wpt` | — | `genet-wpt` | — |
| `serval-web-smoke` | — | `genet-web-smoke` | — |

Types follow: `ServalElement` → `GenetElement`, `ServalAppRunner` →
`GenetAppRunner`, and so on. Files too: `serval_lane.rs` → `genet_lane.rs`,
`verso-tile/src/serval.rs` → `src/genet.rs`.

The stylo fork family renames in lockstep (its ring-3 rename already put it
on `serval-stylo`): `serval-stylo` → `genet-stylo`, `-atoms`, `-dom`,
`-static-prefs`, and the vendored `serval-taffy` → `genet-taffy`. The
`style` lib name stays `style` (servo's universal convention; nine consumers
`use style::`), so the `package =` / dependency-key discipline from the
ring-3 plan carries over unchanged.

## Protected from the sweep

- `docs/archive/**` and naming-history sections — they keep saying Serval,
  per the Strophos precedent. A rename that rewrites its own history is a
  lie.
- The three Cambium-bound packages and `xilem_serval` (above).
- `.cargo-check-logs/**` (gitignored build logs).
- This document.

## Published crates: what happens to the old names

Six `serval-*` packages are live on crates.io and cannot be unpublished:
`serval-extract` 0.1.0, `serval-scripted-dom` 0.1.0, `serval-static-dom`
0.1.0, `serval-paint-types` 0.1.0, `serval-chisel` 0.1.0,
`serval-xilem-core` 0.4.0. The rest of the engine (`serval-layout`,
`serval-render`, `serval-documents`, `serval-scripted`,
`serval-winit-host`, `serval-static-html`) was never published — it sits
behind the ring-3 fork wall — so those names cost nothing to abandon.

Policy, matching the adopted-family archive pattern: the six keep their
final published version and get a **tombstone release** whose description
redirects to the `genet-*` name. No indefinite compatibility wrappers; the
external-consumer count is zero. Publishing is Mark's separately authorized
step, as always.

## Order

1. **P1 — Serval repo internals.** Packages, dirs, libs, identifiers, docs.
2. **P1v — Verify.** `cargo check --workspace` cold (~30 min; `target/` is
   absent), then the standing suites: serval-layout, paint html→pixels,
   serval-scripted, and the nine WPT baselines.
3. **P2 — The stylo fork.** `serval-stylo*` → `genet-stylo*` on fork `main`
   (Track U already realigned it onto v0.19.0), plus the vendored taffy.
4. **P3 — Consumers.** mere, merecat, woodshed, strophe, isometry: manifests,
   imports, and the gitignored local `.cargo/config.toml` patch tables.
5. **P4 — Remotes.** Rename `mark-ik/serval` → `mark-ik/genet` on GitHub
   (its redirects keep existing git deps resolving, so this is not a flag
   day), rename the local checkout directory, push everything.

## The `UserValid` trap (found during execution)

A case-insensitive search for "serval" matches the CSS pseudo-class
identifier **`UserValid`** — "u‑*serVal*‑id" — along with `user-valid` and
`USER_VALID`. A blanket case-insensitive rename would silently corrupt
`UserValid` into `Ugenetid` across the selector parsers, in both this repo
and the stylo fork.

Every substitution in this rename is therefore **case-sensitive**
(`Serval`→`Genet`, `SERVAL`→`GENET`, `serval`→`genet`), which leaves
`UserValid` untouched because its `V` is capitalized. In the stylo fork,
where the *only* source-file matches were this false positive, the sweep
was scoped to the five manifests and no source file was touched at all.
Verified after the fact: zero occurrences of `ugenet`/`genetid`/`user-genet`,
and the serval diff is symmetric (3,524 insertions ↔ 3,524 deletions — a
pure substitution that adds and removes no lines).

## Stop rules

- If the workspace does not go green after P1, fix forward or revert P1
  wholesale — do not push a half-renamed tree that consumers pin by branch.
- Do not touch `repos/cambium`. Its lane is live and has uncommitted work.
- Do not publish anything. Name claims are already secured; releases are a
  separate authorization.
