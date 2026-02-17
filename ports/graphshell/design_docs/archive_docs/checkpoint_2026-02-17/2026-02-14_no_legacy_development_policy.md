# No-Legacy Development Policy (2026-02-14)

Graphshell is pre-user. There is no production user base and no legacy data that must be preserved.

## Required Defaults
- Prefer deletion over compatibility shims, unless ensuring compatibility between existing, supported features or subsystems.
- Do not add migration branches for historical formats unless explicitly requested.
- Keep one active architecture path; avoid fallback product modes that split implementation unless necessary to ensure proper ux (e.g. managing failure states that don't require maintaining obviated code).
- Keep tests focused on current semantics and current persistence schema.

## Documentation Rule
When older plans encode migration/legacy contingencies, either:
1. archive the old plan under a timestamped `archive_docs/checkpoint_*` folder, or
2. revise it in place to this policy.

This policy applies until superseded by an explicit product release and real-user migration requirement.
