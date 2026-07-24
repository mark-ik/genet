//! Accessibility host for a genet-backed Cambium app.
//!
//! Every Cambium app emits a semantic, ARIA-attributed DOM laid out by
//! genet-layout, and paints its custom visuals with Sprigging leaves. This
//! module turns that into a live accessibility tree for the OS screen reader,
//! so no app has to hand-roll the wiring:
//!
//! - [`SpriggingA11y`] lets the layout walk ask each `<custom-leaf>` for its own
//!   semantics ([`sprigging::Leaf::accessibility`]) — the mirror of the paint
//!   registry, for meaning rather than pixels.
//! - [`A11yHost`] owns the platform adapter and the per-frame lifecycle: project
//!   the retained layout into an AccessKit tree (leaf semantics included),
//!   install it the first frame and update it after, and drain a screen reader's
//!   actions, handing back the DOM nodes to activate so the app routes them
//!   through the same click path a mouse uses.
//!
//! The app keeps only what is app-specific: create the window hidden (the
//! adapter must attach before it is shown), drive the first frame synchronously
//! (a hidden window may not receive a deferred redraw), turn the wake callback
//! into a redraw, and dispatch the returned nodes.

use std::collections::HashMap;

use accesskit::{Action, NodeId as A11yNodeId, Tree, TreeId, TreeUpdate};
use genet_layout::{IncrementalLayout, LeafA11ySource, project};
use genet_scripted_dom::{NodeId, ScriptedDom};
use genet_winit_host::{AccessKitBridge, BridgeStatus};
use layout_dom_api::LayoutDom as _;
use sprigging::LeafRegistry;
use winit::window::Window;

/// Bridges genet-layout's a11y walk to a Sprigging leaf registry: when the walk
/// reaches a `<custom-leaf>`, the registered leaf fills its own AccessKit node
/// (a knob announces as a slider, a fretboard as a graphic). Mirrors the paint
/// registry's role, for semantics.
pub struct SpriggingA11y<'a>(pub &'a mut LeafRegistry<u64>);

impl LeafA11ySource for SpriggingA11y<'_> {
    fn describe_leaf(&mut self, key: u64, node: &mut accesskit::Node) {
        if let Some(leaf) = self.0.get_mut(&key) {
            leaf.accessibility(node);
        }
    }
}

/// Owns the OS AccessKit adapter and the per-frame tree lifecycle for a
/// genet-backed Cambium app. Create it in `resumed` (with a wake callback that
/// nudges the event loop), then call [`A11yHost::sync`] after every frame.
pub struct A11yHost {
    bridge: AccessKitBridge,
    installed: bool,
    /// AccessKit node id -> its DOM node, rebuilt each frame, so a screen
    /// reader's action on a node routes back to the element it came from.
    action_map: HashMap<A11yNodeId, NodeId>,
}

impl A11yHost {
    /// Create the adapter. `wake` is called by the adapter when a screen reader
    /// acts while the app is idle; wire it to request a redraw so the queued
    /// action gets drained (e.g. set a flag honored in `about_to_wait`).
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            bridge: AccessKitBridge::new(wake),
            installed: false,
            action_map: HashMap::new(),
        }
    }

    /// Whether the platform adapter is live.
    pub fn status(&self) -> BridgeStatus {
        self.bridge.status()
    }

    /// Project the current layout into an AccessKit tree (with each leaf's own
    /// semantics), install it on the first call — revealing `window`, which must
    /// have been created hidden so the adapter attaches first — and update it
    /// after. Returns the DOM nodes a screen reader asked to Click or Focus, in
    /// request order, for the caller to activate through its own click path.
    ///
    /// `focus` is the app's currently-focused DOM node's opaque id (from
    /// `LayoutDom::opaque_id`), used as the tree's focus when it is really in the
    /// tree, so a stale id never points the reader at nothing.
    pub fn sync(
        &mut self,
        window: &Window,
        dom: &ScriptedDom,
        layout: &IncrementalLayout<NodeId>,
        leaves: &mut LeafRegistry<u64>,
        focus: Option<u64>,
    ) -> Vec<NodeId> {
        let root = dom.document();
        let id_of = |d: &ScriptedDom, n: NodeId| A11yNodeId(d.opaque_id(n));
        let skip = |_: &ScriptedDom, _: NodeId| false;
        let projection = {
            let mut source = SpriggingA11y(leaves);
            project(dom, layout.fragments(), root, &id_of, &skip, &mut source, true)
        };

        let mut nodes = Vec::with_capacity(projection.nodes.len());
        self.action_map.clear();
        self.action_map.reserve(projection.nodes.len());
        for p in projection.nodes {
            self.action_map.insert(p.id, p.dom);
            nodes.push((p.id, p.node));
        }
        let node_count = nodes.len();
        let focus = focus
            .map(A11yNodeId)
            .filter(|id| self.action_map.contains_key(id))
            .unwrap_or(projection.root);
        let tree = TreeUpdate {
            nodes,
            tree: Some(Tree::new(projection.root)),
            tree_id: TreeId::ROOT,
            focus,
        };

        if !self.installed {
            match self.bridge.install(window, tree) {
                Ok(()) => eprintln!(
                    "[cambium-winit] accessibility {:?}, {node_count} nodes projected",
                    self.bridge.status()
                ),
                Err(e) => eprintln!("[cambium-winit] accessibility install failed: {e}"),
            }
            self.installed = true;
            window.set_visible(true);
            return Vec::new();
        }

        self.bridge.update(tree);
        // Route a screen reader's activations back to their DOM nodes.
        self.bridge
            .drain_actions()
            .into_iter()
            .filter(|req| matches!(req.action, Action::Click | Action::Focus))
            .filter_map(|req| self.action_map.get(&req.target_node).copied())
            .collect()
    }
}
