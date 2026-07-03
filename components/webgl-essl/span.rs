/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Byte-offset source spans. ESSL source is small enough that whole-source
//! byte offsets are cheap; line/column resolution is a post-hoc walk.

/// Half-open byte range `[start, end)` in the original source.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(end >= start);
        Self { start, end }
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn slice<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

/// Resolve a byte offset to a 1-based (line, column). Walks once per call;
/// fine for diagnostics, not a hot path.
pub fn line_column(src: &str, offset: usize) -> (u32, u32) {
    let prefix = &src[..offset.min(src.len())];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let col = prefix
        .rfind('\n')
        .map(|nl| prefix.len() - nl - 1)
        .unwrap_or(prefix.len()) as u32
        + 1;
    (line, col)
}
