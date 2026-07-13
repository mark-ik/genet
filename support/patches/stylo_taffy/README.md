# serval's stylo_taffy vendor

A vendored copy of `stylo_taffy 0.3.0-beta.1` (the first stylo_taffy release
targeting taffy `^0.12.1`; `alpha.5`/`alpha.6` still pinned the experimental
`0.11.0-experimental-cache-fix.3` line — see the taffy vendor's own
`SERVAL_PATCHES.md`). Vendored 2026-07-12 rather than depended on directly
from crates.io, for two reasons documented in
`docs/2026-07-12_ring3_fork_rename_publish_plan.md` (T2):

1. **Package renaming.** `[patch]` cannot rename its target (a patch source's
   package name must match the registry name it replaces), so a crate that
   needs serval's *renamed* stylo fork (`serval-stylo`, per T1) must depend on
   it directly by git URL + `package =`, not via a patched registry name.
   Since registry `stylo_taffy` depends on registry `stylo`/`stylo_atoms` by
   their real (unrenamed) names, it can never resolve to serval's fork as-is
   — vendoring is the only way to point it at `serval-stylo` /
   `serval-stylo-atoms`.
2. **Two source edits**, both to match serval's actual stylo fork surface
   rather than the registry `stylo` lineage stylo_taffy's authors track (see
   `src/convert.rs`, `stylo::TrackBreadth::Fr` — the published crate's
   `beta.1` source uses `TrackBreadth::Flex`, a newtype variant from a later
   registry `stylo` than serval's fork carries; serval's fork still has the
   servo-derived `Fr(CSSFloat)` shape). Everything else in `beta.1`'s source
   (notably taffy 0.12's `AlignContent`/`AlignItems` bitflag-style consts,
   replacing 0.11's CamelCase enum variants) is upstream's own change, taken
   as-is — `diff` against the published `beta.1` source shows only this one
   file, one variant-name difference.

## How to keep it in sync

When bumping taffy or serval's stylo fork, check whether a newer `stylo_taffy`
release exists and re-vendor from it (`cargo package --list` / the
crates.io API against `stylo_taffy`), re-applying the `TrackBreadth`
adjustment if serval's fork still uses the `Fr` shape by then. If serval's
fork ever adopts registry stylo's `TrackBreadth::Flex` newtype (a T1/fork
matter, not this vendor's), delete the adjustment and diff clean against
upstream.

## Dependencies

`Cargo.toml` depends on `style`/`style_atoms` directly by git URL (branch
`mark-ik/serval-publish-names`, `package = "serval-stylo"` /
`"serval-stylo-atoms"`) rather than through the workspace root's
`[patch.crates-io]` — again because patches can't rename. `taffy` (unrenamed)
rides the workspace root's `taffy` patch normally.
