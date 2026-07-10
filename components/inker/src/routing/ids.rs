/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Kernel-free routing context identities.
//!
//! Routing takes optional host-graph context (which node / which view asked)
//! for per-node engine pins and surface-target minting. These identities are
//! deliberately taken from neutral crates rather than the host's graph kernel
//! (the seiche precedent), so inker stays portable to hosts that are not mere.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable node handle in the host's graph: petgraph's `NodeIndex`, taken
/// directly. Mere's `kernel::graph::NodeKey` is a type alias for the same
/// type, so mere-side call sites pass theirs through unchanged.
pub type NodeKey = petgraph::graph::NodeIndex;

/// Host view identity used as optional routing context. Inker's own newtype;
/// a host converts from its view id via [`RouteViewId::from_uuid`] (mere:
/// `RouteViewId::from_uuid(graph_view_id.as_uuid())`). Routing never mints
/// one, so there is no random constructor and no wasm randomness concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RouteViewId(Uuid);

impl RouteViewId {
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}
