# Search And Filtering Plan (2026-02-11)

## Search And Filtering Plan
- Goal: Fast, fuzzy search and optional filtering for large graphs.
- Scope: Graph UI search bar, matching, highlighting, filter mode.
- Dependencies: nucleo crate, node title/URL metadata.
- Phase 1: Search UI
  - Add search field toggle (Ctrl+F) and clear (Esc).
  - Display match count and active index.
- Phase 2: Matching
  - Index node titles and URLs with nucleo.
  - Support fuzzy scoring and exact match fallback.
- Phase 3: Highlight and filter
  - Highlight matched nodes; dim non-matches.
  - Optional filter to hide non-matching nodes.
- Phase 4: Navigation
  - Up/Down to cycle matches; Enter focuses node.
- Validation tests
  - Typo-tolerant matches ("gthub" -> github).
  - 200+ nodes search remains responsive.
- Outputs
  - New search module and integration hooks.
  - Documentation of keybinds.

## Findings
- 

## Progress
- 2026-02-11: Plan created.
- 2026-02-14: Landed FT6 implementation:
  - Added graph search UI toggle (`Ctrl+F`) with clear-on-`Esc`.
  - Added fuzzy URL/title matching using `nucleo`.
  - Added highlight and filter mode integration in graph rendering.
  - Added result navigation (`Up/Down`) and active-result select (`Enter`).
  - Added unit coverage for matcher behavior and search-state helpers.
