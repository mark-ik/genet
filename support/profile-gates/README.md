# Profile-ladder tier gates

The profile ladder (`docs/2026-05-12_serval_profile_ladder_plan.md`) is
package-witnessed: the low, JS-engine-free tiers must not depend on a script
engine, a heavy servo render/host subsystem, or the WebGL shader compiler. These
gates are that witness.

## Canonical gate

`check-tiers.sh` (cross-platform; bash) is the canonical check. It scans the
JS-free tiers (`serval-static-dom`, `serval-static-html`) for forbidden crates by
exact crate name (anchored, so "boa" does not match "keyboard"), via
`cargo tree` (dependency resolution only, no build).

```sh
bash support/profile-gates/check-tiers.sh
```

Exit 0 = clean; non-zero prints the offending tier + crates. Extend the
`BLOCKED` list and `TIERS` array as engines/subsystems or tiers are added.

`check-static-html.ps1` is the older Windows-only, single-tier (static-html)
form; `check-tiers.sh` supersedes it (both tiers, cross-platform, anchored).

## CI wiring (prerequisite, not yet done)

The audit (`docs/2026-06-02_serval_holistic_audit.md`) calls for this gate to run
in CI; there is no CI in the repo yet. The blocker is dependency resolution:
`cargo tree` resolves the **whole** workspace, and the workspace pins the JS
engines as **external path deps** outside the repo:

- `nova_vm = { path = "../../crates/nova/nova_vm" }` (git form, per the root
  `Cargo.toml` patch comment: `git = "https://github.com/mark-ik/nova", branch = "serval-embedder"`)
- `boa_engine = { path = "../../crates/boa/core/engine" }` (no git form recorded)

So a runner must materialize those sibling checkouts (or the manifest must switch
to git patches in CI) before `cargo tree` can resolve. nova has a known git form;
boa's fork URL/branch is the missing piece. Once that bootstrap is settled, a
GitHub Actions job is one step:

```yaml
# .github/workflows/tier-gate.yml (after the engine-dep bootstrap)
#   - uses: actions/checkout@v4
#   - <materialize ../../crates/nova and ../../crates/boa>
#   - run: bash support/profile-gates/check-tiers.sh
```

Until then, run the gate locally (where the path deps exist) before pushing a
change that touches the low tiers' dependency surface.
