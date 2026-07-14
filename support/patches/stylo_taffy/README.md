# genet's stylo_taffy vendor

A vendored copy of `stylo_taffy 0.3.0-beta.1` (the first stylo_taffy release
targeting taffy `^0.12.1`; `alpha.5`/`alpha.6` still pinned the experimental
`0.11.0-experimental-cache-fix.3` line — see the taffy vendor's own
`GENET_PATCHES.md`). Vendored 2026-07-12 rather than depended on directly
from crates.io for the package-renaming reason documented in
`docs/2026-07-12_ring3_fork_rename_publish_plan.md` (T2):

`[patch]` cannot rename its target (a patch source's package name must match
the registry name it replaces), so a crate that needs genet's renamed stylo
fork (`genet-stylo`, per T1) must depend on that package directly, not via a
patched registry name. Since registry `stylo_taffy`
depends on registry `stylo`/`stylo_atoms` by their real names, it cannot
resolve to genet's fork as-is.

The old v0.18 compatibility edit (`TrackBreadth::Flex` -> `Fr`) was removed
when Track U realigned the fork onto v0.19.0. The vendored source now follows
the published beta.1 `Flex` spelling; vendoring remains necessary only for the
renamed direct dependencies.

## How to keep it in sync

When bumping taffy or genet's stylo fork, check whether a newer `stylo_taffy`
release exists and re-vendor from it (`cargo package --list` / the
crates.io API against `stylo_taffy`). Keep the source diff clean where
possible; the required local change is the renamed dependency pair in
`Cargo.toml`.

## Dependencies

`Cargo.toml` depends on the published `genet-stylo` and
`genet-stylo-atoms` packages directly rather than through the workspace root's
`[patch.crates-io]` — again because patches can't rename. `taffy` (unrenamed)
rides the workspace root's `taffy` patch normally.
