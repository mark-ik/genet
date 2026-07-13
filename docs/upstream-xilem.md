# Xilem provenance and patch ledger

## Recorded bases

- Serval extraction source: `6b955ff96ed8b2912d04f7a36a85a36b401bb780`
- `mark-ik/xilem` main at audit: `5d72ad41eb660fa620110e045d332fd95684ebae`
- `linebender/xilem` main at audit: `c5950bcb03d4f3d187a20d1159f6aa276fd056bf`

Meristem began as Linebender's Apache-2.0 `xilem_core`, vendored into Serval at
`10b557c3d27003288bd54b86bb5225b4d8127e82`. The extraction repository replays
the four Serval commits that touched that subtree, preserving their authors,
dates, and messages before moving the files to `crates/meristem`.

The upstream side is a path-only replay of the 113 commits that touched
`xilem_core` through the recorded `mark-ik/xilem` base. The filtered tip is
`7f61f0537c2d911498cf0e7c940b377cb7673a76`; merge
`51a8a6a72fc1021ffcaa4c4d7a7ca5dbebddb7bf` joins that lineage to the
Serval-derived extraction without replacing Cambium's live tree. Patch replay
rewrites commit hashes but retains original authors, dates, and messages.

## Semantic patches over the vendored core

The initial Cambium patch set adds three defaulted `ElementSplice` operations:

- `hoist_pending`: preserve a backing node during a same-parent reorder.
- `extract_pending`: park a backing node without destroying it.
- `adopt_pending`: place a parked node under a new parent.

These operations support keyed and portable views in the Serval backend. Their
default implementations preserve compatibility for other Meristem backends.

## Update policy

Reconcile against a recorded Xilem release or commit. Compare the retained core
surface, update this ledger, and run the keyed and portable-move tests before
accepting an upstream change.

The `upstream-xilem` remote points to `mark-ik/xilem`. Fetching it does not
merge the wider Xilem workspace; updates are filtered to the core path first.

