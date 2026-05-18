/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Cross-cutting supporting types shared across the four query
//! traits. Kept minimal — promote types here only when more than one
//! trait module needs them.

use malloc_size_of_derive::MallocSizeOf;
use serde::{Deserialize, Serialize};

/// Opaque per-engine node identity. Lanes choose how to mint these;
/// consumers only compare for equality and use them as map keys.
///
/// The wire shape is `u64` so consumers can serialize hits + selection
/// ranges across IPC without needing a generic NodeId parameter.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct SourceNodeId(pub u64);

/// Half-open `[start, end)` byte-offset range into the lane's source
/// text. Returned by `text_range_for_fragment` and consumed by
/// `rects_for_selection`. Byte offsets, not chars or grapheme
/// clusters, because that's what source/edit machinery needs.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize,
)]
pub struct SourceRange {
    pub start: usize,
    pub end: usize,
}

impl SourceRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// Viewport-space point (CSS pixels, post-transform). Lane impls
/// translate from their device/layout coordinate space when needed.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Viewport-space rectangle (CSS pixels). Origin-and-size shape; we
/// avoid pulling in `euclid` here because consumers may want to use a
/// different geometry crate.
#[derive(Clone, Copy, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct Rect {
    pub origin: Point,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn new(origin: Point, width: f32, height: f32) -> Self {
        Self {
            origin,
            width,
            height,
        }
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.x < self.origin.x + self.width
            && point.y >= self.origin.y
            && point.y < self.origin.y + self.height
    }
}

/// BCP 47 language tag (e.g., "en", "en-US", "zh-Hant"). Owned String
/// because language tags don't have a single shared registry crate we
/// want to force; consumers can `Lang::from(s.to_string())` cheaply.
#[derive(Clone, Debug, Default, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct Lang(pub String);

impl Lang {
    pub fn new(tag: impl Into<String>) -> Self {
        Self(tag.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_inclusive_top_left_exclusive_bottom_right() {
        let r = Rect::new(Point::new(10.0, 10.0), 100.0, 50.0);
        assert!(r.contains(Point::new(10.0, 10.0)));
        assert!(r.contains(Point::new(50.0, 30.0)));
        assert!(!r.contains(Point::new(110.0, 10.0))); // right edge exclusive
        assert!(!r.contains(Point::new(50.0, 60.0))); // bottom edge exclusive
        assert!(!r.contains(Point::new(9.9, 30.0)));
    }

    #[test]
    fn source_range_len_and_empty() {
        assert_eq!(SourceRange::new(5, 10).len(), 5);
        assert!(SourceRange::new(5, 5).is_empty());
        assert!(SourceRange::new(10, 5).is_empty());
    }

    #[test]
    fn source_node_id_round_trips_through_serde() {
        let id = SourceNodeId(0xCAFE_BABE);
        let json = serde_json::to_string(&id).unwrap();
        let back: SourceNodeId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
