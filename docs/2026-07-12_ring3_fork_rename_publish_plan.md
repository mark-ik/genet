# Ring 3 unwalling: genet-stylo + genet-taffy, and the taffy ride-out

**Date:** 2026-07-12
**Status:** **T0, T1, T2, and T3's manifest sweep all landed 2026-07-12.**
genet's workspace now builds, tests, and passes all nine WPT baselines
against: taffy re-vendored to stable 0.12.1 (T0), the renamed stylo fork
family on branch `mark-ik/genet-publish-names` (T1, commit `efaa436663`),
a vendored + renamed-dependency `stylo_taffy` (T2), and every internal
consumer's manifest swept onto the renamed deps (T3's non-publish half).
**Not done:** the taffy package itself is not yet renamed to `genet-taffy`
(unnecessary until actual publishing — see T2 below, a scope correction from
the original plan), and no crate has been published to crates.io — that
remains Mark's per-crate call. Companion to
`2026-07-11_genet_publish_rings_plan.md` (this is the "ring 3 publishes
only if the fork family publishes under its own names" trigger, pulled by
Mark 2026-07-12).

## The decision

Publish the layout cone of ring 3 by giving the two forks registry names —
**genet-stylo** (family) and **genet-taffy** — while working taffy back to
a pure registry ride via upstream PRs (taffy is DioxusLabs, not Servo: the
no-upstream-PRs doctrine does not apply). The script cone (boa/nova fork
branches) stays walled; it is a separate decision with its own naming
posture (the nova fork deliberately keeps the `nova_vm` crate name).

All six candidate names were **free on crates.io** as of 2026-07-12:
`genet-stylo`, `genet-taffy`, `genet-stylo-atoms`, `genet-stylo-dom`,
`genet-stylo-taffy`, `genet-stylo-traits`.

## What the forks actually diverge in (verified)

**stylo** (`mark-ik/stylo`, branch `mark-ik/servo-media-features`; syncs
upstream via squash-sync, so raw merge-base diffs overstate — the semantic
set is the `servo:`-prefixed commits):

| crate | divergence | disposition |
| --- | --- | --- |
| `style` (pub. `stylo`) | ~1.8k lines: Tier D/E media features, multi-capability pointer/hover, forced-color-adjust (+revert), the f32 animation-boundary fix | **rename: `genet-stylo`** |
| `stylo_atoms` | +334 (media-feature atoms) | **rename: `genet-stylo-atoms`** |
| `stylo_dom` | +199 | **rename: `genet-stylo-dom`** |
| `stylo_static_prefs` | +208 | **rename: `genet-stylo-static-prefs`** |
| `malloc_size_of` | +15/-7 (a `OnceLock` impl) | **ride registry** — checked: the impl came from upstream (#365, lazy `AttrValue` serialization), not the fork; verify the registry release carries it at T3 |
| `selectors`, `servo_arc`, `style_traits`, `style_derive`, `to_shmem` | metadata/version churn only | **ride registry** (verify each compiles with fork `style` at the pinned versions) |

Keep `links = "servo_style_crate"` on genet-stylo: it makes cargo refuse
any graph that accidentally pulls registry `stylo` alongside it — the
silent-upstream-resolution trap caught by the genet-static-html
reclassification, turned into a hard error.

**taffy** (`support/patches/taffy`, was vendored at
`0.11.0-experimental-cache-fix.3` + three patches, now re-vendored at
`0.12.1` — see T0): upstream published a **stable 0.11.0, 0.12.0, 0.12.1**,
and 0.12.1 carries `float_layout` as a stable feature — the entire reason
for riding the experimental line is gone. Patch status confirmed against
0.12.1's actual source before re-vendoring:

- **0001 find_content_slot width-fit** — bug still present verbatim (the
  caller still takes the first vertically-eligible slot regardless of
  width); re-applied in its original shape (`min_width` threaded through
  `find_content_slot`, unchanged from the 0.11-experimental patch).
- **0002 exclusion-band accessor** — still absent; additive; consumed by
  genet-layout's parley IFC seam.
- **0003 flex `order`** — still absent (0.12.1 has no `order` on flex item
  style).

All three PR-able; drafted in `support/patches/taffy/UPSTREAM_PR.md`.

## Phases

### T0 — re-vendor taffy on 0.12.1 + upstream PRs — landed 2026-07-12

`support/patches/taffy` re-vendored wholesale from stable 0.12.1 (all 22
upstream-differing files taken as-is; only the 5 files the three genet
patches actually touch were hand-re-applied — `compute/float.rs`,
`compute/block.rs`, `compute/mod.rs`, `style/flex.rs`, `compute/flexbox.rs`).
0001 (width-fit) re-applied in its original shape (threading `min_width`
through `find_content_slot`); 0002 (exclusion-bands) and 0003 (flex `order`)
carried over unchanged in substance. `content_size` (a taffy feature genet
was already implicitly relying on for `Layout.content_size`, gated by a new
`#[cfg]` in 0.12 that didn't exist in 0.11-experimental) had to be added to
all four consumers' feature lists (`paint`, `genet-layout`, `genet-render`,
`genet-wpt`) — the one real gap the re-vendor surfaced.

**Verified:** genet-layout 320 tests, the html-to-pixels paint corpus (30),
`serval-xilem` (101), `genet-scripted` (45), and all nine WPT baselines (7
testharness + 2 reftest) — all green, zero regressions from taffy's own
0.11→0.12 feature work (block `align-content`, the grid
`MinContent`→`MaxContent` auto-row-height fix, aspect-ratio-via-known-dimensions).
Upstream PRs for all three patches drafted in `UPSTREAM_PR.md` (not yet
opened — that's also a go/no-go call, though a lower-stakes one than
publishing). **Sunset path unchanged:** each merged PR shrinks the fork; all
three merged + released ⇒ retire the taffy vendor entirely.

### T1 — stylo family rename (in `repos/stylo`, the fork branch) — landed 2026-07-12

Branch `mark-ik/genet-publish-names` (commit `efaa436663`, pushed):
`genet-stylo` / `genet-stylo-atoms` / `genet-stylo-dom` /
`genet-stylo-static-prefs`. Workspace checks clean, both standalone and
(later, per T3) as consumed by genet.

### T2 — vendor stylo_taffy, point it at the renamed fork — landed 2026-07-12

Vendored `stylo_taffy 0.3.0-beta.1` (not alpha.6: beta.1 is the first
release targeting taffy `^0.12.1` — alpha.5/6 still pin the experimental
line, discovered by checking each version's actual dependency requirements
via the crates.io API rather than assuming). Two source edits from the
published crate (see `support/patches/stylo_taffy/README.md`): reverted
`TrackBreadth::Flex` back to genet fork's `TrackBreadth::Fr` shape (a
registry-`stylo`-lineage rename stylo_taffy's authors made that genet's
fork, still servo-derived, doesn't have); everything else (taffy 0.12's
`AlignContent`/`AlignItems` bitflag-style consts) is upstream's own change,
taken as-is.

**taffy stays unrenamed — a scope correction from the original plan.**
`stylo_taffy` needed vendoring specifically because a `[patch]` cannot
rename its target (see the finding below), and stylo_taffy's own manifest
must name the *real* renamed packages (`genet-stylo`/`genet-stylo-atoms`)
directly — no way around vendoring for that half. But `taffy` itself is
**not** renamed by anything downstream of it (nothing needs `taffy` to
resolve to a package called something else), so it rides the ordinary
`[patch.crates-io] taffy = { path = "..." }` redirect exactly as before —
simpler than the original plan assumed. Renaming taffy to `genet-taffy` is
only forced at actual-publish time (crates.io forbids git/path deps in a
published manifest), so it's deferred to whenever that's decided, not done
now.

### T3 — manifest sweep — the non-publish half landed 2026-07-12

Every internal consumer swept onto the renamed deps; verified with
`cargo check --workspace` (exit 0, zero errors) plus the full suite +
WPT-baseline sweep repeated post-sweep. Publishing itself (the other half of
T3) remains **entirely Mark's call, per crate**, as with rings 0–2 — nothing
in this session published anything.

**The finding that made this take longer than expected: `package =`
overrides Cargo's extern-name binding, not just its source.** Cargo's rule,
confirmed empirically here: with *no* `package =` on a dependency, the name
your code uses to `use` it is the target's own `[lib] name` — *not* the
dependency's TOML key. Adding `package = "..."` (required here, since
`genet-stylo`'s real package name now differs from what every consumer's
`Cargo.toml` key says) flips that: the extern name becomes the **TOML key**
instead. `stylo`'s package is renamed to `genet-stylo`, but its `[lib] name`
is still `style` (deliberately, so `use style::...` — servo's own universal
convention, used in nine consumer crates here — wouldn't need touching).
Adding the necessary `package = "genet-stylo"` to the `stylo = {...}`
workspace-dependency entry silently broke every one of those nine consumers
(`cannot find crate style`), because the key was `stylo`, not `style`. Fix:
rename the *dependency key* itself (in `[workspace.dependencies]` **and**
in the nine consumer manifests using `{ workspace = true }`) from `stylo` to
`style`, so key and desired extern name match again.
`stylo_atoms`/`stylo_dom`/`stylo_static_prefs` needed no such fix — their
`[lib] name` already equals their (pre-rename) package name, so key and lib
name were already the same string and the `package =` add didn't shift
anything.

**Sequencing note:** the rings 0–2 publish lane was active in the workspace
throughout this slice and repeatedly swept this session's in-flight edits
into its own commits (once mid-edit, capturing an intermediate/broken state
that a downstream git consumer — mere/meerkat — then built against; see
residuals below). Coordinate before either lane touches these manifests
again.

## Verification

genet workspace, post-sweep: `cargo check --workspace` (exit 0); test
suites — genet-layout 320, paint html→pixels 30, `serval-xilem` 101,
genet-scripted 45; WPT — all nine baselines (`dom`, `css_animations`,
`dom_abort`, `dom_nodes`, `css_mediaqueries`, `html_webappapis_timers`,
`css_position` testharness + `css_position`/`css_mediaqueries` reftest) at
`unexpected=0`, both immediately after T0/T2 and again after T3's sweep.

## Residuals, named

- **meerkat (mere) build is currently red against this work — a cross-repo
  git-sync timing artifact, not a bug in the verified state above.** mere
  pins genet by `git … branch = "main"`; the rings 0–2 lane's opportunistic
  commit of this session's in-flight edits (`dc394172`, "vendor the
  stylo_taffy patch crate that HEAD already references") captured an
  *intermediate* snapshot of the stylo_taffy vendor — mid-edit, before the
  git-direct-dependency fix and the `style`-key rename above — so mere
  currently hits the exact `links` conflict this doc's finding describes.
  Confirmed by running `cargo check --workspace` directly against this
  session's actual local tree (green, see Verification) — the discrepancy is
  purely which commit mere's lock resolves against. Self-resolves on the
  next full sweep/push of genet's `main`; no action taken here to force a
  push, consistent with how commits to genet have been handled all session.
- **`examples/genet_web_smoke` is broken independent of this work.** Its own
  standalone `[workspace]` fails with `no matching package named
  xilem-serval found` — that package was renamed to `serval-xilem` by a
  concurrent lane (see the same rename hit in the Verification suite list
  above) and this example's manifest wasn't updated. Found while checking
  whether the taffy/stylo_taffy version bumps needed mirroring there; they
  do (its own `[patch.crates-io]` block pins `stylo`/`stylo_atoms` to a bare
  upstream `servo/stylo` rev + an unbumped `taffy` path dep), but the example
  doesn't build at all right now for an unrelated reason, so that mirroring
  is deferred until someone fixes the `xilem-serval` reference first.

## Out of scope, named

- The script cone: boa fork, nova fork (`nova_vm` name is fork identity —
  vano posture), genet-scripted, and therefore genet-documents'
  `scripted` / `smolweb` **features** (cargo validates optional deps at
  publish too). The static lane publishes; the features unlock when that
  cone gets its own decision.
- Upstreaming any stylo change (Servo doctrine stands; the fork is the
  product).
