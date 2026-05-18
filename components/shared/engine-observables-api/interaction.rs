/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Focus, selection, input affordances, activation targets. The
//! bridge between observable geometry and user input.
//!
//! Cf. Hekate doc §"Interaction Plane". The host stores a per-tile
//! lane handle and queries this trait on input — Hekate is *not* on
//! the per-event hot path.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

use crate::types::{Point, SourceNodeId, SourceRange};

/// Common-minimum interaction queries every lane publishes.
pub trait InteractionQuery {
    /// Current focused node, if any.
    fn focus_target(&self) -> Option<SourceNodeId>;

    /// Current selection (the active range across the lane's source).
    fn selection(&self) -> Option<Selection>;

    /// Affordances at a point — what *kinds* of interaction the user
    /// can perform here (link, button, scrollable, editable, etc.).
    /// Returns multiple entries for stacked affordances (e.g., a
    /// link inside a scrollable region — both are reachable).
    fn affordances_at(&self, point: Point) -> Vec<Affordance>;

    /// What gets activated when the user clicks at `point`.
    /// Distinct from `affordances_at` — affordances is "what's
    /// possible", activation_target is "what would happen on
    /// default-click."
    fn activation_target(&self, point: Point) -> Option<SourceNodeId>;
}

/// One active selection range.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct Selection {
    /// Anchor of the selection (where the user first pressed).
    pub anchor: SourceNodeId,
    /// Focus end (where the cursor currently is). Equal to `anchor`
    /// for a caret/collapsed selection.
    pub focus: SourceNodeId,
    /// Source-range form of the selection — what `text_range_for_*`
    /// equivalents produce, normalized so `start <= end`.
    pub range: SourceRange,
}

/// One affordance the user can act on at a given point.
#[derive(Clone, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct Affordance {
    pub kind: AffordanceKind,
    /// Source node that publishes this affordance. The host uses
    /// this for hover tooltips, cursor changes, focus-ring rendering.
    pub source_node: SourceNodeId,
    /// Optional descriptive label (link href, button label, etc.).
    /// `None` if the affordance has no canonical short label.
    pub label: Option<String>,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub enum AffordanceKind {
    /// Hyperlink — left-click navigates, hover shows URL.
    Link,
    /// Button — left-click activates.
    Button,
    /// Editable region (text field, contenteditable).
    Editable,
    /// Scrollable container — wheel/drag scrolls the container.
    Scrollable,
    /// Hover target with no default activation (e.g., abbr title).
    #[default]
    Hoverable,
    /// Form input with non-text affordance (checkbox, radio,
    /// select, etc.).
    FormControl,
}
