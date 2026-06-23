/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Mutable scripted-DOM provider.
//!
//! `ScriptedDom` is the mutable sibling of `serval-static-dom`'s `StaticDocument`:
//! a `NodeId`-keyed arena that implements [`LayoutDom`] (read) and [`LayoutDomMut`]
//! (mutate), recording each structural change as a [`DomMutation`] for
//! serval-layout's scheduler to translate into invalidation. The arena owns the
//! node data; JS reflectors bridge back to it by `NodeId` (via
//! `script-engine-api`'s `make_reflector`/`reflector_data`), so the engine never
//! owns DOM data.
//!
//! Scope (2026-05-23): structural mutation + the mutation stream. The reflector
//! bridge wiring and the DomMutation → serval-layout invalidation loop are the next
//! pass (they need the `script-runtime-api` host layer and serval-layout's
//! scheduler). `set_inner_html` is deferred — it needs html5ever fragment parsing.

#![deny(unsafe_code)]

use layout_dom_api::{
    AttributeView, DomMutation, LayoutDom, LayoutDomMut, LocalName, Namespace, NodeKind, QualName,
};
use serval_static_dom::{StaticDocument, StaticNodeId};

mod forms;
mod serialize;

/// Opaque node identity: a stable index into the arena (slots are never reused, so
/// ids stay valid for the document's lifetime).
// `usize`-backed (pointer-sized): serval-layout's Stylo style-sharing cache asserts
// `size_of::<NodeId>() == size_of::<usize>()` (it packs the id into a pointer-shaped
// `OpaqueElement`). A `u32` would fail that assertion.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(usize);

impl NodeId {
    /// The raw arena index. The reflector bridge packs this into a
    /// `script_engine_api::ReflectorData` (`u64`) so JS can carry it opaquely.
    pub fn raw(self) -> usize {
        self.0
    }

    /// Rebuild a `NodeId` from a raw index recovered from a reflector.
    pub fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

// --- G0: the document fence -------------------------------------------------
//
// Live `ScriptedDom`s are multiplying (chrome, workbench, roster, panes,
// cards, windows). A `NodeId` minted by one document and used against another
// is a silent wrong-node bug. To catch it, each document carries a
// process-unique `doc_tag`; on 64-bit *debug* builds the tag is packed into a
// `NodeId`'s high bits and every accessor `debug_assert`s ownership.
//
// On release and on wasm32 the packing and the asserts compile out entirely,
// so ids are the bare arena index exactly as before and behavior is
// byte-identical. wasm32 has no room to pack (a `usize` is 32 bits there, all
// of it spoken for by the index), and native debug runs already exercise the
// bug class. The packed value rides opaquely through Stylo's `OpaqueElement`
// (never dereferenced) and through the reflector bridge's `raw()`/`from_raw()`
// u64 round-trip, so the tag survives both paths and the assert fires on
// reconstructed ids too.

#[cfg(all(debug_assertions, target_pointer_width = "64"))]
mod fence {
    /// Low bits of a `NodeId` carrying the arena index. 2^48 nodes per
    /// document is ample; the remaining 16 high bits carry the `doc_tag`.
    pub const INDEX_BITS: u32 = 48;
    pub const INDEX_MASK: usize = (1usize << INDEX_BITS) - 1;
    /// 16-bit tag space. On wraparound (65k+ documents in one process) tags
    /// may alias and the assert weakens to a heuristic; it never miscompares a
    /// same-tag id, so correctness is unaffected.
    pub const TAG_MASK: u64 = (1u64 << (64 - INDEX_BITS)) - 1;

    /// Mint a process-unique tag. Starts at 1 so the first document is nonzero.
    pub fn next_doc_tag() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        COUNTER.fetch_add(1, Ordering::Relaxed) & TAG_MASK
    }
}

/// A tiny deterministic FNV-1a hasher for the node store (G3). std `HashMap`'s
/// `RandomState` is seed-dependent (and its seed is an entropy question on
/// wasm32); fixing the hasher makes the store's iteration order identical on
/// every run and target, so pelt-live's byte-determinism stays airtight even
/// though the store is now a map rather than a dense slab.
#[derive(Default)]
struct FnvHasher(u64);

impl std::hash::Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 { 0xcbf2_9ce4_8422_2325 } else { self.0 };
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// The node store: a *prunable* map from a monotonic node value to its [`Node`].
/// Keyed by the untagged id value (the low bits the fence's `index` returns), so
/// the G0 doc-tag never reaches the key. Pruning dead entries is what bounds
/// memory to live nodes (G3); the deterministic hasher keeps it order-stable.
type NodeStore = std::collections::HashMap<usize, Node, std::hash::BuildHasherDefault<FnvHasher>>;

struct Node {
    kind: NodeKind,
    name: Option<QualName>,
    attrs: Vec<(QualName, String)>,
    text: Option<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
}

impl Node {
    fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            name: None,
            attrs: Vec::new(),
            text: None,
            parent: None,
            children: Vec::new(),
        }
    }
}

/// A mutable DOM arena. Nodes live in a prunable [`NodeStore`] keyed by a
/// monotonic id; a pruned id is "gone" (ids are never reused). Memory is bounded
/// to *live* nodes by [`collect`](ScriptedDom::collect) (G3), not to every node
/// ever created.
pub struct ScriptedDom {
    nodes: NodeStore,
    /// Monotonic id counter. The next node's untagged value; never decremented,
    /// so ids are never reused even as the store is pruned.
    next_id: usize,
    /// The primary document root (returned by `document()`) — the sole permanent
    /// mark root for `collect`. Everything else (secondary documents, fragments,
    /// detached subtrees) survives only via reachability from here or the host's
    /// pins, so a dropped secondary document collects like any other orphan.
    root: NodeId,
    mutations: Vec<DomMutation<NodeId>>,
    /// Process-unique document tag (G0 fence). Only present where the fence is
    /// active; elsewhere ids are untagged and this field would be dead weight.
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    doc_tag: u64,
}

impl Default for ScriptedDom {
    fn default() -> Self {
        Self::new()
    }
}

/// A set of node ids to treat as **extra mark roots** for
/// [`ScriptedDom::collect`] — nodes the document tree no longer reaches but that
/// must survive anyway. The host pins a node while script can still reach it (it
/// holds a reflector), so a pinned orphan and its whole connected component are
/// spared; unpinning it makes it collectable. The DOM only sees a pin set; the
/// host's word for these is "reflector pins" (it `pin`s on minting a reflector
/// and `retire`s the ones the engine reports dead). Engine-agnostic — it traffics
/// only in [`NodeId`], naming no engine type.
#[derive(Debug, Default, Clone)]
pub struct Pins {
    pinned: std::collections::HashSet<NodeId>,
}

impl Pins {
    /// An empty pin set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin `id` (script can still reach it). Idempotent.
    pub fn pin(&mut self, id: NodeId) {
        self.pinned.insert(id);
    }

    /// Drop the pin for `id`. Returns `true` if it had been pinned (an id never
    /// pinned here is a harmless no-op).
    pub fn unpin(&mut self, id: NodeId) -> bool {
        self.pinned.remove(&id)
    }

    /// Retire a batch of now-dead ids (the host maps the engine's
    /// `drain_dead_reflectors` output through `NodeId::from_raw` first),
    /// unpinning each. Returns how many were actually pinned.
    pub fn retire_dead(&mut self, dead: impl IntoIterator<Item = NodeId>) -> usize {
        dead.into_iter().filter(|id| self.pinned.remove(id)).count()
    }

    /// Whether `id` is currently pinned.
    pub fn is_pinned(&self, id: NodeId) -> bool {
        self.pinned.contains(&id)
    }

    /// The pinned ids — pass to [`ScriptedDom::collect`] as the extra roots.
    pub fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.pinned.iter().copied()
    }

    /// Number of pinned ids.
    pub fn len(&self) -> usize {
        self.pinned.len()
    }

    /// Whether nothing is pinned.
    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty()
    }

    /// Drop every pin (document teardown).
    pub fn clear(&mut self) {
        self.pinned.clear();
    }
}

impl ScriptedDom {
    /// A fresh document with an empty `Document` root.
    pub fn new() -> Self {
        let mut dom = Self {
            nodes: NodeStore::default(),
            next_id: 0,
            // Placeholder; overwritten by the `push` below so the root id
            // carries this document's tag like every other node.
            root: NodeId(0),
            mutations: Vec::new(),
            #[cfg(all(debug_assertions, target_pointer_width = "64"))]
            doc_tag: fence::next_doc_tag(),
        };
        dom.root = dom.push(Node::new(NodeKind::Document));
        dom
    }

    /// Pack a monotonic id value into a `NodeId`, tagging it with this document
    /// on a fenced build. Off the fence this is the bare value (today's behavior).
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    fn pack(&self, value: usize) -> NodeId {
        debug_assert!(value <= fence::INDEX_MASK, "scripted-dom node id overflow");
        NodeId((((self.doc_tag & fence::TAG_MASK) << fence::INDEX_BITS) as usize) | value)
    }
    #[cfg(not(all(debug_assertions, target_pointer_width = "64")))]
    fn pack(&self, value: usize) -> NodeId {
        NodeId(value)
    }

    /// Resolve a `NodeId` to its store key (the untagged id value), asserting it
    /// belongs to this document first. Every accessor that reads the store goes
    /// through here.
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    fn index(&self, id: NodeId) -> usize {
        debug_assert!(
            (id.0 >> fence::INDEX_BITS) as u64 == (self.doc_tag & fence::TAG_MASK),
            "NodeId from a different document (id tag {}, this doc {})",
            (id.0 >> fence::INDEX_BITS) as u64,
            self.doc_tag & fence::TAG_MASK,
        );
        id.0 & fence::INDEX_MASK
    }
    #[cfg(not(all(debug_assertions, target_pointer_width = "64")))]
    #[inline]
    fn index(&self, id: NodeId) -> usize {
        id.0
    }

    /// Like [`index`](Self::index) but **non-asserting**: returns `None` for an
    /// id minted by a different document (on a fenced build) instead of
    /// panicking, so [`is_live`](Self::is_live) can answer for any id.
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    fn try_index(&self, id: NodeId) -> Option<usize> {
        if (id.0 >> fence::INDEX_BITS) as u64 == (self.doc_tag & fence::TAG_MASK) {
            Some(id.0 & fence::INDEX_MASK)
        } else {
            None
        }
    }
    #[cfg(not(all(debug_assertions, target_pointer_width = "64")))]
    #[inline]
    fn try_index(&self, id: NodeId) -> Option<usize> {
        Some(id.0)
    }

    /// DOM `removeChild`: orphan `child` from its parent but keep it (and its
    /// subtree) alive and re-insertable, recording a [`DomMutation::Removed`].
    /// Unlike [`LayoutDomMut::remove`](layout_dom_api::LayoutDomMut::remove), which
    /// also drops the subtree — script may hold a reference to a removed node and
    /// re-insert it, so the scripted DOM orphans rather than frees.
    pub fn remove_child(&mut self, child: NodeId) {
        let former_parent = self.node(child).parent;
        self.detach(child);
        if let Some(former_parent) = former_parent {
            self.mutations.push(DomMutation::Removed { node: child, former_parent });
        }
    }

    /// Create a detached `Document` node (a second document, for
    /// `DOMImplementation.createDocument` / `createHTMLDocument`). Lives in the same
    /// store as the primary document, so `NodeId`s stay globally unique. It is
    /// **pin-kept**, not a permanent root (G3): while script holds a reflector to
    /// it (or anything in it) the host pins it and `collect` spares the whole
    /// component; once script drops it, it collects like any other orphan.
    pub fn create_document(&mut self) -> NodeId {
        self.push(Node::new(NodeKind::Document))
    }

    /// Create a detached `Comment` node carrying `data`.
    pub fn create_comment(&mut self, data: &str) -> NodeId {
        let mut node = Node::new(NodeKind::Comment);
        node.text = Some(data.to_owned());
        self.push(node)
    }

    /// Create a detached `DocumentFragment` node (a parentless container).
    pub fn create_fragment(&mut self) -> NodeId {
        self.push(Node::new(NodeKind::DocumentFragment))
    }

    fn node(&self, id: NodeId) -> &Node {
        self.nodes.get(&self.index(id)).expect("NodeId refers to a live node")
    }

    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        let i = self.index(id);
        self.nodes.get_mut(&i).expect("NodeId refers to a live node")
    }

    fn push(&mut self, node: Node) -> NodeId {
        let value = self.next_id;
        self.next_id += 1;
        self.nodes.insert(value, node);
        self.pack(value)
    }

    fn sibling(&self, id: NodeId, delta: isize) -> Option<NodeId> {
        let parent = self.node(id).parent?;
        let kids = &self.node(parent).children;
        let pos = kids.iter().position(|&c| c == id)?;
        let target = pos as isize + delta;
        if target < 0 {
            return None;
        }
        kids.get(target as usize).copied()
    }

    /// Unlink `child` from its current parent (no mutation recorded).
    fn detach(&mut self, child: NodeId) {
        if let Some(parent) = self.node(child).parent {
            let kids = &mut self.node_mut(parent).children;
            if let Some(pos) = kids.iter().position(|&c| c == child) {
                kids.remove(pos);
            }
        }
        self.node_mut(child).parent = None;
    }

    /// Free a node and its whole subtree (entries removed from the store).
    fn drop_subtree(&mut self, node: NodeId) {
        let children = std::mem::take(&mut self.node_mut(node).children);
        for child in children {
            self.drop_subtree(child);
        }
        let i = self.index(node);
        self.nodes.remove(&i);
    }

    /// Mark-sweep collection (G3 dangle contract). Prune every node not reachable
    /// — by **undirected** walk over parent + child edges — from a document root
    /// or one of `extra_roots`. `extra_roots` are the host's live-reflector pins
    /// ([`ReflectorPins`](../serval_scripted/struct.ReflectorPins.html)); passing
    /// them keeps a pinned orphan's whole connected component alive (JS can walk
    /// `parentNode` up and children down and re-insert any of it). The DOM stays
    /// pin-agnostic — it takes extra roots, not reflector knowledge. Returns the
    /// number of nodes pruned.
    ///
    /// Idempotent and cheap to call often: the host drives it at the
    /// `drain_mutations` boundary, on an idle tick, and right after an unpin.
    pub fn collect(&mut self, extra_roots: impl IntoIterator<Item = NodeId>) -> usize {
        // Mark: undirected reachability from the primary document root + extra
        // roots (secondary documents and fragments survive only via the pins).
        let mut marked: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut stack: Vec<usize> = Vec::new();
        self.seed_mark(self.root, &mut marked, &mut stack);
        for r in extra_roots {
            self.seed_mark(r, &mut marked, &mut stack);
        }
        while let Some(v) = stack.pop() {
            // This node's neighbours (parent + children) as owned ids, dropping
            // the store borrow before recursing.
            let neighbours: Vec<NodeId> = match self.nodes.get(&v) {
                Some(node) => node.parent.into_iter().chain(node.children.iter().copied()).collect(),
                None => continue,
            };
            for nbr in neighbours {
                self.seed_mark(nbr, &mut marked, &mut stack);
            }
        }

        // Sweep: prune every unmarked entry.
        let before = self.nodes.len();
        self.nodes.retain(|k, _| marked.contains(k));
        before - self.nodes.len()
    }

    /// The number of live nodes currently in the store. Bounded by `collect`
    /// (G3), not by total nodes ever created — a diagnostic for the host and the
    /// regression signal for the bounded-memory churn test.
    pub fn live_node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Mark `id`'s store key live (if it resolves to a live node here) and queue
    /// it for the `collect` walk. Skips foreign/dead ids and already-marked ones.
    fn seed_mark(
        &self,
        id: NodeId,
        marked: &mut std::collections::HashSet<usize>,
        stack: &mut Vec<usize>,
    ) {
        if let Some(v) = self.try_index(id) {
            if self.nodes.contains_key(&v) && marked.insert(v) {
                stack.push(v);
            }
        }
    }

    /// Link `child` under `parent` without recording a mutation. Used while
    /// building a parsed subtree, which is covered by one `SubtreeReplaced`.
    fn attach_silent(&mut self, parent: NodeId, child: NodeId) {
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
    }

    /// Deep-copy a node from a parsed [`StaticDocument`] into this arena (silent),
    /// returning the new id.
    fn copy_fragment_node(&mut self, src: &StaticDocument, sid: StaticNodeId) -> NodeId {
        let new = match src.kind(sid) {
            NodeKind::Element => {
                let mut node = Node::new(NodeKind::Element);
                node.name = src.element_name(sid).cloned();
                for attr in src.attributes(sid) {
                    node.attrs.push((attr.name.clone(), attr.value.to_owned()));
                }
                self.push(node)
            }
            kind @ (NodeKind::Text | NodeKind::Comment) => {
                let mut node = Node::new(kind);
                node.text = src.text(sid).map(str::to_owned);
                self.push(node)
            }
            other => self.push(Node::new(other)),
        };
        let children: Vec<StaticNodeId> = src.dom_children(sid).collect();
        for child in children {
            let copied = self.copy_fragment_node(src, child);
            self.attach_silent(new, copied);
        }
        new
    }

    /// The `<body>` element of a `parse_document`-parsed fragment, if present.
    fn fragment_body(doc: &StaticDocument) -> Option<StaticNodeId> {
        let html = doc.document_element()?;
        doc.dom_children(html)
            .find(|&c| doc.element_name(c).is_some_and(|q| q.local == LocalName::from("body")))
    }
}

impl LayoutDom for ScriptedDom {
    type NodeId = NodeId;

    fn document(&self) -> NodeId {
        self.root
    }

    /// The dangle-contract liveness check (see [`LayoutDom::is_live`]). Live iff
    /// the id belongs to this document and still has an entry in the store
    /// (attached or orphaned-but-kept); a dropped or collected node has no entry.
    /// Never panics, unlike the read accessors.
    fn is_live(&self, id: NodeId) -> bool {
        self.try_index(id).is_some_and(|v| self.nodes.contains_key(&v))
    }

    fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    fn prev_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.sibling(id, -1)
    }

    fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.sibling(id, 1)
    }

    fn dom_children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id).children.iter().copied()
    }

    fn kind(&self, id: NodeId) -> NodeKind {
        self.node(id).kind
    }

    fn opaque_id(&self, id: NodeId) -> u64 {
        // Assert ownership (the fence), then return the full packed id so the
        // tag rides opaquely through Stylo's `OpaqueElement`. Off the fence
        // this is just `id.0`, identical to before.
        let _ = self.index(id);
        id.0 as u64
    }

    fn element_name(&self, id: NodeId) -> Option<&QualName> {
        self.node(id).name.as_ref()
    }

    fn attribute(&self, id: NodeId, ns: &Namespace, local: &LocalName) -> Option<&str> {
        self.node(id)
            .attrs
            .iter()
            .find(|(name, _)| &name.ns == ns && &name.local == local)
            .map(|(_, value)| value.as_str())
    }

    fn attributes(&self, id: NodeId) -> impl Iterator<Item = AttributeView<'_>> + '_ {
        self.node(id)
            .attrs
            .iter()
            .map(|(name, value)| AttributeView {
                name,
                value: value.as_str(),
            })
    }

    fn text(&self, id: NodeId) -> Option<&str> {
        self.node(id).text.as_deref()
    }
}

impl LayoutDomMut for ScriptedDom {
    fn create_element(&mut self, name: QualName) -> NodeId {
        let mut node = Node::new(NodeKind::Element);
        node.name = Some(name);
        self.push(node)
    }

    fn create_text(&mut self, data: &str) -> NodeId {
        let mut node = Node::new(NodeKind::Text);
        node.text = Some(data.to_owned());
        self.push(node)
    }

    fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.node_mut(child).parent = Some(parent);
        self.node_mut(parent).children.push(child);
        self.mutations
            .push(DomMutation::Inserted { node: child, parent });
    }

    fn insert_before(&mut self, parent: NodeId, child: NodeId, reference: Option<NodeId>) {
        self.detach(child);
        self.node_mut(child).parent = Some(parent);
        // Resolve the insertion index *after* detaching (so a move within the
        // same parent reflects the post-detach positions). A missing or
        // non-child reference falls back to append.
        let idx = reference.and_then(|r| {
            self.node(parent).children.iter().position(|&c| c == r)
        });
        let kids = &mut self.node_mut(parent).children;
        match idx {
            Some(i) => kids.insert(i, child),
            None => kids.push(child),
        }
        self.mutations
            .push(DomMutation::Inserted { node: child, parent });
    }

    fn remove(&mut self, node: NodeId) {
        let former_parent = self.node(node).parent;
        self.detach(node);
        if let Some(former_parent) = former_parent {
            self.mutations.push(DomMutation::Removed {
                node,
                former_parent,
            });
        }
        self.drop_subtree(node);
    }

    fn set_attribute(&mut self, node: NodeId, name: QualName, value: &str) {
        let attrs = &mut self.node_mut(node).attrs;
        // Capture the prior value before overwriting — serval-layout needs
        // it to build the Stylo snapshot at restyle time (the old value is
        // gone from the live DOM once we mutate). `None` = newly added.
        let old_value;
        if let Some(existing) = attrs
            .iter_mut()
            .find(|(n, _)| n.ns == name.ns && n.local == name.local)
        {
            old_value = Some(std::mem::replace(&mut existing.1, value.to_owned()));
        } else {
            old_value = None;
            attrs.push((name.clone(), value.to_owned()));
        }
        self.mutations.push(DomMutation::AttributeChanged {
            node,
            name,
            old_value,
        });
    }

    fn remove_attribute(&mut self, node: NodeId, name: QualName) {
        // Drop the matching attribute and capture its prior value; the
        // borrow ends before we record the mutation. No-op (and no record)
        // when the attribute is absent.
        let removed = {
            let attrs = &mut self.node_mut(node).attrs;
            attrs
                .iter()
                .position(|(n, _)| n.ns == name.ns && n.local == name.local)
                .map(|pos| attrs.remove(pos).1)
        };
        if let Some(old) = removed {
            self.mutations.push(DomMutation::AttributeChanged {
                node,
                name,
                old_value: Some(old),
            });
        }
    }

    fn set_text(&mut self, node: NodeId, data: &str) {
        self.node_mut(node).text = Some(data.to_owned());
        self.mutations
            .push(DomMutation::CharacterDataChanged { node });
    }

    fn set_inner_html(&mut self, node: NodeId, html: &str) {
        // Drop the current children silently — the single SubtreeReplaced covers it.
        let existing = std::mem::take(&mut self.node_mut(node).children);
        for child in existing {
            self.node_mut(child).parent = None;
            self.drop_subtree(child);
        }
        // Parse via the static parser (a LayoutDom) and copy the <body> children in.
        // (Simplification: uses `parse_document` + the body subtree rather than true
        // context-aware fragment parsing; fine for the common element-fragment case.)
        let fragment = StaticDocument::parse(html);
        if let Some(body) = Self::fragment_body(&fragment) {
            let body_children: Vec<StaticNodeId> = fragment.dom_children(body).collect();
            for child in body_children {
                let copied = self.copy_fragment_node(&fragment, child);
                self.attach_silent(node, copied);
            }
        }
        self.mutations.push(DomMutation::SubtreeReplaced { node });
    }

    fn drain_mutations(&mut self, out: &mut Vec<DomMutation<NodeId>>) {
        out.append(&mut self.mutations);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qual(local: &str) -> QualName {
        QualName::new(None, Namespace::from(""), LocalName::from(local))
    }

    #[test]
    fn mutate_read_and_record() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();

        let div = dom.create_element(qual("div"));
        dom.append_child(root, div);
        let text = dom.create_text("hello");
        dom.append_child(div, text);
        dom.set_attribute(div, qual("id"), "main");

        // Read surface reflects the mutations.
        assert_eq!(dom.kind(div), NodeKind::Element);
        assert_eq!(dom.element_name(div).unwrap().local, LocalName::from("div"));
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![div]);
        assert_eq!(dom.dom_children(div).collect::<Vec<_>>(), vec![text]);
        assert_eq!(dom.parent(text), Some(div));
        assert_eq!(dom.text(text), Some("hello"));
        assert_eq!(
            dom.attribute(div, &Namespace::from(""), &LocalName::from("id")),
            Some("main")
        );

        // Mutation stream: 2 inserts + 1 attribute change, then drained empty.
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert_eq!(muts.len(), 3);
        let mut again = Vec::new();
        dom.drain_mutations(&mut again);
        assert!(again.is_empty());
    }

    #[test]
    fn set_inner_html_builds_subtree() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(qual("div"));
        dom.append_child(root, div);
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the append

        dom.set_inner_html(div, "<p>hi</p><span>x</span>");

        let kids: Vec<_> = dom.dom_children(div).collect();
        assert_eq!(kids.len(), 2);
        assert_eq!(dom.element_name(kids[0]).unwrap().local, LocalName::from("p"));
        assert_eq!(
            dom.element_name(kids[1]).unwrap().local,
            LocalName::from("span")
        );
        // <p>hi</p> — the <p> has a single text child "hi".
        let p_kids: Vec<_> = dom.dom_children(kids[0]).collect();
        assert_eq!(p_kids.len(), 1);
        assert_eq!(dom.text(p_kids[0]), Some("hi"));

        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert!(matches!(muts.as_slice(), [DomMutation::SubtreeReplaced { .. }]));
    }

    #[test]
    fn siblings_and_remove() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let a = dom.create_element(qual("a"));
        let b = dom.create_element(qual("b"));
        dom.append_child(root, a);
        dom.append_child(root, b);

        assert_eq!(dom.next_sibling(a), Some(b));
        assert_eq!(dom.prev_sibling(b), Some(a));
        assert_eq!(dom.prev_sibling(a), None);

        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the two inserts

        dom.remove(a);
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![b]);
        assert_eq!(dom.next_sibling(b), None);

        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert!(matches!(
            muts.as_slice(),
            [DomMutation::Removed { .. }]
        ));
    }

    #[test]
    fn insert_before_orders_and_appends() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let a = dom.create_element(qual("a"));
        let c = dom.create_element(qual("c"));
        dom.append_child(root, a);
        dom.append_child(root, c);
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the two appends

        // Insert b before c → [a, b, c].
        let b = dom.create_element(qual("b"));
        dom.insert_before(root, b, Some(c));
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![a, b, c]);
        assert_eq!(dom.parent(b), Some(root));

        // reference = None appends → [a, b, c, d].
        let d = dom.create_element(qual("d"));
        dom.insert_before(root, d, None);
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![a, b, c, d]);

        // A reference that isn't a child of root falls back to append → [a, b, c, d, e].
        let orphan = dom.create_element(qual("orphan"));
        let e = dom.create_element(qual("e"));
        dom.insert_before(root, e, Some(orphan));
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![a, b, c, d, e]);

        // Each insert recorded exactly one Inserted under root.
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert_eq!(muts.len(), 3);
        assert!(muts.iter().all(
            |m| matches!(m, DomMutation::Inserted { parent, .. } if *parent == root)
        ));
    }

    #[test]
    fn remove_attribute_records_and_noops() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let div = dom.create_element(qual("div"));
        dom.append_child(root, div);
        dom.set_attribute(div, qual("id"), "main");
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained); // clear the append + set

        // Removing a present attribute drops it and records the old value.
        dom.remove_attribute(div, qual("id"));
        assert_eq!(
            dom.attribute(div, &Namespace::from(""), &LocalName::from("id")),
            None
        );
        let mut muts = Vec::new();
        dom.drain_mutations(&mut muts);
        assert!(matches!(
            muts.as_slice(),
            [DomMutation::AttributeChanged { old_value: Some(v), .. }] if v.as_str() == "main"
        ));

        // Removing an absent attribute is a no-op and records nothing.
        dom.remove_attribute(div, qual("id"));
        let mut again = Vec::new();
        dom.drain_mutations(&mut again);
        assert!(again.is_empty());
    }

    // --- G0: the document fence ---------------------------------------------

    #[test]
    fn secondary_root_is_same_document() {
        // `create_document` mints a second root in the *same* arena (same tag),
        // so cross-using ids between the two roots must not trip the fence.
        let mut dom = ScriptedDom::new();
        let primary = dom.document();
        let secondary = dom.create_document();
        let div = dom.create_element(qual("div"));
        dom.append_child(secondary, div);
        assert_eq!(dom.parent(div), Some(secondary));
        assert_ne!(primary, secondary);
        // Both roots resolve without panicking.
        assert_eq!(dom.kind(primary), NodeKind::Document);
        assert_eq!(dom.kind(secondary), NodeKind::Document);
    }

    /// On a fenced build (64-bit debug) a `NodeId` minted by one document used
    /// against another panics. On release/wasm the fence compiles out, so this
    /// test only exists where the assert is live.
    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    #[test]
    #[should_panic(expected = "different document")]
    fn cross_document_node_id_panics() {
        let mut a = ScriptedDom::new();
        let b = ScriptedDom::new();
        let id_in_a = a.create_element(qual("div"));
        // `id_in_a` carries a's tag; resolving it against b trips the fence.
        let _ = b.kind(id_in_a);
    }

    #[cfg(all(debug_assertions, target_pointer_width = "64"))]
    #[test]
    fn distinct_documents_get_distinct_tags() {
        let a = ScriptedDom::new();
        let b = ScriptedDom::new();
        // Roots share the arena index (0) but differ in the tagged high bits.
        assert_ne!(a.document().raw(), b.document().raw());
    }

    // --- G2: the dangle contract (is_live) ----------------------------------

    /// The contract under create/remove/re-query across frames (the slab
    /// implementation G3 must preserve, allocator aside). Attached → live;
    /// dropped → dead; orphaned-but-kept → still live; cross-document → not live.
    #[test]
    fn dangle_contract_churn_across_frames() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();

        // Frame 1: build a small tree, drain the mutations (the frame boundary).
        let a = dom.create_element(qual("a"));
        let b = dom.create_element(qual("b"));
        dom.append_child(root, a);
        dom.append_child(root, b);
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained);

        // Attached ids are live.
        assert!(dom.is_live(root));
        assert!(dom.is_live(a));
        assert!(dom.is_live(b));

        // Frame 2: `remove` drops `a`'s subtree. Its id is now dead, and a
        // re-query of the tree no longer contains it.
        dom.remove(a);
        dom.drain_mutations(&mut drained);
        assert!(!dom.is_live(a));
        assert!(dom.is_live(b));
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![b]);

        // Frame 3: `remove_child` orphans `b` but keeps it — still live and
        // re-insertable (the orphan semantics the gc-arena refit must honor).
        dom.remove_child(b);
        dom.drain_mutations(&mut drained);
        assert!(dom.is_live(b), "an orphaned node stays live until dropped");
        assert!(dom.dom_children(root).collect::<Vec<_>>().is_empty());
        dom.append_child(root, b); // re-insert the orphan
        assert_eq!(dom.dom_children(root).collect::<Vec<_>>(), vec![b]);
        assert!(dom.is_live(b));
    }

    #[test]
    fn is_live_is_false_for_dropped_and_foreign_ids() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let n = dom.create_element(qual("n"));
        dom.append_child(root, n);
        assert!(dom.is_live(n));
        dom.remove(n);
        assert!(!dom.is_live(n));

        // An id from another document is not live here (no panic — `is_live` is
        // the non-asserting check, unlike the read accessors).
        let other = ScriptedDom::new();
        let foreign = {
            let mut o = other;
            o.create_element(qual("x"))
        };
        assert!(!dom.is_live(foreign));
    }

    // --- G3: mark-sweep collection ------------------------------------------

    const NO_PINS: [NodeId; 0] = [];

    /// `collect` keeps the attached tree and reaps an unpinned orphan, but spares
    /// a pinned orphan *and its whole connected component* (undirected mark).
    #[test]
    fn collect_reaps_unpinned_orphans_keeps_pinned_components() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let attached = dom.create_element(qual("attached"));
        dom.append_child(root, attached);

        // An orphan subtree: parent -> mid -> leaf, all detached from the document.
        let parent = dom.create_element(qual("p"));
        let mid = dom.create_element(qual("mid"));
        let leaf = dom.create_element(qual("leaf"));
        dom.append_child(parent, mid);
        dom.append_child(mid, leaf);

        // With no pins, the orphan subtree is unreachable → collected; the
        // attached tree survives.
        let live_before = dom.live_node_count();
        let pruned = dom.collect(NO_PINS);
        assert_eq!(pruned, 3, "the 3-node orphan subtree is reaped");
        assert_eq!(dom.live_node_count(), live_before - 3);
        assert!(dom.is_live(root) && dom.is_live(attached));
        assert!(!dom.is_live(parent) && !dom.is_live(mid) && !dom.is_live(leaf));

        // Rebuild the orphan and pin only the *deep* leaf: the undirected mark
        // must keep the leaf's ancestors too (JS can walk parentNode up).
        let parent = dom.create_element(qual("p"));
        let mid = dom.create_element(qual("mid"));
        let leaf = dom.create_element(qual("leaf"));
        dom.append_child(parent, mid);
        dom.append_child(mid, leaf);
        let pruned = dom.collect([leaf]); // pin the leaf
        assert_eq!(pruned, 0, "a pin on the leaf spares the whole component");
        assert!(dom.is_live(parent) && dom.is_live(mid) && dom.is_live(leaf));

        // Drop the pin → the component is now collectable.
        assert_eq!(dom.collect(NO_PINS), 3);
        assert!(!dom.is_live(parent) && !dom.is_live(mid) && !dom.is_live(leaf));
    }

    /// The bounded-memory property: sustained create/remove-child/collect cycles
    /// plateau the store, where the never-reuse slab would grow without bound.
    #[test]
    fn collect_bounds_memory_under_churn() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let baseline = dom.live_node_count();

        let mut peak = baseline;
        for _ in 0..500 {
            // Attach a small subtree, then orphan it and collect (no pins) — the
            // SPA churn shape: create nodes, detach them, JS drops the reflectors.
            let host = dom.create_element(qual("host"));
            dom.append_child(root, host);
            for _ in 0..8 {
                let kid = dom.create_element(qual("kid"));
                dom.append_child(host, kid);
            }
            peak = peak.max(dom.live_node_count());
            dom.remove_child(host); // orphan the whole subtree
            dom.collect(NO_PINS); // reap it
        }

        // Monotonic ids kept climbing, but the store is back to baseline — bounded.
        assert!(dom.next_id > 4000, "ids are monotonic (no reuse)");
        assert_eq!(dom.live_node_count(), baseline, "store plateaus, not grows");
        assert!(peak < baseline + 32, "peak stays tiny — only one subtree at a time");
    }

    /// A quiet document (no further mutations) still reaps orphans on an idle
    /// `collect` — the backgrounded-SPA case.
    #[test]
    fn idle_collect_reaps_orphans_without_mutations() {
        let mut dom = ScriptedDom::new();
        let root = dom.document();
        let o = dom.create_element(qual("o"));
        dom.append_child(root, o);
        dom.remove_child(o); // orphan; no pin
        let mut drained = Vec::new();
        dom.drain_mutations(&mut drained);

        // No mutations happen now; an idle collect still reaps the orphan.
        assert!(dom.is_live(o));
        assert_eq!(dom.collect(NO_PINS), 1);
        assert!(!dom.is_live(o));
    }

    /// A `create_document` secondary is pin-kept (G3 carve-out #3): while pinned
    /// its whole subtree survives `collect`; once the pin drops, it collects like
    /// any other orphan (a dropped `createHTMLDocument` no longer leaks).
    #[test]
    fn secondary_document_is_pin_kept() {
        let mut dom = ScriptedDom::new();
        let secondary = dom.create_document();
        let body = dom.create_element(qual("body"));
        dom.append_child(secondary, body);

        // Pinned (script holds it) → the whole secondary component survives.
        assert_eq!(dom.collect([secondary]), 0);
        assert!(dom.is_live(secondary) && dom.is_live(body));

        // Dropped (no pin) → it collects.
        assert_eq!(dom.collect(NO_PINS), 2);
        assert!(!dom.is_live(secondary) && !dom.is_live(body));
    }

    #[test]
    fn pins_pin_unpin_and_retire() {
        let id = NodeId::from_raw;
        let mut pins = Pins::new();
        assert!(pins.is_empty());

        pins.pin(id(0x10));
        pins.pin(id(0x20));
        pins.pin(id(0x10)); // idempotent
        assert_eq!(pins.len(), 2);
        assert!(pins.is_pinned(id(0x10)));

        assert!(pins.unpin(id(0x20)));
        assert!(!pins.unpin(id(0xAA))); // never pinned → no-op

        pins.pin(id(0x30));
        assert_eq!(pins.retire_dead([id(0x10), id(0x30), id(0xBB)]), 2);
        assert!(pins.is_empty());
    }

    #[test]
    fn pins_clear() {
        let mut pins = Pins::new();
        pins.pin(NodeId::from_raw(1));
        pins.pin(NodeId::from_raw(2));
        assert_eq!(pins.len(), 2);
        pins.clear();
        assert!(pins.is_empty());
    }
}
