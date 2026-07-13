# Xilem provenance and patch ledger

## Recorded bases

- Serval extraction source: `6b955ff96ed8b2912d04f7a36a85a36b401bb780`
- `mark-ik/xilem` main at audit: `5d72ad41eb660fa620110e045d332fd95684ebae`
- `linebender/xilem` main at audit: `c5950bcb03d4f3d187a20d1159f6aa276fd056bf`

Meristem began as Linebender's Apache-2.0 `xilem_core`, vendored into Serval at
`10b557c3d27003288bd54b86bb5225b4d8127e82`. The extraction repository replays
the four Serval commits that touched that subtree, preserving their authors,
dates, and messages before moving the files to `crates/meristem`.

This is not yet a complete Xilem history graft. C0 remains partial until the
filtered upstream `xilem_core` history is joined and `git log --follow` reaches
both upstream Xilem and Serval lineage.

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

