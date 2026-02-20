# Cross-Platform Sync and Extension Plan (2026-02-20)

## Status: Active Planning

**Core Thesis**: Graphshell is fundamentally a **graph knowledge capture tool**, not a web browser. The desktop instance is the canonical store. Mobile clients, browser extensions, and platform-specific rich clients all connect via a **simple, server-less sync protocol** — reusing the P2P collaboration architecture with explicit device roles.

**Replaces (defers):**
- Single-Window/Single-Active Obviation Plan — EGL multi-window is not the focus; single-window EGL is sufficient for thin mobile clients
- iOS Port as full Servo browser — unnecessary complexity; iOS clients sync graphs, not render full web pages

**Keeps and layers:**
- iOS WKWebView (2026-02-19_ios_port_plan.md §Phase 1-2) — useful for rich content preview in iOS details view
- Android EGL embedder hardening (2026-02-17_egl_embedder_extension_plan.md) — useful as migration path if Android native support is desired, but not primary

---

## Purpose

Define a practical, device-agnostic architecture where:

1. **Desktop Graphshell** — canonical storage, full editing, P2P sync server
2. **Mobile clients** (iOS/Android) — lightweight sync clients, view/annotate graphs, optional native features (sharing intents, local storage)
3. **Browser extensions** (Firefox, Chrome, Safari) — inject graphs into web context, capture context into graphs, sync bookmarks
4. **All devices** — discoverable locally or via optional delegation server, eventual consistency via version vectors

This approach is:
- **Server-less by default** (local network sync, like AirDrop)
- **Layering-friendly** (platform capabilities are additive, not required)
- **Reusable** (same sync protocol and conflict resolution works across all clients)
- **MVP-focused** (Phase 1 syncs state; later phases add platform-specific UX)

---

## Design Principles

### 1. Local-First, Always Functional

- Every device is autonomous and complete (can work offline).
- Sync is eventual; conflicts are rare if peers follow intent semantics.
- No device depends on any other device being available.

### 2. Graph is the Interchange Format

- Mobile and extension clients don't replicate Graphshell's full UI.
- They sync the **graph state** (nodes, edges, persistence metadata).
- Each client renders the graph in a form suited to its platform (mobile native list, extension sidebar, desktop canvas).

### 3. Intent-Based Mutation (Inherited from Architecture)

- All state changes are expressed as `GraphIntent` (or device-agnostic equivalent).
- Intents are journaled, version-stamped, and replayed for conflict resolution.
- Platform-specific features (iOS sharing, Firefox context capture) are translated to intents before sync.

### 4. Platform Features Are Enhancements, Not Requirement

- A client can sync and work with **only** the core protocol.
- Platform-specific features (WKWebView content preview, shared storage, notification intent) are optional and gracefully degrade.

---

## Architecture

### Sync Protocol

#### Core Concepts

**Device ID**: Unique identifier per device (UUID, derived from certificate if authenticated).

**Graph Workspace**: Atomic unit of sync. Multiple workspaces per desktop instance; mobile/extension clients choose which workspaces to sync.

**Sync Unit**: Minimal transferable state:
```
SyncUnit {
  workspace_id: UUID,
  logical_timestamp: u64,           // Vector clock element for this device
  device_id: UUID,                  // Origin device
  version_vector: HashMap<UUID, u64>, // Causal deps: what each peer has seen
  mutations: Vec<GraphIntent>,      // Batch of intents applied
  snapshot_if_needed: Option<GraphSnapshot>, // For fast-forward or reset
}
```

**Sync Exchange**: 
1. Mobile asks: "What state do you have for workspace X, given my version_vector?"
2. Desktop replies: "Here are the SyncUnits you haven't seen yet, plus this snapshot if you're too far behind."
3. Mobile merges (see below) and updates its version vector.
4. Desktop also accepts mobile's SyncUnits (third-way merge if both changed the graph).

#### Discovery and Connection

**Local Network (Default)**:
- mDNS advertisement: `graphshell-<device-name>._tcp.local`
- Desktop announces available workspaces + TLS certificate fingerprint
- Mobile/extension connects directly over local WiFi/Bluetooth
- No server required; works in airplane mode after first pairing

**Delegation Server (Optional, Out of Scope for Phase 1)**:
- For cloud sync without a central authority
- Desktop can opt-in to relay through a server
- Server stores SyncUnits, relays between offline devices
- End-to-end encrypted (server never sees plaintext)

#### Authentication and Privacy

**Phase 1**: Fingerprint-based trust (like SSH):
- Mobile initiates: "TLS handshake"
- Desktop shows: "Unknown device <fingerprint>. Allow? (yes / no)"
- On yes: desktop stores fingerprint, mobile gets access to whitelisted workspaces

**Phase 2+**: Long-lived invite codes or OAuth-like flows.

### Conflict Resolution

Inherited from P2P spec (2026-02-11_p2p_collaboration_plan.md), adapted for device roles:

**Commutative Operations** (always safe to merge):
- `AddNode` (different UUIDs auto-assigned)
- `AddTraversal` to existing edge (append-only, order-independent)
- Metadata mutations on disjoint fields (title, position, tags)

**Non-Commutative** (requires version-vector check):
- `RemoveNode` — if both peers reference it in dependent intents, defer removal via archive
- `UpdateNodeTitle` (same field) — last-write-wins or manual merge prompt
- `SetNodePosition` (same field) — local edit wins (user is holding pointer on this device)

**Merge Strategy**:
1. Check version_vector: if mobile's VV ≥ desktop's VV for all peers, no conflicts
2. Apply mobile's intents to desktop's latest snapshot
3. If intent conflicts with post-snapshot intent on desktop, run conflict resolver (auto or UI)
4. Return merged state

### Mobile Client Architecture

#### Synchronization Layer (All Platforms)

```
┌─────────────────────────────────────┐
│ Mobile App (iOS/Android native)     │
├─────────────────────────────────────┤
│ Graph Sync Engine                   │
│ ├─ GraphState (in-memory cache)     │
│ ├─ LogEntry journal (local storage) │
│ ├─ SyncUnit encoder/decoder         │
│ └─ VersionVector tracker            │
├─────────────────────────────────────┤
│ Network Layer                       │
│ ├─ mDNS discovery                   │
│ ├─ WebSocket or HTTP/2 stream       │
│ └─ TLS + fingerprint trust          │
├─────────────────────────────────────┤
│ Platform Storage                    │
│ ├─ iOS: Core Data or SQLite         │
│ ├─ Android: Room or SQLite          │
│ └─ Shared: journal + snapshots      │
└─────────────────────────────────────┘
```

#### Rendering Layer (Platform-Specific)

**iOS**:
- SwiftUI list view of nodes (grouped by workspace)
- Tap to view node detail (title, tags, connected edges)
- Optional: WKWebView preview of node's stored URL (from 2026-02-19_ios_port_plan.md Phase 1 concepts, adapted)
- Swipe to sync / pull-to-refresh

**Android**:
- Material 3 RecyclerView of nodes
- Tap to view/edit node detail
- Optional: WebView preview (WebDriver compatible, parallel to WKWebView approach)
- Sync status indicator + manual/auto sync toggle

#### Features by Phase

**Phase 1** (MVP):
- List nodes in a workspace
- Create node (captures title, auto-infers UUID, timestamp)
- Edit node metadata (title, tags)
- Sync state with desktop (push + pull)
- Offline-first (all changes journaled locally before sync)

**Phase 2**:
- View node edges (connected nodes, edge types)
- Create edges (tap two nodes -> "Connect As...")
- Delete nodes/edges
- Search nodes (full-text on title, tags)

**Phase 3** (Platform-Specific Enhancements):
- **iOS**: Sharing intent integration (share URL -> create node, add to graph)
- **iOS**: iCloud sync option (AES-256-GCM encrypted, shared via iCloud Keychain)
- **Android**: Shared storage + MediaStore integration
- **Both**: Background sync (periodic check every 5 min if WiFi available)

#### Storage Strategy

**Keep it minimal**: Only store what's needed for offline access + diff computation.

```
iOS (Core Data):
├─ Workspace (sync from desktop)
│  ├─ workspace_id: UUID
│  ├─ name: String
│  └─ last_sync: DateTime
├─ Node (synced, cached)
│  ├─ node_id: UUID
│  ├─ title: String
│  ├─ tags: [String]
│  └─ position: (f32, f32)
├─ Edge (synced, cached)
│  └─ edge_id: (from_node_id, to_node_id)
└─ LogEntry (local journal for unsync'd changes)

Android: Same structure, Room ORM
```

---

### Browser Extension Architecture

#### Discovery and Delegation

Extensions run in sandboxed contexts (Firefox, Chrome); they cannot listen on mDNS. Two options:

**Option A: Desktop Companion** (Recommended for Phase 1)
- Extension talks to a **local companion app** (Electron, Tauri, or bundled with Graphshell)
- Companion announces mDNS, holds TLS cert, manages discovery
- Extension IPC → Companion → Graphshell over WebSocket

**Option B: Delegation Server** (Requires Phase 2)
- Extension registers a cloud account
- Desktop syncs to delegation server
- Extension pulls from server when user opens extension
- Still end-to-end encrypted (desktop encrypts before upload)

#### Manifest and Permissions

**Permissions needed**:
- `storage` — cache graph state locally
- `webRequest` or `webNavigation` — optional, to capture current page context
- `activeTab` — optional, to extract URL/title for new nodes
- Host access to `graphshell://` protocol or local port (companion)

#### Features by Phase

**Phase 1** (MVP):
- Sidebar icon → toggle "Add to Graph"
- Context menu: "Capture this page" → creates node with URL + title
- Sidebar shows search box (live filter of nodes)
- Click node → open in new tab

**Phase 2**:
- Sidebar shows graph structure (edges, related nodes)
- Drag URL from page → drop on node → create edge (context-derived type)
- Tag node from extension sidebar

**Phase 3**:
- Sync bookmarks ↔ graph (optional bidirectional)
- Annotation mode: highlight + comment → creates sub-node or edge
- Read-only public sharing (publish workspace URL, others browse via extension)

#### Storage and State

Extensions have very limited storage. Strategy:

1. **Host most state on desktop**, extension caches only essential data
2. **Lazy load**: extension syncs only when sidebar is open
3. **Cloud fallback**: if desktop unreachable, extension degrades to stored cache + read-only

```
Extension Storage (via browser.storage.sync or local):
├─ desktop_pairing_token: string (fingerprint trust)
├─ last_synced_graph: CompressedSnapshot
├─ search_cache: HashMap<String, Vec<NodeMetadata>>
└─ open_requests: Queue<GraphIntent> (queued while offline)
```

---

## Platform-Specific Layering

### iOS: WKWebView for Content Preview (Optional Enhancement)

From 2026-02-19_ios_port_plan.md, adapted:

- Mobile app fetches and caches a node's URL
- In detail view, optionally show WKWebView preview (opt-in, consume storage/battery)
- Preview is **read-only** (no JS mutation back to graph)
- Falls back gracefully if URL is unavailable or network is down

**When useful**: Node points to a web article; user can read it without leaving the app.

**Not required**: Core sync and graph functionality work without it.

### Android: EGL Option for Rich Rendering (Future)

From 2026-02-17_egl_embedder_extension_plan.md, adapted:

- If Android team wants a richer browser experience, can layer in EGL+Servo
- But not required for Phase 1-2 (native WebView + canvas works fine)
- Reuses WKWebView strategy above (optional content preview, not core)

### Desktop: No Changes to Architecture

Desktop Graphshell remains canonical store and serves sync requests. Features added:
- Sync status indicator (last sync time, device list)
- Workspace sync settings (which peers can access which workspaces)
- Optional delegation server config

---

## Integration with Existing Architecture

### P2P Collaboration (2026-02-11)

**Reuses**:
- LogEntry structure and intent-based mutation model
- AES-256-GCM encryption at rest (extend to sync payloads)
- Version vector and eventual consistency strategy
- Conflict resolution rules

**Extends**:
- Adds **device role** (desktop = primary, mobile/extension = replica)
- Exposes LogEntry sequence numbers in SyncUnit for bandwidth optimization
- Adds **workspace access control** (pinned nodes per device, discovery scope)

### Edge Traversal Model (2026-02-20)

**Syncs cleanly**:
- `EdgePayload { user_asserted: bool, traversals: Vec<Traversal> }` is append-only
- Traversals from different devices merge without conflict
- Mobile/extension can create user_asserted edges, traversals flow back to desktop

### Settings Architecture (2026-02-20)

**Sync settings and device management at**:
- `graphshell://settings/sync` — pair devices, manage workspace access, storage limits
- `graphshell://settings/accounts` — optional delegation server account

---

## Implementation Roadmap

### Phase 1: Core Sync Protocol (Weeks 1-4)

1. Define SyncUnit message format (protobuf or messagepack)
2. Implement version-vector logic and conflict detection
3. Add mDNS discovery to desktop
4. Create iOS/Android minimal sync client (list + create/edit nodes)
5. Create Firefox extension minimal client (sidebar, capture page)

**Exit criteria**: Mobile and extension sync graph state bidirectionally with desktop, offline mode works, no data loss.

### Phase 2: Rich Client Features (Weeks 5-8)

1. Edges and edge creation in mobile/extension clients
2. Search and filter across synced state
3. Background sync on mobile
4. Sharing intents on iOS

**Exit criteria**: Mobile users can browse and build graphs collaboratively; extension context-captures pages.

### Phase 3: Platform-Specific Polish (Weeks 9-12)

1. iOS WKWebView preview (fallback if available)
2. Android native sharing, Material 3 polish
3. Extension bookmark sync, annotation UI
4. Delegation server option (TBD scope)

**Exit criteria**: Delightful platform-native feel; optional cloud fallback.

---

## Comparison: Sync Client vs. Native Ports

| Dimension | Sync Client (This Plan) | Native EGL (Deferred) | Native iOS WKWebView (Deferred) |
|---|---|---|---|
| **Implementation Time** | ~8-12 weeks | 4-6 months | 3-4 months |
| **Maintenance Burden** | Low (shared sync code) | High (EGL complexity) | Medium (iOS SDK churn) |
| **Offline Capability** | Full (cached sync state) | Full (but slower) | Limited (WKWebView caches pages, not graph) |
| **Platform Integration** | Medium (intents, storage) | Hard (rendering, display link) | Hard (Apple APIs) |
| **User Value** | View/annotate graphs, capture context | Browse full web in app | Preview URLs, Apple ecosystem |
| **Scaling to 3rd Platforms** | Easy (same sync protocol) | Rewrite per platform | Rewrite per platform |
| **When to Choose** | Want multi-platform story now | Android must replicate Graphshell UX | iOS needs web content in app |

---

## Open Questions for Design Review

1. **Discovery scope**: Should desktop advertise on public WiFi, or only in trusted networks (home, office)? Privacy implications?

2. **Delegation server**: Is eventual-consistency P2P sufficient for MVP, or should we plan for cloud-fallback in Phase 1?

3. **Workspace access model**: Should mobile users be able to create new workspaces, or only sync existing ones from desktop?

4. **Identity and signing**: Do we need a formal key-signing mechanism (like GPG subkeys per device), or is fingerprint trust sufficient?

5. **Storage limits**: Should mobile/extension clients sync full graph state, or selective workspaces? Battery/storage cost on mobile.

6. **Real-time vs. batch**: Should sync be automatic when network available, or require user-initiated "Sync" action for predictability?

---

## References

- **P2P Collaboration Plan**: 2026-02-11, §1-5 (version vectors, conflict resolution, encryption)
- **Settings Architecture Plan**: 2026-02-20, sync settings page design
- **Edge Traversal Model**: 2026-02-20, append-only traversals (merge-friendly)
- **iOS WKWebView Path** (Deferred): 2026-02-19_ios_port_plan.md, Phase 1-2 (useful for preview feature layering)
- **EGL Embedder Hardening** (Deferred): 2026-02-17_egl_embedder_extension_plan.md, optional Android migration path

---

## Historical Context (Archive Notes)

**Single-Window/Single-Active Obviation Plan** (2026-02-18, now deferred):
- Proposed clean-up of EGL singleton patterns in preparation for true multi-window
- Conclusion: not worth the refactoring cost for EGL single-window model
- Kept for reference: if EGL multi-window ever becomes customer requirement, audit inventory is preserved

**iOS Port Plan** (2026-02-19, now downscoped):
- Proposed full Graphshell ported to iOS using WKWebView instead of Servo
- Conclusion: unnecessary; iOS users don't need full browsing. Sync client + WKWebView preview for rich nodes is enough
- Kept for reference: Phase 1-2 architecture (RendererId abstraction, Cargo.toml cfg gates) useful if iOS native support is desired later

