# Servo Integration Brief (Revised 2026-02-14)

**Purpose**: Define the current Graphshell/Servo integration contract.

## Current Contract
- Graphshell receives navigation semantics from Servo delegate events.
- Graph semantics are applied through `GraphIntent` reducer paths.
- `sync_to_graph` is reconciliation-only (cleanup/active highlight), not a semantic inference path.
- Runtime webviews are ephemeral; graph state is authoritative and persisted.

## Event Semantics
- `notify_url_changed`: update mapped node URL in-place.
- `notify_history_changed`: update node history metadata.
- `request_create_new`: create child node + hyperlink edge from parent mapping.
- `notify_page_title_changed`: update node title metadata.

## Non-goals
- No contingency architecture for alternate browser engines.
- No compatibility layer for pre-signal polling semantics.
- No legacy migration branches for nonexistent user data.

## Validation
- Same-tab navigation never creates a new node.
- New-tab creation creates exactly one node and one hyperlink edge.
- History/title updates are reflected on the mapped node deterministically.
