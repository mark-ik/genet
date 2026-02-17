# Graph Persistence Plan (2026-02-11, Revised 2026-02-14)

## Scope
- Goal: crash-safe, fast recovery for current Graphshell runtime.
- Storage: fjall mutation log + redb snapshots + rkyv serialization.
- Identity model: UUID-only node identity. URL is mutable metadata.

## Policy
- No legacy-user compatibility requirements.
- No backward-compat schema shims for pre-UUID data.
- If persistence format changes again, update code and tests in lockstep for the current format only.

## Active Work
1. Recovery correctness
- Verify snapshot load + log replay determinism with UUID identity.
- Verify duplicate-URL nodes recover correctly.

2. Durability tests
- Add/maintain tests for partial write, clear-all, and crash-restart scenarios.
- Ensure replay only applies valid UUID-addressable mutations.

3. Performance
- Benchmark save/load at 1k and 10k nodes.
- Tune snapshot cadence and compaction thresholds.

4. Diagnostics
- Track snapshot age and log growth.
- Emit warnings when log growth exceeds configured bounds.

## Validation
- Simulated crash during write restores to last durable state.
- Replay is UUID-driven and independent of URL uniqueness.
- Load time remains within target at 10k nodes.

## Progress
- 2026-02-11: Initial draft created.
- 2026-02-14: Rebased to UUID-only, no-legacy policy.
