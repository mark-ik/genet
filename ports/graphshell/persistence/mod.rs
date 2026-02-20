/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Graph persistence using fjall (append-only log) + redb (snapshots) + rkyv (serialization).
//!
//! Architecture:
//! - Every graph mutation is journaled to fjall as a rkyv-serialized LogEntry
//! - Periodic snapshots write the full graph to redb via rkyv
//! - On startup: load latest snapshot, replay log entries after it

pub mod types;

use crate::graph::Graph;
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use log::warn;
use rand::RngCore;
use redb::{ReadableDatabase, ReadableTable};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use types::{GraphSnapshot, LogEntry};
use uuid::Uuid;

const SNAPSHOT_TABLE: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new("snapshots");
const TILE_LAYOUT_TABLE: redb::TableDefinition<&str, &[u8]> =
    redb::TableDefinition::new("tile_layout");
pub const DEFAULT_SNAPSHOT_INTERVAL_SECS: u64 = 300;
const NAMED_GRAPH_PREFIX: &str = "named:";
const ENCRYPTED_PAYLOAD_MAGIC: &[u8; 8] = b"GSEV0001";
const AES_GCM_NONCE_LEN: usize = 12;

struct PersistenceKey {
    bytes: [u8; 32],
}

impl PersistenceKey {
    #[cfg(test)]
    fn load_or_generate_for_store(_base_dir: &std::path::Path) -> Result<Self, GraphStoreError> {
        Ok(Self { bytes: [0xA5; 32] })
    }

    #[cfg(not(test))]
    fn load_or_generate_for_store(base_dir: &std::path::Path) -> Result<Self, GraphStoreError> {
        let service = "graphshell.persistence";
        let account = base_dir.to_string_lossy().to_string();
        let entry = keyring::Entry::new(service, &account)
            .map_err(|e| GraphStoreError::Key(format!("Failed to access keyring: {e}")))?;

        match entry.get_password() {
            Ok(hex) => {
                let bytes = decode_hex_32(&hex).ok_or_else(|| {
                    GraphStoreError::Key("Stored key has invalid format".to_string())
                })?;
                Ok(Self { bytes })
            },
            Err(keyring::Error::NoEntry) => {
                let mut key = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut key);
                let encoded = encode_hex(&key);
                entry.set_password(&encoded).map_err(|e| {
                    GraphStoreError::Key(format!("Failed to persist generated key: {e}"))
                })?;
                Ok(Self { bytes: key })
            },
            Err(e) => Err(GraphStoreError::Key(format!(
                "Failed to read persistence key: {e}"
            ))),
        }
    }
}

#[cfg(not(test))]
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(not(test))]
fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (idx, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)? as u8;
        let lo = (chunk[1] as char).to_digit(16)? as u8;
        out[idx] = (hi << 4) | lo;
    }
    Some(out)
}

/// Persistent graph store backed by fjall (log) + redb (snapshots)
pub struct GraphStore {
    /// Kept alive so the Keyspace borrow remains valid (fjall requires it).
    _db: fjall::Database,
    log_keyspace: fjall::Keyspace,
    snapshot_db: redb::Database,
    log_sequence: u64,
    last_snapshot: Instant,
    snapshot_interval: Duration,
    persistence_key: PersistenceKey,
}

impl GraphStore {
    fn named_graph_key(name: &str) -> Result<String, GraphStoreError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(GraphStoreError::Io(
                "Graph snapshot name must not be empty".to_string(),
            ));
        }
        if trimmed == "latest" {
            return Err(GraphStoreError::Io(
                "Graph snapshot name 'latest' is reserved".to_string(),
            ));
        }
        Ok(format!("{NAMED_GRAPH_PREFIX}{trimmed}"))
    }

    /// Open or create a graph store at the given directory
    pub fn open(base_dir: PathBuf) -> Result<Self, GraphStoreError> {
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| GraphStoreError::Io(format!("Failed to create dir: {e}")))?;

        let persistence_key = PersistenceKey::load_or_generate_for_store(&base_dir)?;

        let log_path = base_dir.join("log");
        let snapshot_path = base_dir.join("snapshots.redb");

        let db = fjall::Database::builder(&log_path)
            .open()
            .map_err(|e| GraphStoreError::Fjall(format!("{e}")))?;

        let log_keyspace = db
            .keyspace("mutations", || fjall::KeyspaceCreateOptions::default())
            .map_err(|e| GraphStoreError::Fjall(format!("{e}")))?;

        let snapshot_db = redb::Database::create(&snapshot_path)
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;

        // Find the next log sequence number
        let log_sequence = Self::find_max_sequence(&log_keyspace) + 1;

        let mut store = Self {
            _db: db,
            log_keyspace,
            snapshot_db,
            log_sequence,
            last_snapshot: Instant::now(),
            snapshot_interval: Duration::from_secs(DEFAULT_SNAPSHOT_INTERVAL_SECS),
            persistence_key,
        };

        if store.has_legacy_plaintext_data() {
            warn!("Detected unencrypted legacy graph persistence; migrating in place.");
            store.migrate_legacy_plaintext_data()?;
        }

        Ok(store)
    }

    fn encode_persisted_bytes(&self, plaintext: &[u8]) -> Result<Vec<u8>, GraphStoreError> {
        let compressed = zstd::stream::encode_all(std::io::Cursor::new(plaintext), 3)
            .map_err(|e| GraphStoreError::Compression(format!("zstd encode failed: {e}")))?;
        let cipher = Aes256Gcm::new_from_slice(&self.persistence_key.bytes)
            .map_err(|e| GraphStoreError::Crypto(format!("AES key init failed: {e}")))?;
        let mut nonce = [0u8; AES_GCM_NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), compressed.as_ref())
            .map_err(|e| GraphStoreError::Crypto(format!("AES-GCM encrypt failed: {e}")))?;

        let mut out = Vec::with_capacity(
            ENCRYPTED_PAYLOAD_MAGIC.len() + AES_GCM_NONCE_LEN + ciphertext.len(),
        );
        out.extend_from_slice(ENCRYPTED_PAYLOAD_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    fn decode_persisted_bytes(&self, stored: &[u8]) -> Result<Vec<u8>, GraphStoreError> {
        if !stored.starts_with(ENCRYPTED_PAYLOAD_MAGIC) {
            // Legacy plaintext payload (pre-encryption migration path).
            return Ok(stored.to_vec());
        }
        if stored.len() < ENCRYPTED_PAYLOAD_MAGIC.len() + AES_GCM_NONCE_LEN {
            return Err(GraphStoreError::Crypto(
                "Encrypted payload too short".to_string(),
            ));
        }
        let nonce_start = ENCRYPTED_PAYLOAD_MAGIC.len();
        let nonce_end = nonce_start + AES_GCM_NONCE_LEN;
        let nonce = &stored[nonce_start..nonce_end];
        let ciphertext = &stored[nonce_end..];
        let cipher = Aes256Gcm::new_from_slice(&self.persistence_key.bytes)
            .map_err(|e| GraphStoreError::Crypto(format!("AES key init failed: {e}")))?;
        let compressed = cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|e| GraphStoreError::Crypto(format!("AES-GCM decrypt failed: {e}")))?;
        zstd::stream::decode_all(std::io::Cursor::new(compressed))
            .map_err(|e| GraphStoreError::Compression(format!("zstd decode failed: {e}")))
    }

    fn has_legacy_plaintext_data(&self) -> bool {
        let has_legacy_in_table = |table_def: redb::TableDefinition<&str, &[u8]>| {
            let Ok(read_txn) = self.snapshot_db.begin_read() else {
                return false;
            };
            let Ok(table) = read_txn.open_table(table_def) else {
                return false;
            };
            let Ok(iter) = table.iter() else {
                return false;
            };
            for entry in iter.flatten() {
                let (_, value) = entry;
                if !value.value().starts_with(ENCRYPTED_PAYLOAD_MAGIC) {
                    return true;
                }
            }
            false
        };

        if has_legacy_in_table(SNAPSHOT_TABLE) || has_legacy_in_table(TILE_LAYOUT_TABLE) {
            return true;
        }

        for guard in self.log_keyspace.iter() {
            let Ok((_, value)) = guard.into_inner() else {
                continue;
            };
            if !value.as_ref().starts_with(ENCRYPTED_PAYLOAD_MAGIC) {
                return true;
            }
        }
        false
    }

    fn migrate_legacy_plaintext_data(&mut self) -> Result<(), GraphStoreError> {
        self.migrate_legacy_plaintext_table(SNAPSHOT_TABLE)?;
        self.migrate_legacy_plaintext_table(TILE_LAYOUT_TABLE)?;

        let mut legacy_log_entries = Vec::<(Vec<u8>, Vec<u8>)>::new();
        for guard in self.log_keyspace.iter() {
            let (key, value) = guard
                .into_inner()
                .map_err(|e| GraphStoreError::Fjall(format!("{e}")))?;
            if !value.as_ref().starts_with(ENCRYPTED_PAYLOAD_MAGIC) {
                legacy_log_entries.push((key.to_vec(), value.to_vec()));
            }
        }

        for (key, plaintext) in legacy_log_entries {
            let encrypted = self.encode_persisted_bytes(&plaintext)?;
            self.log_keyspace
                .insert(key, encrypted.as_slice())
                .map_err(|e| GraphStoreError::Fjall(format!("{e}")))?;
        }
        Ok(())
    }

    fn migrate_legacy_plaintext_table(
        &mut self,
        table_def: redb::TableDefinition<&str, &[u8]>,
    ) -> Result<(), GraphStoreError> {
        let mut legacy_values = Vec::<(String, Vec<u8>)>::new();
        {
            let read_txn = self
                .snapshot_db
                .begin_read()
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            let Ok(table) = read_txn.open_table(table_def) else {
                return Ok(());
            };
            let iter = table
                .iter()
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            for entry in iter {
                let (key, value) = entry.map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
                if !value.value().starts_with(ENCRYPTED_PAYLOAD_MAGIC) {
                    legacy_values.push((key.value().to_string(), value.value().to_vec()));
                }
            }
        }

        if legacy_values.is_empty() {
            return Ok(());
        }

        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(table_def)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            for (key, plaintext) in legacy_values {
                let encrypted = self.encode_persisted_bytes(&plaintext)?;
                table
                    .insert(key.as_str(), encrypted.as_slice())
                    .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            }
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    /// Append a mutation to the log
    pub fn log_mutation(&mut self, entry: &LogEntry) {
        let plaintext = match rkyv::to_bytes::<rkyv::rancor::Error>(entry) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to serialize log entry: {e}");
                return;
            },
        };
        let bytes = match self.encode_persisted_bytes(plaintext.as_ref()) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to encrypt log entry: {e}");
                return;
            },
        };

        let key = self.log_sequence.to_be_bytes();
        if let Err(e) = self.log_keyspace.insert(key, bytes.as_slice()) {
            warn!("Failed to write log entry: {e}");
        }
        self.log_sequence += 1;
    }

    /// Take a full snapshot of the graph and compact the log
    pub fn take_snapshot(&mut self, graph: &Graph) {
        let snapshot = graph.to_snapshot();
        let plaintext = match rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to serialize snapshot: {e}");
                return;
            },
        };
        let bytes = match self.encode_persisted_bytes(plaintext.as_ref()) {
            Ok(b) => b,
            Err(e) => {
                warn!("Failed to encrypt snapshot: {e}");
                return;
            },
        };

        // Write snapshot to redb
        let write_result = (|| -> Result<(), GraphStoreError> {
            let write_txn = self
                .snapshot_db
                .begin_write()
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            {
                let mut table = write_txn
                    .open_table(SNAPSHOT_TABLE)
                    .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
                table
                    .insert("latest", bytes.as_slice())
                    .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            }
            write_txn
                .commit()
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            Ok(())
        })();

        if let Err(e) = write_result {
            warn!("Failed to write snapshot: {e}");
            return;
        }

        // Clear the log since we have a fresh snapshot
        self.clear_log();
        self.last_snapshot = Instant::now();
    }

    /// Recover graph state from snapshot + log replay
    pub fn recover(&self) -> Option<Graph> {
        let snapshot = self.load_snapshot();

        let mut graph = if let Some(snap) = &snapshot {
            Graph::from_snapshot(snap)
        } else {
            Graph::new()
        };

        self.replay_log(&mut graph);

        if graph.node_count() > 0 {
            Some(graph)
        } else {
            None
        }
    }

    /// Check if it's time for a periodic snapshot
    pub fn check_periodic_snapshot(&mut self, graph: &Graph) {
        if self.last_snapshot.elapsed() >= self.snapshot_interval {
            self.take_snapshot(graph);
        }
    }

    /// Configure periodic snapshot interval (seconds).
    pub fn set_snapshot_interval_secs(&mut self, secs: u64) -> Result<(), GraphStoreError> {
        if secs == 0 {
            return Err(GraphStoreError::Io(
                "Snapshot interval must be greater than zero seconds".to_string(),
            ));
        }
        self.snapshot_interval = Duration::from_secs(secs);
        Ok(())
    }

    /// Current periodic snapshot interval in seconds.
    pub fn snapshot_interval_secs(&self) -> u64 {
        self.snapshot_interval.as_secs()
    }

    /// Clear all persisted graph data (snapshot + mutation log).
    pub fn clear_all(&mut self) -> Result<(), GraphStoreError> {
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            table
                .remove("latest")
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            if let Ok(mut tile_table) = write_txn.open_table(TILE_LAYOUT_TABLE) {
                let mut keys = Vec::new();
                let iter = tile_table
                    .iter()
                    .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
                for entry in iter {
                    let (key, _) = entry.map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
                    keys.push(key.value().to_string());
                }
                for key in keys {
                    tile_table
                        .remove(key.as_str())
                        .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
                }
            }
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;

        self.clear_log();
        self.last_snapshot = Instant::now();
        Ok(())
    }

    /// Persist serialized tile layout JSON.
    pub fn save_tile_layout_json(&mut self, layout_json: &str) -> Result<(), GraphStoreError> {
        let encrypted = self.encode_persisted_bytes(layout_json.as_bytes())?;
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(TILE_LAYOUT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            table
                .insert("latest", encrypted.as_slice())
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    /// Load serialized tile layout JSON if present.
    pub fn load_tile_layout_json(&self) -> Option<String> {
        let read_txn = self.snapshot_db.begin_read().ok()?;
        let table = read_txn.open_table(TILE_LAYOUT_TABLE).ok()?;
        let entry = table.get("latest").ok()??;
        let decrypted = self.decode_persisted_bytes(entry.value()).ok()?;
        std::str::from_utf8(&decrypted).ok().map(|s| s.to_string())
    }

    /// Persist a named full-graph snapshot.
    pub fn save_named_graph_snapshot(
        &mut self,
        name: &str,
        graph: &Graph,
    ) -> Result<(), GraphStoreError> {
        let key = Self::named_graph_key(name)?;
        let snapshot = graph.to_snapshot();
        let plaintext = rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot)
            .map_err(|e| GraphStoreError::Io(format!("Failed to serialize graph snapshot: {e}")))?;
        let bytes = self.encode_persisted_bytes(plaintext.as_ref())?;
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            table
                .insert(key.as_str(), bytes.as_slice())
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    /// Load a named full-graph snapshot if present.
    pub fn load_named_graph_snapshot(&self, name: &str) -> Option<Graph> {
        let key = Self::named_graph_key(name).ok()?;
        let read_txn = self.snapshot_db.begin_read().ok()?;
        let table = read_txn.open_table(SNAPSHOT_TABLE).ok()?;
        let entry = table.get(key.as_str()).ok()??;
        let bytes = self.decode_persisted_bytes(entry.value()).ok()?;
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(&bytes);
        let snapshot = rkyv::from_bytes::<GraphSnapshot, rkyv::rancor::Error>(&aligned).ok()?;
        Some(Graph::from_snapshot(&snapshot))
    }

    /// List named graph snapshots in stable order.
    pub fn list_named_graph_snapshot_names(&self) -> Vec<String> {
        let Ok(read_txn) = self.snapshot_db.begin_read() else {
            return Vec::new();
        };
        let Ok(table) = read_txn.open_table(SNAPSHOT_TABLE) else {
            return Vec::new();
        };
        let Ok(iter) = table.iter() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in iter {
            if let Ok((key, _)) = entry {
                let key = key.value();
                if let Some(stripped) = key.strip_prefix(NAMED_GRAPH_PREFIX) {
                    names.push(stripped.to_string());
                }
            }
        }
        names.sort();
        names
    }

    /// Delete a named full-graph snapshot.
    pub fn delete_named_graph_snapshot(&mut self, name: &str) -> Result<(), GraphStoreError> {
        let key = Self::named_graph_key(name)?;
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(SNAPSHOT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            let _ = table
                .remove(key.as_str())
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    /// Persist serialized tile layout JSON under a workspace name.
    pub fn save_workspace_layout_json(
        &mut self,
        name: &str,
        layout_json: &str,
    ) -> Result<(), GraphStoreError> {
        if name.trim().is_empty() {
            return Err(GraphStoreError::Io(
                "Workspace name must not be empty".to_string(),
            ));
        }
        let encrypted = self.encode_persisted_bytes(layout_json.as_bytes())?;
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(TILE_LAYOUT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            table
                .insert(name, encrypted.as_slice())
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    /// Load serialized tile layout JSON by workspace name.
    pub fn load_workspace_layout_json(&self, name: &str) -> Option<String> {
        let read_txn = self.snapshot_db.begin_read().ok()?;
        let table = read_txn.open_table(TILE_LAYOUT_TABLE).ok()?;
        let entry = table.get(name).ok()??;
        let decrypted = self.decode_persisted_bytes(entry.value()).ok()?;
        std::str::from_utf8(&decrypted).ok().map(|s| s.to_string())
    }

    /// List saved workspace layout names in stable order.
    pub fn list_workspace_layout_names(&self) -> Vec<String> {
        let Ok(read_txn) = self.snapshot_db.begin_read() else {
            return Vec::new();
        };
        let Ok(table) = read_txn.open_table(TILE_LAYOUT_TABLE) else {
            return Vec::new();
        };
        let Ok(iter) = table.iter() else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for entry in iter {
            if let Ok((key, _)) = entry {
                names.push(key.value().to_string());
            }
        }
        names.sort();
        names
    }

    /// Delete a workspace layout by name.
    pub fn delete_workspace_layout(&mut self, name: &str) -> Result<(), GraphStoreError> {
        let write_txn = self
            .snapshot_db
            .begin_write()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        {
            let mut table = write_txn
                .open_table(TILE_LAYOUT_TABLE)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
            let _ = table
                .remove(name)
                .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        }
        write_txn
            .commit()
            .map_err(|e| GraphStoreError::Redb(format!("{e}")))?;
        Ok(())
    }

    fn load_snapshot(&self) -> Option<GraphSnapshot> {
        let read_txn = self.snapshot_db.begin_read().ok()?;
        let table = read_txn.open_table(SNAPSHOT_TABLE).ok()?;
        let entry = table.get("latest").ok()??;
        let bytes = self.decode_persisted_bytes(entry.value()).ok()?;

        // Copy to aligned buffer — redb bytes may not satisfy rkyv alignment
        let mut aligned = rkyv::util::AlignedVec::<16>::new();
        aligned.extend_from_slice(&bytes);

        rkyv::from_bytes::<GraphSnapshot, rkyv::rancor::Error>(&aligned).ok()
    }

    fn replay_log(&self, graph: &mut Graph) {
        use types::ArchivedLogEntry;

        for guard in self.log_keyspace.iter() {
            let (_, value) = match guard.into_inner() {
                Ok(kv) => kv,
                Err(_) => continue,
            };
            let decoded = match self.decode_persisted_bytes(value.as_ref()) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let archived = match rkyv::access::<ArchivedLogEntry, rkyv::rancor::Error>(&decoded) {
                Ok(a) => a,
                Err(_) => continue,
            };

            match archived {
                ArchivedLogEntry::AddNode {
                    node_id,
                    url,
                    position_x,
                    position_y,
                } => {
                    let Ok(node_id) = Uuid::parse_str(node_id.as_str()) else {
                        continue;
                    };
                    if graph.get_node_key_by_id(node_id).is_none() {
                        let px: f32 = (*position_x).into();
                        let py: f32 = (*position_y).into();
                        graph.add_node_with_id(
                            node_id,
                            url.to_string(),
                            euclid::default::Point2D::new(px, py),
                        );
                    }
                },
                ArchivedLogEntry::AddEdge {
                    from_node_id,
                    to_node_id,
                    edge_type,
                } => {
                    let Ok(from_node_id) = Uuid::parse_str(from_node_id.as_str()) else {
                        continue;
                    };
                    let Ok(to_node_id) = Uuid::parse_str(to_node_id.as_str()) else {
                        continue;
                    };
                    let from = graph.get_node_key_by_id(from_node_id);
                    let to = graph.get_node_key_by_id(to_node_id);
                    if let (Some(from_key), Some(to_key)) = (from, to) {
                        let et = match edge_type {
                            types::ArchivedPersistedEdgeType::Hyperlink => {
                                crate::graph::EdgeType::Hyperlink
                            },
                            types::ArchivedPersistedEdgeType::History => {
                                crate::graph::EdgeType::History
                            },
                            types::ArchivedPersistedEdgeType::UserGrouped => {
                                crate::graph::EdgeType::UserGrouped
                            },
                        };
                        graph.add_edge(from_key, to_key, et);
                    }
                },
                ArchivedLogEntry::RemoveEdge {
                    from_node_id,
                    to_node_id,
                    edge_type,
                } => {
                    let Ok(from_node_id) = Uuid::parse_str(from_node_id.as_str()) else {
                        continue;
                    };
                    let Ok(to_node_id) = Uuid::parse_str(to_node_id.as_str()) else {
                        continue;
                    };
                    let from = graph.get_node_key_by_id(from_node_id);
                    let to = graph.get_node_key_by_id(to_node_id);
                    if let (Some(from_key), Some(to_key)) = (from, to) {
                        let et = match edge_type {
                            types::ArchivedPersistedEdgeType::Hyperlink => {
                                crate::graph::EdgeType::Hyperlink
                            },
                            types::ArchivedPersistedEdgeType::History => {
                                crate::graph::EdgeType::History
                            },
                            types::ArchivedPersistedEdgeType::UserGrouped => {
                                crate::graph::EdgeType::UserGrouped
                            },
                        };
                        let _ = graph.remove_edges(from_key, to_key, et);
                    }
                },
                ArchivedLogEntry::UpdateNodeTitle { node_id, title } => {
                    let Ok(node_id) = Uuid::parse_str(node_id.as_str()) else {
                        continue;
                    };
                    if let Some(key) = graph.get_node_key_by_id(node_id)
                        && let Some(node_mut) = graph.get_node_mut(key)
                    {
                        node_mut.title = title.to_string();
                    }
                },
                ArchivedLogEntry::PinNode { node_id, is_pinned } => {
                    let Ok(node_id) = Uuid::parse_str(node_id.as_str()) else {
                        continue;
                    };
                    if let Some(key) = graph.get_node_key_by_id(node_id)
                        && let Some(node_mut) = graph.get_node_mut(key)
                    {
                        node_mut.is_pinned = *is_pinned;
                    }
                },
                ArchivedLogEntry::RemoveNode { node_id } => {
                    let Ok(node_id) = Uuid::parse_str(node_id.as_str()) else {
                        continue;
                    };
                    if let Some(key) = graph.get_node_key_by_id(node_id) {
                        graph.remove_node(key);
                    }
                },
                ArchivedLogEntry::ClearGraph => {
                    *graph = Graph::new();
                },
                ArchivedLogEntry::UpdateNodeUrl { node_id, new_url } => {
                    let Ok(node_id) = Uuid::parse_str(node_id.as_str()) else {
                        continue;
                    };
                    if let Some(key) = graph.get_node_key_by_id(node_id) {
                        graph.update_node_url(key, new_url.to_string());
                    }
                },
            }
        }
    }

    fn clear_log(&mut self) {
        let keys: Vec<Vec<u8>> = self
            .log_keyspace
            .iter()
            .filter_map(|guard| guard.key().ok().map(|k| k.to_vec()))
            .collect();
        for key in keys {
            let _ = self.log_keyspace.remove(key);
        }
        self.log_sequence = 0;
    }

    fn find_max_sequence(keyspace: &fjall::Keyspace) -> u64 {
        let mut max = 0u64;
        for guard in keyspace.iter() {
            if let Ok(key_bytes) = guard.key() {
                if key_bytes.len() == 8 {
                    let seq = u64::from_be_bytes(key_bytes.as_ref().try_into().unwrap_or([0u8; 8]));
                    max = max.max(seq);
                }
            }
        }
        max
    }

    /// Get the default storage directory for graph data
    pub fn default_data_dir() -> PathBuf {
        let mut dir = dirs::config_dir().expect("No config directory available");
        dir.push("graphshell");
        dir.push("graphs");
        dir
    }
}

/// Errors from the graph store
#[derive(Debug)]
#[cfg_attr(test, allow(dead_code))]
pub enum GraphStoreError {
    Io(String),
    Fjall(String),
    Redb(String),
    Key(String),
    Crypto(String),
    Compression(String),
}

impl std::fmt::Display for GraphStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphStoreError::Io(e) => write!(f, "IO error: {e}"),
            GraphStoreError::Fjall(e) => write!(f, "Fjall error: {e}"),
            GraphStoreError::Redb(e) => write!(f, "Redb error: {e}"),
            GraphStoreError::Key(e) => write!(f, "Key error: {e}"),
            GraphStoreError::Crypto(e) => write!(f, "Crypto error: {e}"),
            GraphStoreError::Compression(e) => write!(f, "Compression error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeType;
    use euclid::default::Point2D;
    use std::fs;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn create_test_store() -> (GraphStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = GraphStore::open(dir.path().to_path_buf()).unwrap();
        (store, dir)
    }

    #[test]
    fn test_empty_startup() {
        let (store, _dir) = create_test_store();
        let recovered = store.recover();
        assert!(recovered.is_none());
    }

    #[test]
    fn test_log_and_recover() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_a.to_string(),
                url: "https://a.com".to_string(),
                position_x: 10.0,
                position_y: 20.0,
            });
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_b.to_string(),
                url: "https://b.com".to_string(),
                position_x: 30.0,
                position_y: 40.0,
            });
            store.log_mutation(&LogEntry::AddEdge {
                from_node_id: id_a.to_string(),
                to_node_id: id_b.to_string(),
                edge_type: types::PersistedEdgeType::Hyperlink,
            });
        }

        {
            let store = GraphStore::open(path).unwrap();
            let graph = store.recover().unwrap();
            assert_eq!(graph.node_count(), 2);
            assert_eq!(graph.edge_count(), 1);

            let (_, a) = graph.get_node_by_url("https://a.com").unwrap();
            assert_eq!(a.position.x, 10.0);
            assert_eq!(a.position.y, 20.0);
        }
    }

    #[test]
    fn test_log_and_recover_user_grouped_edge() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_a.to_string(),
                url: "https://a.com".to_string(),
                position_x: 10.0,
                position_y: 20.0,
            });
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_b.to_string(),
                url: "https://b.com".to_string(),
                position_x: 30.0,
                position_y: 40.0,
            });
            store.log_mutation(&LogEntry::AddEdge {
                from_node_id: id_a.to_string(),
                to_node_id: id_b.to_string(),
                edge_type: types::PersistedEdgeType::UserGrouped,
            });
        }

        {
            let store = GraphStore::open(path).unwrap();
            let graph = store.recover().unwrap();
            let has_user_grouped = graph.edges().any(|e| e.edge_type == EdgeType::UserGrouped);
            assert!(has_user_grouped);
        }
    }

    #[test]
    fn test_log_remove_edge_recover() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_a.to_string(),
                url: "https://a.com".to_string(),
                position_x: 10.0,
                position_y: 20.0,
            });
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_b.to_string(),
                url: "https://b.com".to_string(),
                position_x: 30.0,
                position_y: 40.0,
            });
            store.log_mutation(&LogEntry::AddEdge {
                from_node_id: id_a.to_string(),
                to_node_id: id_b.to_string(),
                edge_type: types::PersistedEdgeType::UserGrouped,
            });
            store.log_mutation(&LogEntry::RemoveEdge {
                from_node_id: id_a.to_string(),
                to_node_id: id_b.to_string(),
                edge_type: types::PersistedEdgeType::UserGrouped,
            });
        }

        {
            let store = GraphStore::open(path).unwrap();
            let graph = store.recover().unwrap();
            let has_user_grouped = graph.edges().any(|e| e.edge_type == EdgeType::UserGrouped);
            assert!(!has_user_grouped);
        }
    }

    #[test]
    fn test_snapshot_and_recover() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            let mut graph = Graph::new();
            graph.add_node("https://a.com".to_string(), Point2D::new(100.0, 200.0));
            graph.add_node("https://b.com".to_string(), Point2D::new(300.0, 400.0));
            let (n1, _) = graph.get_node_by_url("https://a.com").unwrap();
            let (n2, _) = graph.get_node_by_url("https://b.com").unwrap();
            graph.add_edge(n1, n2, EdgeType::Hyperlink);

            store.take_snapshot(&graph);
        }

        {
            let store = GraphStore::open(path).unwrap();
            let graph = store.recover().unwrap();
            assert_eq!(graph.node_count(), 2);
            assert_eq!(graph.edge_count(), 1);
        }
    }

    #[test]
    fn test_snapshot_plus_log_recovery() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            let mut graph = Graph::new();
            graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
            store.take_snapshot(&graph);

            let id_b = Uuid::new_v4();
            store.log_mutation(&LogEntry::AddNode {
                node_id: id_b.to_string(),
                url: "https://b.com".to_string(),
                position_x: 50.0,
                position_y: 50.0,
            });
        }

        {
            let store = GraphStore::open(path).unwrap();
            let graph = store.recover().unwrap();
            assert_eq!(graph.node_count(), 2);
            assert!(graph.get_node_by_url("https://a.com").is_some());
            assert!(graph.get_node_by_url("https://b.com").is_some());
        }
    }

    #[test]
    fn test_duplicate_url_supported_with_distinct_ids() {
        let (mut store, _dir) = create_test_store();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        store.log_mutation(&LogEntry::AddNode {
            node_id: id_a.to_string(),
            url: "https://a.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::AddNode {
            node_id: id_b.to_string(),
            url: "https://a.com".to_string(),
            position_x: 100.0,
            position_y: 100.0,
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 2);
    }

    #[test]
    fn test_log_title_update() {
        let (mut store, _dir) = create_test_store();
        let id = Uuid::new_v4();

        store.log_mutation(&LogEntry::AddNode {
            node_id: id.to_string(),
            url: "https://a.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::UpdateNodeTitle {
            node_id: id.to_string(),
            title: "My Site".to_string(),
        });

        let graph = store.recover().unwrap();
        let (_, node) = graph.get_node_by_url("https://a.com").unwrap();
        assert_eq!(node.title, "My Site");
    }

    #[test]
    fn test_log_remove_node_recover() {
        let (mut store, _dir) = create_test_store();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        store.log_mutation(&LogEntry::AddNode {
            node_id: id_a.to_string(),
            url: "https://a.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::AddNode {
            node_id: id_b.to_string(),
            url: "https://b.com".to_string(),
            position_x: 100.0,
            position_y: 100.0,
        });
        store.log_mutation(&LogEntry::RemoveNode {
            node_id: id_a.to_string(),
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_url("https://a.com").is_none());
        assert!(graph.get_node_by_url("https://b.com").is_some());
    }

    #[test]
    fn test_log_clear_graph_recover() {
        let (mut store, _dir) = create_test_store();

        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://a.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://b.com".to_string(),
            position_x: 100.0,
            position_y: 100.0,
        });
        store.log_mutation(&LogEntry::ClearGraph);

        let recovered = store.recover();
        assert!(recovered.is_none()); // Empty graph returns None
    }

    #[test]
    fn test_log_clear_then_add_recover() {
        let (mut store, _dir) = create_test_store();

        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://old.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::ClearGraph);
        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://new.com".to_string(),
            position_x: 50.0,
            position_y: 50.0,
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_url("https://old.com").is_none());
        assert!(graph.get_node_by_url("https://new.com").is_some());
    }

    #[test]
    fn test_log_update_node_url_recover() {
        let (mut store, _dir) = create_test_store();
        let id = Uuid::new_v4();

        store.log_mutation(&LogEntry::AddNode {
            node_id: id.to_string(),
            url: "https://old.com".to_string(),
            position_x: 10.0,
            position_y: 20.0,
        });
        store.log_mutation(&LogEntry::UpdateNodeUrl {
            node_id: id.to_string(),
            new_url: "https://new.com".to_string(),
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_url("https://old.com").is_none());
        let (_, node) = graph.get_node_by_url("https://new.com").unwrap();
        assert_eq!(node.position.x, 10.0);
        assert_eq!(node.position.y, 20.0);
    }

    #[test]
    fn test_uuid_log_replay_resolves_by_id_not_url() {
        let (mut store, _dir) = create_test_store();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        store.log_mutation(&LogEntry::AddNode {
            node_id: id_a.to_string(),
            url: "https://same.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::AddNode {
            node_id: id_b.to_string(),
            url: "https://same.com".to_string(),
            position_x: 100.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::UpdateNodeUrl {
            node_id: id_a.to_string(),
            new_url: "https://updated-a.com".to_string(),
        });

        let graph = store.recover().unwrap();
        let (_, node_a) = graph.get_node_by_id(id_a).unwrap();
        let (_, node_b) = graph.get_node_by_id(id_b).unwrap();
        assert_eq!(node_a.url, "https://updated-a.com");
        assert_eq!(node_b.url, "https://same.com");
    }

    #[test]
    fn test_remove_nonexistent_node_noop() {
        let (mut store, _dir) = create_test_store();

        let id = Uuid::new_v4();
        store.log_mutation(&LogEntry::AddNode {
            node_id: id.to_string(),
            url: "https://a.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::RemoveNode {
            node_id: Uuid::new_v4().to_string(),
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
    }

    #[test]
    fn test_clear_all_removes_snapshot_and_log() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let mut store = GraphStore::open(path.clone()).unwrap();
            let mut graph = Graph::new();
            graph.add_node("https://a.com".to_string(), Point2D::new(0.0, 0.0));
            store.take_snapshot(&graph);
            store.log_mutation(&LogEntry::AddNode {
                node_id: Uuid::new_v4().to_string(),
                url: "https://b.com".to_string(),
                position_x: 10.0,
                position_y: 20.0,
            });
            store.clear_all().unwrap();
        }

        {
            let store = GraphStore::open(path).unwrap();
            assert!(store.recover().is_none());
        }
    }

    #[test]
    fn test_tile_layout_roundtrip() {
        let (mut store, _dir) = create_test_store();
        let layout = r#"{"root":null,"tiles":{}}"#;
        store.save_tile_layout_json(layout).unwrap();
        let loaded = store.load_tile_layout_json().unwrap();
        assert_eq!(loaded, layout);
    }

    #[test]
    fn test_clear_all_removes_tile_layout() {
        let (mut store, _dir) = create_test_store();
        store
            .save_tile_layout_json(r#"{"root":null,"tiles":{}}"#)
            .unwrap();
        assert!(store.load_tile_layout_json().is_some());
        store.clear_all().unwrap();
        assert!(store.load_tile_layout_json().is_none());
    }

    #[test]
    fn test_named_workspace_layout_roundtrip_and_list_delete() {
        let (mut store, _dir) = create_test_store();
        store
            .save_workspace_layout_json("workspace-a", r#"{"root":"a"}"#)
            .unwrap();
        store
            .save_workspace_layout_json("workspace-b", r#"{"root":"b"}"#)
            .unwrap();

        assert_eq!(
            store.load_workspace_layout_json("workspace-a").as_deref(),
            Some(r#"{"root":"a"}"#)
        );
        assert_eq!(
            store.load_workspace_layout_json("workspace-b").as_deref(),
            Some(r#"{"root":"b"}"#)
        );

        let names = store.list_workspace_layout_names();
        assert!(names.contains(&"workspace-a".to_string()));
        assert!(names.contains(&"workspace-b".to_string()));

        store.delete_workspace_layout("workspace-a").unwrap();
        assert!(store.load_workspace_layout_json("workspace-a").is_none());
        assert!(store.load_workspace_layout_json("workspace-b").is_some());
    }

    #[test]
    fn test_named_graph_snapshot_roundtrip_and_list_delete() {
        let (mut store, _dir) = create_test_store();
        let mut graph_a = Graph::new();
        graph_a.add_node("https://a.example".to_string(), Point2D::new(1.0, 2.0));
        let mut graph_b = Graph::new();
        graph_b.add_node("https://b.example".to_string(), Point2D::new(3.0, 4.0));

        store
            .save_named_graph_snapshot("graph-a", &graph_a)
            .unwrap();
        store
            .save_named_graph_snapshot("graph-b", &graph_b)
            .unwrap();

        let loaded_a = store.load_named_graph_snapshot("graph-a").unwrap();
        let loaded_b = store.load_named_graph_snapshot("graph-b").unwrap();
        assert!(loaded_a.get_node_by_url("https://a.example").is_some());
        assert!(loaded_b.get_node_by_url("https://b.example").is_some());

        let names = store.list_named_graph_snapshot_names();
        assert_eq!(names, vec!["graph-a".to_string(), "graph-b".to_string()]);

        store.delete_named_graph_snapshot("graph-a").unwrap();
        assert!(store.load_named_graph_snapshot("graph-a").is_none());
        assert!(store.load_named_graph_snapshot("graph-b").is_some());
    }

    #[test]
    fn test_set_snapshot_interval_secs() {
        let (mut store, _dir) = create_test_store();
        store.set_snapshot_interval_secs(42).unwrap();
        assert_eq!(store.snapshot_interval_secs(), 42);
    }

    #[test]
    fn test_set_snapshot_interval_secs_rejects_zero() {
        let (mut store, _dir) = create_test_store();
        assert!(store.set_snapshot_interval_secs(0).is_err());
        assert_eq!(
            store.snapshot_interval_secs(),
            DEFAULT_SNAPSHOT_INTERVAL_SECS
        );
    }

    #[test]
    fn test_roundtrip_encrypt_decrypt() {
        let (store, _dir) = create_test_store();
        let payload = b"hello graphshell encrypted world";
        let encrypted = store.encode_persisted_bytes(payload).unwrap();
        let decrypted = store.decode_persisted_bytes(&encrypted).unwrap();
        assert_eq!(decrypted, payload);
        assert!(encrypted.starts_with(ENCRYPTED_PAYLOAD_MAGIC));
    }

    #[test]
    fn test_tampered_ciphertext_rejected() {
        let (store, _dir) = create_test_store();
        let payload = b"important payload";
        let mut encrypted = store.encode_persisted_bytes(payload).unwrap();
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(store.decode_persisted_bytes(&encrypted).is_err());
    }

    #[test]
    fn test_key_not_stored_in_data_dir() {
        let (store, dir) = create_test_store();
        drop(store);
        let test_key = [0xA5; 32];
        let mut stack = vec![dir.path().to_path_buf()];
        while let Some(next) = stack.pop() {
            for entry in fs::read_dir(next).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let bytes = fs::read(path).unwrap();
                assert!(
                    !bytes
                        .windows(test_key.len())
                        .any(|window| window == test_key),
                    "Found raw key bytes in persisted file"
                );
            }
        }
    }

    #[test]
    fn test_migration_detects_unencrypted_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        {
            let store = GraphStore::open(path.clone()).unwrap();
            let mut graph = Graph::new();
            graph.add_node("https://legacy.example".to_string(), Point2D::new(0.0, 0.0));
            let snapshot = graph.to_snapshot();
            let plaintext = rkyv::to_bytes::<rkyv::rancor::Error>(&snapshot).unwrap();
            let write_txn = store.snapshot_db.begin_write().unwrap();
            {
                let mut table = write_txn.open_table(SNAPSHOT_TABLE).unwrap();
                table.insert("latest", plaintext.as_ref()).unwrap();
            }
            write_txn.commit().unwrap();

            assert!(store.has_legacy_plaintext_data());
        }

        let store = GraphStore::open(path).unwrap();
        assert!(!store.has_legacy_plaintext_data());
        let snapshot = store.load_snapshot().unwrap();
        assert_eq!(snapshot.nodes.len(), 1);
    }

    #[test]
    fn test_log_output_is_not_plaintext() {
        let (mut store, _dir) = create_test_store();
        let entry = LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://plaintext-check.example".to_string(),
            position_x: 1.0,
            position_y: 2.0,
        };
        let plaintext = rkyv::to_bytes::<rkyv::rancor::Error>(&entry).unwrap();
        store.log_mutation(&entry);

        let mut found_any = false;
        for guard in store.log_keyspace.iter() {
            let (_, value) = guard.into_inner().unwrap();
            found_any = true;
            assert_ne!(value.as_ref(), plaintext.as_ref());
            assert!(value.as_ref().starts_with(ENCRYPTED_PAYLOAD_MAGIC));
        }
        assert!(found_any);
    }

    #[test]
    fn test_recover_ignores_corrupt_log_entries() {
        let (mut store, _dir) = create_test_store();
        let valid_id = Uuid::new_v4();
        store.log_mutation(&LogEntry::AddNode {
            node_id: valid_id.to_string(),
            url: "https://valid.com".to_string(),
            position_x: 1.0,
            position_y: 2.0,
        });
        // Append an invalid rkyv payload directly to the log.
        let corrupt_key = 99u64.to_be_bytes();
        store.log_keyspace.insert(corrupt_key, b"not-rkyv").unwrap();

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_id(valid_id).is_some());
    }

    #[test]
    fn test_recover_skips_invalid_uuid_log_entries() {
        let (mut store, _dir) = create_test_store();
        store.log_mutation(&LogEntry::AddNode {
            node_id: "not-a-uuid".to_string(),
            url: "https://bad.com".to_string(),
            position_x: 0.0,
            position_y: 0.0,
        });
        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://good.com".to_string(),
            position_x: 3.0,
            position_y: 4.0,
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_url("https://good.com").is_some());
        assert!(graph.get_node_by_url("https://bad.com").is_none());
    }

    #[test]
    fn test_recover_with_corrupt_snapshot_replays_log_only() {
        let (mut store, _dir) = create_test_store();
        // Write an invalid snapshot payload.
        {
            let write_txn = store.snapshot_db.begin_write().unwrap();
            {
                let mut table = write_txn.open_table(SNAPSHOT_TABLE).unwrap();
                table.insert("latest", &b"corrupt-snapshot"[..]).unwrap();
            }
            write_txn.commit().unwrap();
        }
        // Valid log entry should still recover.
        store.log_mutation(&LogEntry::AddNode {
            node_id: Uuid::new_v4().to_string(),
            url: "https://from-log.com".to_string(),
            position_x: 9.0,
            position_y: 9.0,
        });

        let graph = store.recover().unwrap();
        assert_eq!(graph.node_count(), 1);
        assert!(graph.get_node_by_url("https://from-log.com").is_some());
    }

    #[test]
    fn test_recover_with_corrupt_snapshot_and_empty_log_returns_none() {
        let (store, _dir) = create_test_store();
        {
            let write_txn = store.snapshot_db.begin_write().unwrap();
            {
                let mut table = write_txn.open_table(SNAPSHOT_TABLE).unwrap();
                table.insert("latest", &b"corrupt-snapshot"[..]).unwrap();
            }
            write_txn.commit().unwrap();
        }
        assert!(store.recover().is_none());
    }

    #[test]
    #[ignore]
    fn perf_snapshot_and_recover_5k_nodes_under_budget() {
        let (mut store, _dir) = create_test_store();
        let mut graph = Graph::new();
        for i in 0..5000 {
            let _ = graph.add_node(
                format!("https://example.com/{i}"),
                Point2D::new(i as f32, (i % 200) as f32),
            );
        }

        let start = std::time::Instant::now();
        store.take_snapshot(&graph);
        let recovered = store.recover();
        let elapsed = start.elapsed();
        assert!(recovered.is_some());
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "snapshot+recover exceeded budget: {elapsed:?}"
        );
    }
}
