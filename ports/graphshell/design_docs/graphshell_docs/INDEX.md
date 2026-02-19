# Graphshell Design Documentation Index

**Last Updated**: February 19, 2026
**Status**: M1 complete; F1-F7 architectural features complete; active: workspace routing/membership implementation, graph UX polish, edge/radial UX follow-on, universal node content model vision

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
| **[tests/VALIDATION_TESTING.md](tests/VALIDATION_TESTING.md)** | Incomplete headed/extended validation tests extracted from archived plans |

---

## Active Implementation Plans

| Document | Scope |
| -------- | ----- |
| **[2026-02-16_architecture_and_navigation_plan.md](implementation_strategy/2026-02-16_architecture_and_navigation_plan.md)** | Consolidated architecture: semantic parity model, Servo delegate wiring, intent boundary |
| **[2026-02-17_feature_priority_dependency_plan.md](implementation_strategy/2026-02-17_feature_priority_dependency_plan.md)** | Feature-priority reference: F1-F7 dependency analysis and implementation record (all complete) |
| **[2026-02-18_single_window_active_obviation_plan.md](implementation_strategy/2026-02-18_single_window_active_obviation_plan.md)** | Deferred: structural single-window/single-active obviation inventory (EGL-only, no desktop impact) |
| **[2026-02-18_edge_operations_and_radial_palette_plan.md](implementation_strategy/2026-02-18_edge_operations_and_radial_palette_plan.md)** | Edge create/remove UX, radial command model, omnibar @ scope (Step 4d validation pending) |
| **[2026-02-19_workspace_routing_and_membership_plan.md](implementation_strategy/2026-02-19_workspace_routing_and_membership_plan.md)** | Workspace-first node-open routing, UUID-keyed membership index, resolver authority function |
| **[2026-02-17_egl_embedder_extension_plan.md](implementation_strategy/2026-02-17_egl_embedder_extension_plan.md)** | EGL extension phases: single-window hardening, semantic convergence, host/vsync contract |
| **[2026-02-19_graph_ux_polish_plan.md](implementation_strategy/2026-02-19_graph_ux_polish_plan.md)** | Keyboard shortcuts, hover/labels, visual differentiation (research §11 priority items) |
| **[2026-02-19_layout_advanced_plan.md](implementation_strategy/2026-02-19_layout_advanced_plan.md)** | Physics micro-improvements (auto-pause, reheat, placement), advanced layout (degree repulsion, label culling, domain clustering) |
| **[2026-02-19_undo_redo_plan.md](implementation_strategy/2026-02-19_undo_redo_plan.md)** | Undo/redo: implementation inventory, spec boundary clarification, unit test gaps |
| **[2026-02-19_persistence_hub_plan.md](implementation_strategy/2026-02-19_persistence_hub_plan.md)** | Persistence Hub expansion: Bookmarks, Node History, Maintenance (batch delete, log compaction, export/import) |

## Architectural Vision (Long-Term)

| Document | Purpose |
| -------- | ------- |
| **[2026-02-18_universal_node_content_model.md](2026-02-18_universal_node_content_model.md)** | Nodes as universal content containers: modular renderers, protocol resolver, Tor/IPFS/Gemini, platform webviews, version-controlled history |

---

## Future Feature Plans

| Document | Feature Target |
| -------- | -------------- |
| **[2026-02-11_bookmarks_history_import_plan.md](implementation_strategy/2026-02-11_bookmarks_history_import_plan.md)** | FT7: Browser bookmark/history file import (superseded for in-app bookmarks by persistence_hub_plan) |
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

Latest checkpoint: `checkpoint_2026-02-19/` (5 archived docs: selection semantics, physics migration, layout strategy, F1 validation checklist, F6 explicit targeting — all complete).
