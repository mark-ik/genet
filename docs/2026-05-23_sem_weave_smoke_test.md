# Sem / Weave Smoke Test Notes

**Date:** 2026-05-23
**Status:** Tooling note / local workflow recommendation.

## Summary

Ataraxy Labs' `sem` and `weave` are worth using as local, non-authoritative
helpers for agent-heavy development:

- `sem` is immediately useful for entity-level diff, entity listing, context,
  and impact queries on top of Git.
- `weave` is promising as a conflict-resolution assistant for false conflicts
  where independent edits touch different functions, classes, keys, or other
  structural entities in the same file.
- Neither should replace compile/test validation. They understand structural
  entities, not full program semantics or project invariants.

Do not enable repo-wide setup by default yet. Prefer explicit commands,
temporary tests, and local-only configuration until the tools have been used on
real Serval conflicts.

## What was tested

The initial check used the npm wrapper for `sem` in the Mere workspace:

```powershell
npx -y @ataraxy-labs/sem --version
npx -y @ataraxy-labs/sem diff --format plain
npx -y @ataraxy-labs/sem diff --format json
```

Observed result:

- `sem 0.5.5` ran successfully on Windows.
- The working tree had no semantic changes.
- A synthetic two-file Rust comparison reported exactly one modified function
  and one added function.
- A scoped real-file query over Mere's `control-plane` module found 51 entities,
  including enums, structs, impls, and functions.
- `sem context ActionBus --budget 2000 --json` and `sem impact ActionBus --file
  ... --json` both produced useful structured context.

The `weave` test used a temporary clone under `%TEMP%` and invoked
`weave-driver` directly. No `weave setup`, `sem setup`, `.gitattributes`, or
repo merge-driver configuration was changed.

```powershell
git clone --depth 1 https://github.com/Ataraxy-Labs/weave.git `
  $env:TEMP\ataraxy-weave-test\weave

cargo run --manifest-path `
  $env:TEMP\ataraxy-weave-test\weave\crates\weave-driver\Cargo.toml -- `
  --version
```

Observed result:

```text
weave-driver 0.3.3
```

## Weave positive control

Three TypeScript files were created in `%TEMP%`:

- `base.ts`: one existing function.
- `ours.ts`: base plus `validateToken`.
- `theirs.ts`: base plus `formatDate`.

Plain Git produced conflict markers for the two independent additions:

```powershell
git merge-file -p ours.ts base.ts theirs.ts
```

`weave-driver` merged the same three inputs cleanly:

```powershell
cargo run --manifest-path $manifest -- `
  base.ts ours.ts theirs.ts `
  -o out.ts -l 7 -p scenario.ts
```

The output preserved both independent functions:

```ts
export function existing(): string {
  return "base";
}

export function formatDate(date: Date): string {
  return date.toISOString().split("T")[0];
}

export function validateToken(token: string): boolean {
  return token.length > 0 && token.startsWith("sk-");
}
```

This validates the main useful case: Git can invent a line conflict when two
branches edit the same textual region, while `weave` can merge when the edits
belong to different structural entities.

## Weave negative control

A second three-way test edited the same `process` function on both sides:

- `ours`: `data.trim().toUpperCase()`.
- `theirs`: `data.trim().toLowerCase()`.

`weave-driver` exited with code 1 and reported one real conflict:

```text
weave [same-entity.ts]: unchanged: 0, CONFLICTS: 1
weave: 1 conflict(s) in 'same-entity.ts'
  - function `process`: both modified
```

The output conflict was scoped inside the function and included the base:

```ts
export function process(data: string): string {
<<<<<<< ours
  return data.trim().toUpperCase();
||||||| base
  return data.trim();
=======
  return data.trim().toLowerCase();
>>>>>>> theirs
}
```

This is the desired conservative behavior: preserve a true same-entity conflict
instead of silently choosing one side.

## Recommended use

Use `sem` freely for scoped inspection:

```powershell
npx -y @ataraxy-labs/sem diff --format plain
npx -y @ataraxy-labs/sem diff --format json
npx -y @ataraxy-labs/sem entities path\to\file.rs --json
npx -y @ataraxy-labs/sem context SymbolName --budget 2000 --json
npx -y @ataraxy-labs/sem impact SymbolName --file path\to\file.rs --json
```

Avoid broad scans on large workspaces unless the output is filtered. A raw
`sem entities crates --json` on a Mere-sized tree produced a very large stream.

Use `weave` first as an explicit preview or direct conflict-resolution helper:

```powershell
weave-cli preview feature-branch
```

or invoke `weave-driver` directly for a known conflict file while evaluating it.

Do not run these by default in shared repos yet:

```powershell
sem setup
weave setup
```

If setup is desired for experiments, prefer local-only configuration where the
tool supports it, and inspect the generated Git attributes/config before using
it on important branches.

## Adoption posture

For Serval and sibling repos, the useful near-term workflow is:

1. Use `sem diff` and `sem context` to brief agents and reviewers on what
   changed at entity granularity.
2. Use `sem impact` as a hint for affected dependents and tests, not as a
   substitute for `cargo check` or targeted tests.
3. Use `weave` on false conflicts caused by independent entity edits in the same
   file.
4. Keep normal Git merge and normal compiler/test validation as the authority.

The tools are best treated as structural navigation and conflict triage. They
make agent work less text-blind, but they do not prove behavioral correctness.