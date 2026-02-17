# Graphshell Design Documentation Index

**Last Updated**: February 16, 2026
**Status**: M1 complete; active: navigation control-plane, physics migration, selection consolidation

---

## Essential Reading Order

1. **[README.md](README.md)** — Project vision, build & run, status summary
2. **[DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)** — Orientation for contributors and AI assistants
3. **[ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md)** — Foundation code, architecture decisions, key crates
4. **[GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md)** — Browser behavior specification
5. **[IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md)** — Feature targets, validation criteria, execution order
6. **[2026-02-14_no_legacy_development_policy.md](2026-02-14_no_legacy_development_policy.md)** — No-legacy development defaults

---

## Reference Docs

| Document | Purpose |
| -------- | ------- |
| **[CODEBASE_MAP.md](CODEBASE_MAP.md)** | Module breakdown, test distribution, data flow |
| **[BUILD.md](BUILD.md)** | Platform build instructions |
| **[QUICKSTART.md](QUICKSTART.md)** | Quick build reference |

---

## Active Implementation Plans

| Document | Scope |
| -------- | ----- |
| **[2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md)** | Consolidated architecture: semantic parity model, Servo delegate wiring, intent boundary, legacy cleanup |
| **[2026-02-14_physics_migration_plan.md](implementation_strategy/2026-02-14_physics_migration_plan.md)** | Replace custom physics with egui_graphs FruchtermanReingold |
| **[2026-02-14_selection_semantics_plan.md](implementation_strategy/2026-02-14_selection_semantics_plan.md)** | Single-source selection state |

## Future Feature Plans

| Document | Feature Target |
| -------- | -------------- |
| **[2026-02-11_bookmarks_history_import_plan.md](implementation_strategy/2026-02-11_bookmarks_history_import_plan.md)** | FT7: Bookmarks & history import |
| **[2026-02-11_performance_optimization_plan.md](implementation_strategy/2026-02-11_performance_optimization_plan.md)** | FT8: 500+ node performance |
| **[2026-02-11_clipping_dom_extraction_plan.md](implementation_strategy/2026-02-11_clipping_dom_extraction_plan.md)** | FT9: DOM element clipping |
| **[2026-02-11_diagnostic_inspector_plan.md](implementation_strategy/2026-02-11_diagnostic_inspector_plan.md)** | FT10: Engine inspector |
| **[2026-02-11_p2p_collaboration_plan.md](implementation_strategy/2026-02-11_p2p_collaboration_plan.md)** | FT11: P2P collaboration |

---

## Verse (Phase 3+ Research)

| Document | Purpose |
| -------- | ------- |
| **[verse_docs/VERSE.md](../verse_docs/VERSE.md)** | Tokenization research |
| **[verse_docs/GRAPHSHELL_P2P_COLLABORATION.md](../verse_docs/GRAPHSHELL_P2P_COLLABORATION.md)** | P2P collaboration patterns |
| **[verse_docs/SEARCH_FINDINGS_SUMMARY.md](../verse_docs/SEARCH_FINDINGS_SUMMARY.md)** | Verse research scan |

---

## Archive

**[archive_docs/](../archive_docs/)** — Superseded plans, completed work, checkpoint snapshots.

Latest checkpoint: `checkpoint_2026-02-16/` (19 archived docs including completed FT2-6 plans, egui_tiles migration, architecture research, and navigation options).
