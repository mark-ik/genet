# Graphshell Design Documentation Index

**Last Updated**: February 18, 2026
**Status**: M1 complete; active: desktop architecture hardening, explicit targeting follow-up, edge/radial UX planning

---

## Essential Reading Order

1. **[README.md](README.md)** — Project vision, build & run, status summary
2. **[DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md)** — Orientation for contributors and AI assistants
3. **[ARCHITECTURAL_OVERVIEW.md](ARCHITECTURAL_OVERVIEW.md)** — Foundation code, architecture decisions, key crates
4. **[GRAPHSHELL_AS_BROWSER.md](GRAPHSHELL_AS_BROWSER.md)** — Browser behavior specification
5. **[IMPLEMENTATION_ROADMAP.md](IMPLEMENTATION_ROADMAP.md)** — Feature targets, validation criteria, execution order
6. **[../DOC_POLICY.md](../DOC_POLICY.md)** — Documentation and no-legacy defaults (see "Architecture-First Evolution")

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
| **[2026-02-17_feature_priority_dependency_plan.md](implementation_strategy/2026-02-17_feature_priority_dependency_plan.md)** | Feature-priority sequencing with dependency gating (F1-F7) |
| **[2026-02-18_f6_explicit_targeting_plan.md](implementation_strategy/2026-02-18_f6_explicit_targeting_plan.md)** | Explicit EGL/WebDriver targeting audit and local-first implementation strategy |
| **[2026-02-18_single_window_active_obviation_plan.md](implementation_strategy/2026-02-18_single_window_active_obviation_plan.md)** | Deferred follow-on inventory for structural single-window/single-active obviation |
| **[2026-02-18_edge_operations_and_radial_palette_plan.md](implementation_strategy/2026-02-18_edge_operations_and_radial_palette_plan.md)** | Edge create/remove UX, radial command model, and multi-select workflow plan |
| **[2026-02-17_egl_embedder_extension_plan.md](implementation_strategy/2026-02-17_egl_embedder_extension_plan.md)** | EGL extension phases: single-window hardening, semantic convergence, host/vsync contract, optional multi-window |
| **[2026-02-14_physics_migration_plan.md](implementation_strategy/2026-02-14_physics_migration_plan.md)** | Physics migration record (implemented) |
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
