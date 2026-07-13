/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Profile-neutral DOM trait.
//!
//! `LayoutDom` is the ID-first surface that `genet-layout` (and other
//! read-only DOM walkers — reader-mode, serialization, querySelector helpers)
//! consume. It does not commit to a backing store: `genet-static-dom`'s
//! `StaticDocument` and a future scripted-DOM provider both implement it.
//!
//! Design rationale and prior art: see
//! `docs/2026-05-16_layout_dom_api_design.md`.

#![deny(unsafe_code)]

use std::fmt::Debug;
use std::hash::Hash;
use std::ops::ControlFlow;

#[cfg(feature = "capture")]
use markup5ever::Prefix;
pub use markup5ever::interface::QuirksMode;
pub use markup5ever::{LocalName, Namespace, QualName};
#[cfg(feature = "capture")]
use serde::{Deserialize, Serialize};

/// Profile-neutral DOM. Implementors expose opaque `NodeId`s and a small set
/// of lookup primitives; traversal happens through the default `walk` impl
/// over a [`NodeVisitor`], or through caller-driven cursors built on the
/// lookup primitives.
pub trait LayoutDom {
    /// Opaque per-backend node identity. Must be `Copy` for cheap pass-through.
    type NodeId: Copy + Eq + Hash + Debug + 'static;

    // ---- identity / structure -------------------------------------------

    /// The document root.
    ///
    /// Two shapes are supported. A `Document` wrapper node whose element
    /// children are the roots: parsed HTML has exactly one (`<html>`), but a
    /// host-built synthetic DOM (an app chrome layer, a widget pool) may hang
    /// SEVERAL elements here with no wrapper — layout styles and paints every
    /// one of them (genet-layout wraps them in a synthetic block root; see
    /// its `multi_root_document_paints_every_root_element` test). Or an
    /// element node (a re-rooted subtree view): that element is itself the
    /// root. Hosts do not need to invent an `<html>`/container element just
    /// to satisfy layout. Note the CSS root-background propagation
    /// (`<html>`/`<body>` background painting the whole canvas) applies only
    /// to a sole-root document.
    fn document(&self) -> Self::NodeId;

    /// Whether `id` still resolves to a live node — the **dangle contract**.
    ///
    /// Contract: an id for an **attached** node is always live. An id for a node
    /// that was dropped (by [`LayoutDomMut::remove`], or — once a backend
    /// collects detached nodes — orphaned, unpinned, and collected) is **dead**.
    /// `is_live` is the only read that is safe to call on a possibly-dead id; it
    /// never panics. The other accessors assume a live id and may panic on a
    /// dead one (the same "not found" outcome a removed slot gives). A caller
    /// that holds an id across frames (a handler registry, a layout side-table,
    /// a query result, an undrained mutation log) must treat it as possibly dead
    /// and guard reads with `is_live`.
    ///
    /// Default: `true`. Immutable backends (a parsed [`LayoutDom`] with no
    /// removal) never produce dead ids; a mutable backend overrides this.
    fn is_live(&self, _id: Self::NodeId) -> bool {
        true
    }

    /// The document's quirks mode, as selected by the parser (presence/absence
    /// of a `<!DOCTYPE>`). Drives quirk-gated cascade behaviour (Stylo's
    /// `QuirksMode`-conditional UA rules, e.g. the table font-size quirk).
    /// Defaults to standards mode; a backend that parses a real document
    /// overrides it.
    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }

    /// Parent node, if any.
    fn parent(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Previous sibling in DOM order. Hot on selector-matching paths
    /// (`prev_sibling_element` in `selectors::Element`); deriving it from
    /// `dom_children(parent)` would be O(siblings) per call.
    fn prev_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// Next sibling in DOM order. See [`Self::prev_sibling`].
    fn next_sibling(&self, id: Self::NodeId) -> Option<Self::NodeId>;

    /// DOM-tree children (parse-order, ignores shadow trees).
    fn dom_children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_;

    /// Flat-tree children (slot-assigned for shadow hosts, otherwise DOM
    /// order). Backends without shadow DOM should leave this defaulted.
    fn flat_children(&self, id: Self::NodeId) -> impl Iterator<Item = Self::NodeId> + '_ {
        self.dom_children(id)
    }

    // ---- kind and hot primitives ----------------------------------------

    /// What kind of node `id` is. Plain enum; details via the typed
    /// accessors below.
    fn kind(&self, id: Self::NodeId) -> NodeKind;

    /// Stable per-node identity as a `u64`. Used by foreign trait adapters
    /// (Stylo's `OpaqueNode`, `selectors::OpaqueElement`) that need a
    /// pointer-shaped value for identity comparisons in the cascade.
    ///
    /// Must satisfy: distinct nodes within the same backing store return
    /// distinct `opaque_id` values, and the same node returns the same value
    /// across calls. Implementations may use the inner storage index (dense
    /// DOMs) or a hash (sparse DOMs).
    ///
    /// The default implementation hashes `id` with `DefaultHasher` — works
    /// for any `NodeId: Hash` but isn't guaranteed to be collision-free
    /// across all node sets. Backends should override when they can return
    /// the natural underlying index cheaply.
    fn opaque_id(&self, id: Self::NodeId) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    }

    /// Element name when `id` is an element, else `None`. Hot on
    /// selector/style match paths.
    fn element_name(&self, id: Self::NodeId) -> Option<&QualName>;

    /// Attribute value lookup by namespace + local name. Hot on selector/style
    /// match paths. Backends with column-stored attrs can implement this as
    /// a keyed lookup without materializing a full slice.
    fn attribute(&self, id: Self::NodeId, ns: &Namespace, local: &LocalName) -> Option<&str>;

    /// Iterate this element's attributes (cold path: serialization,
    /// introspection). Yields `AttributeView`s borrowed from the backing
    /// store.
    fn attributes(&self, id: Self::NodeId) -> impl Iterator<Item = AttributeView<'_>> + '_;

    /// Text content for text or comment nodes, else `None`.
    fn text(&self, id: Self::NodeId) -> Option<&str>;

    // ---- traversal -------------------------------------------------------

    /// Walk the whole document from `document()`, descending via
    /// `dom_children`. Backends override when they want backend-driven
    /// traversal (parallel layout pass, prefetching, flat-tree descent).
    fn walk<V>(&self, visitor: &mut V) -> ControlFlow<V::Stop>
    where
        V: NodeVisitor<Self> + ?Sized,
    {
        walk_subtree(self, self.document(), visitor)
    }

    // ---- class / tag queries --------------------------------------------
    //
    // Pre-order subtree searches a host and genet-internal callers both reach for
    // (find the element painting a class, collect a class's placeholders, hit-test a
    // tag). Provided as defaults over `dom_children` / `attributes` / `element_name`
    // so neither side re-rolls the walk.

    /// Whether element `id` carries CSS class `class` (whitespace-split `class` attr).
    fn has_class(&self, id: Self::NodeId, class: &str) -> bool {
        self.attributes(id).any(|a| {
            a.name.local.as_ref() == "class" && a.value.split_whitespace().any(|c| c == class)
        })
    }

    /// The first element carrying CSS class `class` in pre-order under `id` (inclusive).
    fn first_with_class(&self, id: Self::NodeId, class: &str) -> Option<Self::NodeId> {
        if self.has_class(id, class) {
            return Some(id);
        }
        self.dom_children(id)
            .find_map(|c| self.first_with_class(c, class))
    }

    /// Every element carrying CSS class `class` in pre-order under `id` (inclusive).
    fn all_with_class(&self, id: Self::NodeId, class: &str) -> Vec<Self::NodeId> {
        let mut out = Vec::new();
        if self.has_class(id, class) {
            out.push(id);
        }
        for child in self.dom_children(id) {
            out.extend(self.all_with_class(child, class));
        }
        out
    }

    /// The first element with local tag name `local` in pre-order under `id` (inclusive).
    fn first_tag(&self, id: Self::NodeId, local: &str) -> Option<Self::NodeId> {
        if self
            .element_name(id)
            .is_some_and(|q| q.local.as_ref() == local)
        {
            return Some(id);
        }
        self.dom_children(id).find_map(|c| self.first_tag(c, local))
    }
}

/// Mutation extension for scripted DOMs (plan Part 3 / the layout_dom_api design's
/// open question #1). Read-only consumers (reader-mode, serialization, static
/// layout) implement only [`LayoutDom`]; `genet-scripted-dom` implements both.
///
/// Mutators record *structural* change as [`DomMutation`] records — they carry no
/// notion of dirty bits, style, or layout. genet-layout's scheduler drains the
/// stream ([`Self::drain_mutations`]) and translates it into StylePlane/LayoutPlane
/// invalidation; the DOM provider itself stays render-state-free.
pub trait LayoutDomMut: LayoutDom {
    /// Create a detached element node (no parent until appended).
    fn create_element(&mut self, name: QualName) -> Self::NodeId;

    /// Create a detached text node.
    fn create_text(&mut self, data: &str) -> Self::NodeId;

    /// Append `child` as the last child of `parent`, detaching it from any
    /// previous parent first.
    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId);

    /// Insert `child` immediately before `reference` among `parent`'s children,
    /// detaching `child` from any previous parent first. Appends if `reference`
    /// is `None`, or if `reference` is not a child of `parent` (defensive: the
    /// DOM `insertBefore` throws in that case, but the layout-side contract
    /// stays total). The ordered-insertion primitive a reactive differ needs;
    /// `append_child` is the `reference == None` tail case.
    fn insert_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    );

    /// Atomically move an in-tree `child` under `parent` before `reference`
    /// (append when `None`), preserving subtree state — the `Node.moveBefore()`
    /// contract (WHATWG DOM; docs/2026-07-05_movebefore_dom_standard_plan.md).
    /// Records one [`DomMutation::Moved`] instead of the `Removed` + `Inserted`
    /// pair [`insert_before`](Self::insert_before) produces for an in-tree node,
    /// so consumers may keep the subtree's retained state. A move resolving to
    /// the current position records nothing; a disconnected `child` degrades to
    /// a plain insert (the DOM-level `moveBefore` throws there; this layout-side
    /// contract stays total, like `insert_before`'s bad-reference fallback).
    fn move_before(
        &mut self,
        parent: Self::NodeId,
        child: Self::NodeId,
        reference: Option<Self::NodeId>,
    );

    /// Detach `node` from its parent and drop its subtree.
    fn remove(&mut self, node: Self::NodeId);

    /// Set (or replace) an attribute on an element.
    fn set_attribute(&mut self, node: Self::NodeId, name: QualName, value: &str);

    /// Remove the attribute named `name` from `node` (no-op if absent). Records
    /// an [`DomMutation::AttributeChanged`] carrying the removed value as
    /// `old_value` (the live DOM then reads as absent), so genet-layout builds
    /// the Stylo snapshot the same way it does for a value change.
    fn remove_attribute(&mut self, node: Self::NodeId, name: QualName);

    /// Replace a text/comment node's character data.
    fn set_text(&mut self, node: Self::NodeId, data: &str);

    /// Replace `node`'s children with the subtree parsed from an HTML fragment
    /// (the `innerHTML` setter). Records a single [`DomMutation::SubtreeReplaced`].
    fn set_inner_html(&mut self, node: Self::NodeId, html: &str);

    /// Drain the structural mutations recorded since the last call into `out`.
    /// The provider records WHAT changed; genet-layout decides what to invalidate.
    fn drain_mutations(&mut self, out: &mut Vec<DomMutation<Self::NodeId>>);
}

/// A recorded structural DOM mutation — render-state-free (no dirty bits, no style).
/// `Id` is the implementor's [`LayoutDom::NodeId`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DomMutation<Id> {
    /// `node` was inserted under `parent`.
    Inserted { node: Id, parent: Id },
    /// `node` was removed from `former_parent`.
    Removed { node: Id, former_parent: Id },
    /// The attribute named `name` was set or changed on `node`.
    ///
    /// `old_value` is the attribute's value *before* this change (`None`
    /// if the attribute was newly added). It is plain pre-mutation DOM
    /// data — not render state — and lets genet-layout reconstruct a
    /// Stylo `ElementSnapshot` at restyle time (the old value is gone from
    /// the live DOM by then). See
    /// `docs/2026-05-25_fine_grained_restyle_plan.md`.
    AttributeChanged {
        node: Id,
        name: QualName,
        old_value: Option<String>,
    },
    /// A text/comment node's character data changed.
    CharacterDataChanged { node: Id },
    /// `node`'s entire child subtree was replaced (e.g. via `innerHTML`).
    SubtreeReplaced { node: Id },
    /// `node` moved atomically from `from_parent` to `to_parent` (possibly the
    /// same parent, reordered) with state preserved — the `Node.moveBefore()`
    /// contract (WHATWG DOM). Unlike a `Removed` + `Inserted` pair this promises
    /// the subtree never left the tree: consumers may keep per-node retained
    /// state (boxes, shaped text, focus, scroll) and treat the move as a
    /// splice/graft candidate rather than a teardown. A conservative consumer
    /// handles it exactly as removed-from + inserted-under.
    /// (docs/2026-07-05_movebefore_dom_standard_plan.md, S1.)
    Moved {
        node: Id,
        from_parent: Id,
        to_parent: Id,
    },
}

/// Serializable mirror of [`QualName`] for capture/replay logs.
#[cfg(feature = "capture")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapturedQualName {
    pub prefix: Option<String>,
    pub ns: String,
    pub local: String,
}

#[cfg(feature = "capture")]
impl From<&QualName> for CapturedQualName {
    fn from(value: &QualName) -> Self {
        Self {
            prefix: value.prefix.as_ref().map(ToString::to_string),
            ns: value.ns.to_string(),
            local: value.local.to_string(),
        }
    }
}

#[cfg(feature = "capture")]
impl CapturedQualName {
    pub fn into_qual_name(self) -> QualName {
        QualName::new(
            self.prefix.map(Prefix::from),
            Namespace::from(self.ns),
            LocalName::from(self.local),
        )
    }
}

/// Serializable mirror of [`DomMutation`] for capture/replay logs.
#[cfg(feature = "capture")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CapturedMutation {
    Inserted {
        node: u64,
        parent: u64,
    },
    Removed {
        node: u64,
        former_parent: u64,
    },
    AttributeChanged {
        node: u64,
        name: CapturedQualName,
        old_value: Option<String>,
    },
    CharacterDataChanged {
        node: u64,
    },
    SubtreeReplaced {
        node: u64,
    },
    Moved {
        node: u64,
        from_parent: u64,
        to_parent: u64,
    },
}

#[cfg(feature = "capture")]
impl CapturedMutation {
    pub fn capture<Id>(mutation: &DomMutation<Id>, to_raw: impl Fn(&Id) -> u64) -> Self {
        match mutation {
            DomMutation::Inserted { node, parent } => Self::Inserted {
                node: to_raw(node),
                parent: to_raw(parent),
            },
            DomMutation::Removed {
                node,
                former_parent,
            } => Self::Removed {
                node: to_raw(node),
                former_parent: to_raw(former_parent),
            },
            DomMutation::AttributeChanged {
                node,
                name,
                old_value,
            } => Self::AttributeChanged {
                node: to_raw(node),
                name: CapturedQualName::from(name),
                old_value: old_value.clone(),
            },
            DomMutation::CharacterDataChanged { node } => {
                Self::CharacterDataChanged { node: to_raw(node) }
            },
            DomMutation::SubtreeReplaced { node } => Self::SubtreeReplaced { node: to_raw(node) },
            DomMutation::Moved {
                node,
                from_parent,
                to_parent,
            } => Self::Moved {
                node: to_raw(node),
                from_parent: to_raw(from_parent),
                to_parent: to_raw(to_parent),
            },
        }
    }

    pub fn replay<Id>(self, from_raw: impl Fn(u64) -> Id) -> DomMutation<Id> {
        match self {
            Self::Inserted { node, parent } => DomMutation::Inserted {
                node: from_raw(node),
                parent: from_raw(parent),
            },
            Self::Removed {
                node,
                former_parent,
            } => DomMutation::Removed {
                node: from_raw(node),
                former_parent: from_raw(former_parent),
            },
            Self::AttributeChanged {
                node,
                name,
                old_value,
            } => DomMutation::AttributeChanged {
                node: from_raw(node),
                name: name.into_qual_name(),
                old_value,
            },
            Self::CharacterDataChanged { node } => DomMutation::CharacterDataChanged {
                node: from_raw(node),
            },
            Self::SubtreeReplaced { node } => DomMutation::SubtreeReplaced {
                node: from_raw(node),
            },
            Self::Moved {
                node,
                from_parent,
                to_parent,
            } => DomMutation::Moved {
                node: from_raw(node),
                from_parent: from_raw(from_parent),
                to_parent: from_raw(to_parent),
            },
        }
    }
}

/// Plain node kind. Use the typed accessors on [`LayoutDom`]
/// (`element_name`, `attribute`, `text`, etc.) to read kind-specific data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeKind {
    Document,
    Doctype,
    Element,
    Text,
    Comment,
    ProcessingInstruction,
    /// A `DocumentFragment` (nodeType 11): a parentless container, used as the
    /// scripted-DOM holder for `createDocumentFragment` and fragment parsing.
    DocumentFragment,
}

/// Borrowed view of one attribute on an element.
#[derive(Clone, Copy, Debug)]
pub struct AttributeView<'a> {
    pub name: &'a QualName,
    pub value: &'a str,
}

/// Visitor over a [`LayoutDom`]. Methods return [`ControlFlow`] so the visitor
/// can bail early with a typed `Stop` value. Use `type Stop = ()` for plain
/// "stop or not"; use `core::convert::Infallible` to assert the walk never
/// terminates early; use a typed error type to carry per-node-failure data
/// out of the walk.
pub trait NodeVisitor<D: LayoutDom + ?Sized> {
    /// Early-termination payload carried out of the walk.
    type Stop;

    /// Called when descending into a node. Default: descend.
    fn enter(&mut self, _dom: &D, _id: D::NodeId) -> ControlFlow<Self::Stop, Descent> {
        ControlFlow::Continue(Descent::Descend)
    }

    /// Called after a node's subtree has been visited. Default: continue.
    fn exit(&mut self, _dom: &D, _id: D::NodeId) -> ControlFlow<Self::Stop> {
        ControlFlow::Continue(())
    }
}

/// Per-node descent decision returned from [`NodeVisitor::enter`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Descent {
    /// Descend into this node's children.
    Descend,
    /// Skip this node's subtree but continue walking siblings/parent.
    Skip,
}

/// Walk `root`'s subtree with `visitor`, descending via
/// [`LayoutDom::dom_children`]. Returns `ControlFlow::Break(stop)` if any
/// visitor method bailed; otherwise `ControlFlow::Continue(())`.
pub fn walk_subtree<D, V>(dom: &D, root: D::NodeId, visitor: &mut V) -> ControlFlow<V::Stop>
where
    D: LayoutDom + ?Sized,
    V: NodeVisitor<D> + ?Sized,
{
    match visitor.enter(dom, root)? {
        Descent::Skip => ControlFlow::Continue(()),
        Descent::Descend => {
            for child in dom.dom_children(root) {
                walk_subtree(dom, child, visitor)?;
            }
            visitor.exit(dom, root)
        },
    }
}
