# Bookmarks And History Import Plan (2026-02-11)

**Data model update (2026-02-20):**
This plan predates the unified tag system (persistence hub plan Phase 1,
`2026-02-19_persistence_hub_plan.md`). The following table maps old concepts to current
architecture. All references to "bookmarks" and "folder labels as metadata" below should be
read through this mapping:

| Old concept | Current equivalent |
| --- | --- |
| Imported bookmark → "bookmarked node" (`is_bookmarked`) | `TagNode { tag: TAG_STARRED }` — node tagged `#starred` |
| Folder label as metadata | User-defined tag per folder path component (e.g. `Bookmarks/Work/Tools` → tags `Work`, `Tools`) |
| History visit → node creation | `AddNode` intent (URL dedup via `get_node_by_url` — existing behavior) |
| Referrer edge | `AddEdge` with `EdgeKind::Traversal` or `UserCreated` (cross-reference: edge traversal plan) |

The import wizard emits standard `GraphIntent` sequences (`AddNode`, `AddEdge`, `TagNode`) — no
new persistence primitives are needed. The tag and node data models are already defined.

---

## Bookmarks And History Import Plan

- Goal: Seed graph from browser bookmarks and history data.
- Scope: Import UI and parsers; graph merge logic.
- Dependencies: file picker, JSON/HTML parsing, SQLite access.
- Phase 1: Bookmarks import
  - Parse Firefox/Chrome bookmarks export formats (Netscape HTML, Chrome JSON).
  - Create nodes via `AddNode`; tag each with `#starred` via `TagNode { tag: TAG_STARRED }`.
  - Emit `TagNode { tag: folder_name }` for each folder path component (user-defined tags).
- Phase 2: History import
  - Read browser history SQLite databases (read-only mode; row-by-row iteration — no
    file-derived data in SQL queries).
  - Create nodes from recent visits via `AddNode`.
  - Create referrer edges via `AddEdge` (cross-reference: edge traversal plan for edge kind).
- Phase 3: Dedup and merge
  - Merge nodes by URL — `get_node_by_url()` returns the existing node if already present;
    import only adds missing data (tags, edges) to it rather than creating duplicates.
  - Preserve folder labels as user-defined tags (see data model table above).
- Phase 4: UX flow
  - Provide import wizard and progress feedback.
  - Show preview of nodes/edges to be created before committing.
  - Report skipped duplicates and merge decisions.

## Validation Tests

- Import 100+ bookmarks with structure preserved; folder hierarchy reflected as user tags.
- Dedup avoids duplicate nodes for same URL (existing node receives tags; no second node created).
- Imported bookmarks carry `#starred` tag; verified via `tag_index[TAG_STARRED]`.
- History SQLite opened read-only; no SQL injection surface (row iteration, no dynamic queries).

## Outputs

- Import commands and parsers.
- Migration guide for users.

## Findings

- (See data model update note at top of file.)

## Progress

- 2026-02-11: Plan created.
- 2026-02-20: Updated data model references — `is_bookmarked` → `TagNode { tag: TAG_STARRED }`;
  folder labels → user-defined tags; referrer edges → `AddEdge` via edge traversal plan.
  Import wizard emits standard `GraphIntent` sequences; no new persistence primitives needed.
