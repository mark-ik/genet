# P2P Collaboration Plan (Refactored 2026-02-20)

**Status**: Planning phase. Supersedes 2026-02-11 stub. Aligns with current architecture (fjall/redb/rkyv, encryption at rest, edge traversal model) and cross-references detailed P2P spec in `verse_docs/GRAPHSHELL_P2P_COLLABORATION.md`.

**Primary Source**: `../verse_docs/GRAPHSHELL_P2P_COLLABORATION.md` — full P2P sync architecture, command-based mutation model, version vectors, merge strategies, conflict resolution UI patterns.

**Cross-References**:
- `2026-02-20_edge_traversal_model_research.md` — Edge payload model impacts sync: edges accumulate `Vec<Traversal>` records, not typed singletons. Merge strategy must handle traversal append (commutative), not edge type conflicts.
- `2026-02-19_persistence_hub_plan.md` §Phase 4 — Encryption at rest (AES-256-GCM) is implemented; P2P content sharing should use this pipeline.
- `2026-02-20_settings_architecture_plan.md` — Sync settings and peer management UI should live at `graphshell://settings/sync` (not a floating panel).

---

## Design Principle: Local-First, Sync Second

Graphshell must:
1. Work perfectly offline (single user, no server dependency).
2. Support optional P2P sync (explicit opt-in per workspace or graph).
3. Handle conflicts gracefully (deterministic auto-merge where possible, UI prompt where ambiguous).
4. Preserve privacy (encrypted payloads, explicit consent before sharing any data).

**Anti-pattern**: Real-time collaborative editing (Google Docs style). Graphshell sync is batch-oriented, git-like, designed for asynchronous graph sharing across trusted peers.

---

## 1. Current Architecture Baseline (2026-02-20)

### 1.1 Persistence Layer

| Component | Technology | Current Capabilities | P2P Readiness |
| --- | --- | --- | --- |
| **Operation log** | fjall (append-only LSM) | Every graph mutation journaled as `LogEntry` | ✅ Replayable, sequential |
| **Snapshots** | redb (embedded KV) | Periodic full-graph snapshots | ✅ Merkle-tree baseline for sync deltas |
| **Serialization** | rkyv (zero-copy) | Fast encode/decode of `LogEntry` and `GraphSnapshot` | ✅ Network-efficient format |
| **Encryption** | AES-256-GCM + zstd | All persisted data encrypted at rest (keyring-backed key) | ✅ Reusable for P2P payloads |

**Gap**: No version vectors, no peer ID tracking, no conflict detection metadata on `LogEntry`.

### 1.2 LogEntry Structure

Current schema (from `persistence/types.rs:67`):

```rust
pub enum LogEntry {
    AddNode { node_id: String, url: String, position_x: f32, position_y: f32 },
    AddEdge { from_node_id: String, to_node_id: String, edge_type: PersistedEdgeType },
    RemoveEdge { from_node_id: String, to_node_id: String, edge_type: PersistedEdgeType },
    UpdateNodeTitle { node_id: String, title: String },
    PinNode { node_id: String, is_pinned: bool },
    RemoveNode { node_id: String },
    ClearGraph,
    UpdateNodeUrl { node_id: String, new_url: String },
}
```

**Strengths**:
- Deterministic, idempotent operations.
- Already journaled to durable storage before state mutation (write-ahead semantics).
- Edge ops are directed (from/to), supporting causal ordering.

**Gaps for P2P**:
- No `timestamp` (wall clock or logical).
- No `peer_id` (originating device/user).
- No `version_vector` (causal dependency tracking).
- No `sequence_number` exposed in the type (fjall sequence is internal).

### 1.3 Edge Traversal Model Impact

The proposed edge model (`2026-02-20_edge_traversal_model_research.md`) replaces `EdgeType` enum with:

```rust
struct EdgePayload {
    user_asserted: bool,
    traversals: Vec<Traversal>,  // append-only, commutative
}
```

**P2P Implication**: `AddEdge` becomes `AssertEdge` (sets `user_asserted: true`) or `AddTraversal { from, to, timestamp, trigger }`. Traversal records are **commutative** (order-independent append), simplifying merge: no conflict possible when two peers add different traversals to the same edge. This is a major sync win over the current `EdgeType` singleton model.

---

## 2. Proposed Sync Architecture (High-Level)

### 2.1 Transport: iroh (Recommended)

**Rationale** (from archived research, validated 2026-02-20):
- **iroh**: Pure Rust, QUIC-based, automatic NAT traversal, dial by public key, higher hole-punching success rate than libp2p.
- **libp2p**: Overkill for 2–10 peer graphs. Kademlia DHT and GossipSub are unnecessary for Graphshell's small-scale, trust-based collaboration model.
- **Alternatives**: Manual WebRTC (complex), Tailscale/Netbird (requires third-party account), direct TCP (NAT-unfriendly).

**Decision**: Use `iroh-net` for peer discovery and transport, `iroh-docs` or custom sync protocol over QUIC streams.

### 2.2 Sync Model: Version Vector + Operation Log Push/Pull

**Not a CRDT**: Graphshell does not need fine-grained real-time convergence. The graph is coarse-grained (nodes/edges), and users tolerate eventual consistency with conflict prompts.

**Strategy**: Hybrid version-vector + last-write-wins (LWW) for most fields, explicit conflict UI for structural changes (concurrent node deletions, concurrent edge assertions to the same pair).

**Protocol**:
1. **Handshake**: Peers exchange version vectors.
2. **Delta Computation**: Each peer identifies operations the other lacks (`remote.vector[self_id] < local.sequence`).
3. **Push/Pull**: Missing `LogEntry` records are serialized (rkyv), compressed (zstd), encrypted (AES-GCM, ephemeral session key), and sent over QUIC.
4. **Merge**: Receiver replays operations in causal order (topological sort by version vector), applying deterministic merge rules.

**No Central Authority**: Both peers are equals. Sync is bidirectional request-response, not client-server.

---

## 3. Implementation Phases

### Phase 1: Version Vector Foundation (Pre-sync, Local-Only)

**Goal**: Augment `LogEntry` and persistence layer with causal tracking, but no network code yet. Validate merge logic on single-machine simulated dual-workspace scenarios.

#### Step 1a: Extend `LogEntry` with Sync Metadata

Add fields to each variant (via a wrapper struct to avoid breaking existing enum):

```rust
pub struct SyncedLogEntry {
    pub entry: LogEntry,  // existing enum unchanged
    pub timestamp_ms: u64,  // Unix milliseconds
    pub peer_id: PeerId,    // UUID, device-specific (generated on first run from OS entropy)
    pub sequence: u64,      // monotonic counter per peer (fjall already provides this implicitly)
    pub version_vector: HashMap<PeerId, u64>,  // causal snapshot at entry creation
}
```

**Backward Compatibility**: Existing persisted `LogEntry` payloads deserialize into `SyncedLogEntry` with default metadata (`timestamp: 0, peer_id: LOCAL_DEFAULT, sequence: inferred, version_vector: empty`).

#### Step 1b: Persist Version Vector State

Add a new redb table:

```rust
const VERSION_VECTOR_TABLE: redb::TableDefinition<&str, &[u8]> = 
    redb::TableDefinition::new("version_vector");
```

On each `LogEntry` append, increment `self.version_vector[self.peer_id]` and persist the updated vector. On startup, restore vector from redb.

#### Step 1c: Merge Simulator (Test Harness)

Write a test utility that:
1. Creates two in-memory `GraphStore` instances (Peer A, Peer B).
2. Applies divergent operations to each (A adds node X, B adds node Y).
3. Exports operation logs from both.
4. Merges logs into a third `GraphStore` (Peer C).
5. Validates convergence: C's graph contains both X and Y, no duplicates, causal order respected.

**Validation Criteria**:
- Concurrent `AddNode` with same URL but different `node_id`: both nodes survive (duplicates allowed, user merges later via UI).
- Concurrent `UpdateNodeTitle` on same node: later timestamp wins (LWW).
- Concurrent `RemoveNode` + `UpdateNodeTitle`: delete wins (tombstone semantics).
- Concurrent `AddEdge` on same node pair with different `edge_type` (legacy model): conflict flagged for user resolution. Under traversal model: both survive as separate traversal records.

#### Deliverable

- [ ] `SyncedLogEntry` wrapper implemented and integrated into `GraphStore::append_log`.
- [ ] Version vector persisted and restored correctly.
- [ ] Test suite: `test_merge_concurrent_add_node`, `test_merge_lww_title_update`, `test_merge_delete_wins`, `test_merge_commutative_traversals`.

---

### Phase 2: Sync Backend Abstraction (Network-Agnostic Interface)

**Goal**: Define a trait for sync backends and implement a "local filesystem" backend (simulates sync by copying log files between directories). This enables testing sync logic without iroh dependency.

#### Step 2a: Define `SyncBackend` Trait

```rust
#[async_trait]
pub trait SyncBackend: Send + Sync {
    /// Discover available peers. Returns list of peer IDs and human-readable names.
    async fn discover_peers(&self) -> Result<Vec<(PeerId, String)>, SyncError>;
    
    /// Fetch the version vector from a remote peer.
    async fn get_peer_version_vector(&self, peer_id: PeerId) -> Result<HashMap<PeerId, u64>, SyncError>;
    
    /// Fetch a range of log entries from a remote peer.
    async fn pull_entries(&self, peer_id: PeerId, after_seq: u64, limit: usize) -> Result<Vec<SyncedLogEntry>, SyncError>;
    
    /// Push local log entries to a remote peer.
    async fn push_entries(&self, peer_id: PeerId, entries: Vec<SyncedLogEntry>) -> Result<(), SyncError>;
}
```

#### Step 2b: Implement `LocalFilesystemBackend`

Simulates sync by watching a shared directory:

```text
/tmp/graphshell_sync/
    peer_a/
        version_vector.json
        log_entries/
            00000.json
            00001.json
    peer_b/
        version_vector.json
        log_entries/
            00000.json
```

Each peer writes its log entries to its subdirectory. `pull_entries` reads from another peer's subdirectory. This is **not** production-ready (no encryption, no conflict detection at transport layer), but sufficient for testing merge logic.

#### Step 2c: Sync Orchestrator

```rust
pub struct SyncOrchestrator {
    backend: Box<dyn SyncBackend>,
    local_store: Arc<Mutex<GraphStore>>,
    local_peer_id: PeerId,
}

impl SyncOrchestrator {
    pub async fn sync_with_peer(&self, peer_id: PeerId) -> Result<SyncReport, SyncError> {
        // 1. Fetch remote version vector
        let remote_vv = self.backend.get_peer_version_vector(peer_id).await?;
        
        // 2. Compute local delta (entries remote lacks)
        let local_delta = self.compute_local_delta(&remote_vv);
        
        // 3. Push local delta to remote
        if !local_delta.is_empty() {
            self.backend.push_entries(peer_id, local_delta).await?;
        }
        
        // 4. Compute remote delta (entries local lacks)
        let remote_seq = remote_vv.get(&peer_id).copied().unwrap_or(0);
        let local_seq = self.local_store.lock().unwrap().version_vector.get(&peer_id).copied().unwrap_or(0);
        let to_fetch = remote_seq.saturating_sub(local_seq);
        
        // 5. Pull remote delta
        if to_fetch > 0 {
            let remote_entries = self.backend.pull_entries(peer_id, local_seq, to_fetch as usize).await?;
            self.apply_remote_entries(remote_entries)?;
        }
        
        Ok(SyncReport { pushed: local_delta.len(), pulled: to_fetch as usize })
    }
}
```

#### Deliverable

- [ ] `SyncBackend` trait defined.
- [ ] `LocalFilesystemBackend` implemented and working.
- [ ] `SyncOrchestrator` orchestrates bidirectional sync.
- [ ] Integration test: Two `GraphStore` instances sync via filesystem backend, converge to identical graph state (modulo node order).

---

### Phase 3: iroh Transport Integration

**Goal**: Replace `LocalFilesystemBackend` with `IrohBackend`, enabling real P2P sync over the internet.

#### Step 3a: Peer Identity and Discovery

Each Graphshell instance generates a permanent `PeerId` (UUID) and an iroh node ID (Ed25519 keypair) on first run. Store in OS keyring or config file.

**Discovery Mechanisms** (pick one or support multiple):
1. **Manual**: User copies/pastes peer ID or connection string (Ticket) from one instance to another.
2. **Local Network**: mDNS broadcast (iroh-net supports this via `LocalDiscovery`).
3. **Relay Server** (optional): A public relay node for NAT-unfriendly networks (user-run or community-hosted).

**No DHT**: Graphshell does not need global peer-to-peer discovery. Users explicitly share connection info (like Tailscale or WireGuard).

#### Step 3b: Implement `IrohBackend`

```rust
pub struct IrohBackend {
    node: iroh::Node,
    peer_id: PeerId,
}

#[async_trait]
impl SyncBackend for IrohBackend {
    async fn discover_peers(&self) -> Result<Vec<(PeerId, String)>, SyncError> {
        // Query local mDNS or return manually-added peers from config
        todo!()
    }
    
    async fn pull_entries(&self, peer_id: PeerId, after_seq: u64, limit: usize) -> Result<Vec<SyncedLogEntry>, SyncError> {
        // Open QUIC stream to peer, send pull request, receive response
        let node_id = self.resolve_peer_id_to_iroh_node(peer_id)?;
        let mut stream = self.node.connect(node_id, b"graphshell-sync").await?;
        
        // Protocol: Send JSON request { "op": "pull", "after_seq": 42, "limit": 100 }
        // Receive JSON array of SyncedLogEntry (rkyv-serialized payloads)
        todo!()
    }
    
    // ... similar for push_entries, get_peer_version_vector
}
```

**Encryption**: Each QUIC stream is already encrypted by iroh-net (via QUIC TLS 1.3). Application-level payload encryption (reusing AES-GCM pipeline) is optional but recommended for defense-in-depth.

#### Step 3c: Sync UI Panel

Location: `graphshell://settings/sync` (as per `2026-02-20_settings_architecture_plan.md`).

**Components**:
1. **Peer List**: Show known peers (name, last sync time, sync status: Idle/Syncing/Error).
2. **Add Peer**: Input field for peer connection string (iroh Ticket or manual PeerId + relay address).
3. **Sync Now**: Button to manually trigger sync with selected peer.
4. **Auto-Sync Toggle**: Enable periodic background sync (every N minutes).
5. **Privacy Controls**: Opt-in toggle per workspace ("Allow sync for this workspace"). Default: off.

**Status Indicators**:
- "Synced 5 minutes ago"
- "Sync in progress… (23/47 operations)"
- "Conflict detected: 2 nodes require manual merge"

#### Deliverable

- [ ] `IrohBackend` implemented and tested on LAN (two machines on same WiFi).
- [ ] Manual peer addition via connection string works.
- [ ] Sync UI panel (`graphshell://settings/sync`) displays peer list and sync status.
- [ ] Background sync task runs periodically (tokio timer, respects auto-sync toggle).

---

### Phase 4: Conflict Resolution UI

**Goal**: When deterministic merge fails (e.g., concurrent `RemoveNode` + `UpdateNodeTitle`), surface a conflict and let the user decide.

#### Step 4a: Conflict Detection

During `apply_remote_entries`, detect patterns that require user input:

| Conflict Type | Condition | Auto-Merge? |
| --- | --- | --- |
| Concurrent node creation (same URL, different ID) | Two peers add nodes with same URL but different UUIDs | ✅ Keep both (user merges nodes via UI later if desired) |
| Concurrent title updates (same node) | Two peers update the same node's title at similar timestamps | ✅ LWW (latest timestamp wins) |
| Delete vs. Update | Peer A deletes node X, Peer B updates node X's title | ❌ Prompt: "Node was deleted. Restore and apply update, or keep deleted?" |
| Concurrent user-asserted edges | Both peers add `user_asserted` edge between same nodes | ✅ Auto-merge (no conflict — both assertions mean the same thing) |
| Concurrent traversals | Both peers add traversals to same edge | ✅ Auto-merge (commutative append) |

**Conflict Record**:

```rust
pub struct MergeConflict {
    conflict_id: Uuid,
    conflict_type: ConflictType,
    local_entry: SyncedLogEntry,
    remote_entry: SyncedLogEntry,
    suggested_resolution: Resolution,
}

pub enum ConflictType {
    DeleteVsUpdate,
    ConcurrentStructuralChange,
}

pub enum Resolution {
    KeepLocal,
    KeepRemote,
    KeepBoth,
    ManualMerge,  // user must decide
}
```

Store unresolved conflicts in `GraphStore` (new redb table: `CONFLICT_TABLE`). Block further syncs with that peer until conflicts are resolved (or allow user to force-sync with "accept all remote" or "accept all local").

#### Step 4b: Conflict UI

When conflicts exist, show a banner at the top of the graph view:

```
⚠️ 2 sync conflicts require attention. [Review Conflicts]
```

Clicking opens a modal or sidebar:

```
Conflict 1 of 2: Delete vs. Update
─────────────────────────────────────
Node: "Servo Documentation" (servo.org)

Local: You deleted this node 2 minutes ago.
Remote: Peer "Alice's Laptop" updated the title to "Servo Docs (archived)" 1 minute ago.

Resolution:
( ) Keep deleted
( ) Restore node with remote update
(•) Decide later

[Apply] [Skip]
```

#### Deliverable

- [ ] Conflict detection logic in `apply_remote_entries`.
- [ ] Conflicts persisted to `CONFLICT_TABLE`.
- [ ] Conflict review UI accessible from graph view banner or settings.
- [ ] User can resolve conflicts (apply resolution, update graph, clear conflict record).

---

### Phase 5: Validation and Stress Testing

#### Test Scenarios

1. **Offline Edits, Then Sync**: Peer A and Peer B both offline. A adds 10 nodes, B adds 15 nodes. Both come online, sync. Verify: 25 nodes total, no duplicates (assuming different URLs).
2. **Concurrent Edits to Same Node**: Both peers update the title of node X at the same second. Verify: LWW wins, loser's edit is discarded (no conflict).
3. **Delete vs. Update**: A deletes node X, B updates X's position. Verify: Conflict flagged, user prompted.
4. **Traversal Commutativity**: A logs traversal α→β at T1, B logs β→α at T2. Both sync. Verify: One visual edge with 2 traversal records (forward and reverse).
5. **Network Interruption**: Sync starts, network drops mid-transfer. Verify: Partial entries are not applied (atomic batch), sync resumes cleanly on reconnect.

#### Performance Targets

- Sync 1000 operations in <5 seconds over LAN.
- Sync 100 operations in <10 seconds over relay (high-latency path).
- Memory overhead: <50 MB for version vector and pending operation queue (10k operations).

#### Deliverable

- [ ] All 5 test scenarios pass.
- [ ] Performance benchmarks meet targets.
- [ ] Sync remains responsive during large merges (non-blocking UI, background thread for merge).

---

## 4. Out of Scope (Defer to Future)

### 4.1 Multi-Device Single User (Automatic Sync)

**Current Plan**: Treats each device as a separate peer. User must manually connect devices via peer IDs.

**Future Enhancement**: Auto-discover devices on same LAN (mDNS) or link via cloud identifier (email/passkey-based pairing, no user data sent to cloud).

### 4.2 Real-Time Collaborative Editing

**Not a Goal**: Graphshell is asynchronous-first. "Ghost cursors" and live viewport sync (from `verse_docs/GRAPHSHELL_P2P_COLLABORATION.md` §15.2) are research ideas, not Phase 1–5 deliverables.

### 4.3 Conflict-Free Replicated Data Types (CRDTs)

**CRDT Overkill**: Graphshell's mutation model is coarse-grained and infrequent (nodes/edges, not characters in a document). Version vectors + LWW + occasional conflict prompts are simpler and sufficient.

**Traversal Records Are Commutative**: The edge traversal model (`Vec<Traversal>`) is CRDT-like (G-Set, grow-only set) — this is the one place CRDT semantics apply naturally.

### 4.4 Graph Encryption (End-to-End)

**Already Implemented**: Encryption at rest (AES-256-GCM) protects local storage.

**P2P Encryption**: QUIC streams are TLS-encrypted. Payload-level encryption (double encryption) is optional and deferred.

**Verse Tokenization**: Out of scope. See `verse_docs/VERSE.md` for decentralized search/indexing research.

---

## 5. Dependencies and Risks

### 5.1 Dependencies

| Crate | Purpose | Maturity | License |
| --- | --- | --- | --- |
| `iroh-net` | QUIC transport, NAT traversal | Beta (0.2x) | Apache-2.0 / MIT |
| `iroh-docs` (optional) | Higher-level sync primitives | Alpha | Apache-2.0 / MIT |
| `serde` + `serde_json` | Version vector serialization | Stable | Apache-2.0 / MIT |

**Risk**: iroh API instability. Mitigation: Pin to specific version, wrap in adapter layer to isolate breaking changes.

### 5.2 Performance Risks

- **Full Graph Snapshot Sync**: Sending entire graph snapshot (1000 nodes) is expensive. Mitigation: Delta sync only (operation log).
- **Large Operation Logs**: After 10k operations, log size is ~5 MB. Mitigation: Snapshot compression (already uses zstd), or implement log compaction (merge consecutive updates to same node).

### 5.3 Privacy Risks

- **Accidental Data Leak**: User syncs private browsing history to untrusted peer. Mitigation: Opt-in per workspace, clear warnings before first sync.
- **Metadata Exposure**: Even encrypted, peer IDs and operation counts leak "how much you browse." Mitigation: Document in privacy policy, allow peer ID rotation.

---

## 6. Success Criteria

Phase 1–5 are considered **complete** when:

- [ ] Two Graphshell instances on separate machines can sync graphs bidirectionally over LAN.
- [ ] Concurrent edits merge correctly in at least 90% of cases (LWW, commutative traversals).
- [ ] Conflicts are surfaced in UI and resolvable by user.
- [ ] Sync state is durable (persisted version vector, resumable after crash).
- [ ] Sync is opt-in (no data leaves device unless user explicitly adds a peer).

---

## 7. Timeline Estimate

| Phase | LOC Estimate | Effort (Days) | Dependencies |
| --- | --- | --- | --- |
| Phase 1: Version Vector | ~300 LOC (types, persistence, tests) | 3–5 days | None (extends existing `LogEntry`) |
| Phase 2: Sync Backend Abstraction | ~400 LOC (trait, filesystem impl, orchestrator) | 5–7 days | Phase 1 |
| Phase 3: iroh Integration | ~500 LOC (backend impl, discovery, UI panel) | 7–10 days | Phase 2, iroh crates |
| Phase 4: Conflict UI | ~300 LOC (detection, storage, modal UI) | 4–6 days | Phase 3 |
| Phase 5: Validation | ~200 LOC (integration tests) | 3–5 days | Phase 4 |
| **Total** | **~1700 LOC** | **22–33 days** | — |

---

## Progress

- 2026-02-11: Initial stub plan created.
- 2026-02-20: Refactored to align with current architecture (fjall/redb/rkyv, encryption, edge traversal model). Cross-referenced `verse_docs/GRAPHSHELL_P2P_COLLABORATION.md` and persistence/settings architecture plans. Expanded to 5-phase roadmap with test criteria and deliverables.
