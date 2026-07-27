/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The [`TextInput`] struct, its IME/ghost state, single-character editing, and
//! basic (non-line-aware) caret motion. [`super::multiline`] and
//! [`super::word_motion`] add more `impl TextInput` blocks over this same type.

use unicode_segmentation::UnicodeSegmentation;

use crate::editor::EditHistory;

/// The caret marker inserted into [`TextInput::display`]'s *textual* rendering
/// (never into the buffer). The on-screen field paints a real caret bar instead;
/// this is for headless tests / debug.
const CARET_MARKER: char = '|';

/// Which side of a shaped cluster owns a caret at a shared byte boundary.
///
/// Most logical editing can use [`Downstream`](Self::Downstream). Layout-aware
/// hosts preserve the affinity returned by Parley so a caret at a bidi or
/// soft-wrap boundary paints on the side the user actually moved to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CaretAffinity {
    #[default]
    Downstream,
    Upstream,
}

/// A layout-facing caret position: UTF-8 byte offset plus visual affinity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretPosition {
    pub byte: usize,
    pub affinity: CaretAffinity,
}

/// A layout-facing selection, retaining direction and affinity at both ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaretSelection {
    pub anchor: CaretPosition,
    pub focus: CaretPosition,
}

/// An in-progress IME composition. `selection` is the IME-provided byte range
/// within `text`; a collapsed range is the candidate-window caret.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Composition {
    pub text: String,
    pub selection: Option<(usize, usize)>,
}

/// The committed part of a [`TextInput`] captured for undo. Ephemeral
/// composition and completion text are deliberately excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextSnapshot {
    text: String,
    caret: usize,
    anchor: usize,
    caret_affinity: CaretAffinity,
    anchor_affinity: CaretAffinity,
}

/// The state of an editable text field: the `text` buffer plus a `caret`
/// insertion point.
///
/// `caret` is an **extended grapheme-cluster** index in
/// `0..=text.graphemes(true).count()` — it can sit before the first grapheme
/// (`0`) or after the last. Combining sequences and emoji ZWJ families are
/// therefore indivisible under arrows, Backspace, and Delete. Layout-aware
/// hosts cross the boundary through [`caret_position`](Self::caret_position)
/// and [`set_caret_position`](Self::set_caret_position), whose byte offsets and
/// affinities match Parley.
///
/// Fields are `pub(super)`: private to the outside world, but visible to the
/// `multiline` and `word_motion` sibling impls split this
/// type's behaviour across files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    pub(super) text: String,
    /// The caret — the *moving* end of the selection (where the caret paints and
    /// where insertion happens once collapsed). A grapheme index.
    pub(super) caret: usize,
    pub(super) caret_affinity: CaretAffinity,
    /// The selection's *fixed* end. `anchor == caret` means no selection (a
    /// collapsed caret); otherwise the selection spans
    /// `[min(anchor, caret), max(anchor, caret))`.
    pub(super) anchor: usize,
    pub(super) anchor_affinity: CaretAffinity,
    /// In-progress IME composition shown inline at the caret but **not** in the
    /// committed `text` (IME T2). Empty when not composing. The host sets it from
    /// `Ime::Preedit` and clears it on `Ime::Commit` (folding the committed text
    /// into the buffer). [`render_text`](Self::render_text) splices it at the
    /// caret; [`caret_byte_in_render`](Self::caret_byte_in_render) is where the
    /// IME caret then sits.
    pub(super) composition: Option<Composition>,
    /// An inline autocomplete suffix shown dim **after** the text but **not** in
    /// the committed `text` — fish/omnibar-style ghost completion. The host sets it
    /// from whatever vocabulary it completes against; [`accept_ghost`](Self::accept_ghost)
    /// (the host's → / Tab) splices it into the buffer. It is deliberately outside
    /// [`render_text`](Self::render_text) and the caret geometry, so submitting
    /// evaluates only what was actually typed, never the suggestion.
    pub(super) ghost: String,
    /// The sticky **goal column** for vertical motion (ArrowUp/ArrowDown): the
    /// grapheme column the caret aims for, preserved across a *run* of up/down
    /// presses so the caret does not drift toward shorter lines (Tier 2). `Some` only mid-run; any
    /// horizontal move or edit clears it ([`reset_goal`](Self::reset_goal)) so the
    /// next vertical move re-seeds it from the caret's actual column.
    pub(super) goal_col: Option<usize>,
    /// The default editing path owns its undo/redo journal. A host that needs
    /// transaction-level grouping can still keep a separate [`EditHistory`].
    pub(super) history: EditHistory,
}

impl TextInput {
    /// A field holding `text`, with the caret collapsed at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.graphemes(true).count();
        Self {
            text,
            caret,
            caret_affinity: CaretAffinity::Downstream,
            anchor: caret,
            anchor_affinity: CaretAffinity::Downstream,
            composition: None,
            ghost: String::new(),
            goal_col: None,
            history: EditHistory::new(),
        }
    }

    /// Drop the sticky vertical [`goal_col`](Self::goal_col). Every caret move or
    /// edit that is *not* ArrowUp/ArrowDown calls this, so the goal column lives only
    /// for an uninterrupted run of vertical presses; the next one re-seeds it from the
    /// caret's real column.
    pub(super) fn reset_goal(&mut self) {
        self.goal_col = None;
    }

    /// The buffer, without the caret marker.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The in-progress IME composition (empty when not composing).
    pub fn preedit(&self) -> &str {
        self.composition
            .as_ref()
            .map_or("", |composition| composition.text.as_str())
    }

    /// The full in-progress IME composition, including the IME-provided
    /// selection/caret range within the preedit string.
    pub fn composition(&self) -> Option<&Composition> {
        self.composition.as_ref()
    }

    /// Set the IME composing text (from `Ime::Preedit`). Shown inline at the
    /// caret by [`render_text`](Self::render_text); not in the committed buffer.
    pub fn set_preedit(&mut self, text: impl Into<String>) {
        let text = text.into();
        let end = text.len();
        self.set_composition(text, Some((end, end)));
    }

    /// Set the IME composition and its byte selection/caret within `text`.
    pub fn set_composition(&mut self, text: impl Into<String>, selection: Option<(usize, usize)>) {
        let text = text.into();
        let selection = selection.and_then(|(anchor, focus)| {
            let snap = |byte: usize| {
                let byte = byte.min(text.len());
                (0..=byte)
                    .rev()
                    .find(|&candidate| text.is_char_boundary(candidate))
                    .unwrap_or(0)
            };
            Some((snap(anchor), snap(focus)))
        });
        self.composition = (!text.is_empty()).then_some(Composition { text, selection });
    }

    /// Clear the IME composition (on `Ime::Commit` / `Ime::Disabled`).
    pub fn clear_preedit(&mut self) {
        self.composition = None;
    }

    /// The inline autocomplete suffix (empty when there is no completion).
    pub fn ghost(&self) -> &str {
        &self.ghost
    }

    /// Set the ghost-completion suffix shown dim after the text. Not committed to
    /// the buffer until [`accept_ghost`](Self::accept_ghost).
    pub fn set_ghost(&mut self, text: impl Into<String>) {
        self.ghost = text.into();
    }

    /// Clear the ghost suffix.
    pub fn clear_ghost(&mut self) {
        self.ghost.clear();
    }

    /// Select the entire buffer (Ctrl / Cmd + A): anchor at the start, caret at
    /// the end, so the next edit replaces everything.
    pub fn select_all(&mut self) {
        self.reset_goal();
        self.anchor = 0;
        self.anchor_affinity = CaretAffinity::Downstream;
        self.caret = self.grapheme_count();
        self.caret_affinity = CaretAffinity::Upstream;
    }

    /// Splice the ghost suffix into the buffer (the host's → / Tab): append it,
    /// move the caret to the end, and clear the ghost. A no-op when there is no
    /// ghost. The buffer is the source of truth, so this is the only way ghost
    /// text ever enters [`text`](Self::text).
    pub fn accept_ghost(&mut self) {
        if self.ghost.is_empty() {
            return;
        }
        self.reset_goal();
        self.text.push_str(&self.ghost);
        self.ghost.clear();
        self.caret = self.grapheme_count();
        self.caret_affinity = CaretAffinity::Upstream;
        self.anchor = self.caret;
        self.anchor_affinity = self.caret_affinity;
    }

    /// The text to render: the buffer with any IME preedit spliced in at the
    /// caret. Equals the buffer when not composing.
    pub fn render_text(&self) -> String {
        let Some(composition) = &self.composition else {
            return self.text.clone();
        };
        let (lo, hi) = self.selection();
        let start = self.byte_of(lo);
        let end = self.byte_of(hi);
        let mut s = String::with_capacity(self.text.len() - (end - start) + composition.text.len());
        s.push_str(&self.text[..start]);
        s.push_str(&composition.text);
        s.push_str(&self.text[end..]);
        s
    }

    /// The caret's byte offset within [`render_text`](Self::render_text) — after
    /// the spliced preedit while composing, else the plain caret. This is where
    /// the painted caret and the IME candidate area sit.
    pub fn caret_byte_in_render(&self) -> usize {
        let (lo, _) = self.selection();
        let start = self.byte_of(lo);
        let Some(composition) = &self.composition else {
            return self.byte_of(self.caret);
        };
        let within = composition
            .selection
            .map_or(composition.text.len(), |(_, focus)| focus);
        start + within.min(composition.text.len())
    }

    /// The rendered text split at the caret into `(before, preedit, after)`, so
    /// the field can render the IME preedit as a distinct (underlined) span. The
    /// three concatenate to [`render_text`](Self::render_text); `preedit` is empty
    /// when not composing.
    pub fn render_parts(&self) -> (String, String, String) {
        let (lo, hi) = self.selection();
        let start = self.byte_of(lo);
        let end = self.byte_of(hi);
        (
            self.text[..start].to_string(),
            self.preedit().to_owned(),
            self.text[end..].to_string(),
        )
    }

    /// The caret (moving end): a grapheme index.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The caret as the byte offset and affinity consumed by layout.
    pub fn caret_position(&self) -> CaretPosition {
        CaretPosition {
            byte: self.byte_of(self.caret),
            affinity: self.caret_affinity,
        }
    }

    /// The selection's fixed end (anchor); equals [`caret`](Self::caret) when
    /// nothing is selected.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// The directed selection in layout byte space.
    pub fn caret_selection(&self) -> CaretSelection {
        CaretSelection {
            anchor: CaretPosition {
                byte: self.byte_of(self.anchor),
                affinity: self.anchor_affinity,
            },
            focus: self.caret_position(),
        }
    }

    /// Whether a non-empty range is selected.
    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    /// The selected char range `[start, end)`, ordered. Empty (`start == end`)
    /// when nothing is selected.
    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// The currently selected substring (empty when nothing is selected) — the
    /// source for copy / cut.
    pub fn selected_text(&self) -> &str {
        let (lo, hi) = self.selection_bytes();
        &self.text[lo..hi]
    }

    /// The selected byte range `[start, end)`, ordered.
    pub fn selection_bytes(&self) -> (usize, usize) {
        let (lo, hi) = self.selection();
        (self.byte_of(lo), self.byte_of(hi))
    }

    /// The number of grapheme clusters in the buffer (the caret's upper bound).
    pub(super) fn grapheme_count(&self) -> usize {
        self.text.graphemes(true).count()
    }

    /// Byte offset of the `i`-th grapheme boundary, or the buffer end when
    /// `i == grapheme_count` (the past-the-last-grapheme insertion point).
    pub(super) fn byte_of(&self, i: usize) -> usize {
        self.text
            .grapheme_indices(true)
            .nth(i)
            .map(|(byte, _)| byte)
            .unwrap_or(self.text.len())
    }

    /// Grapheme index at a byte offset, snapping to the preceding grapheme
    /// boundary. This is the inverse of [`byte_of`](Self::byte_of).
    pub(super) fn grapheme_of_byte(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        self.text[..byte].graphemes(true).count()
    }

    /// Delete the selected range and collapse the caret to its start. No-op when
    /// nothing is selected.
    pub(super) fn delete_selection(&mut self) {
        if !self.has_selection() {
            return;
        }
        let (lo, hi) = self.selection();
        let start = self.byte_of(lo);
        let end = self.byte_of(hi);
        self.text.replace_range(start..end, "");
        self.caret = lo;
        self.caret_affinity = CaretAffinity::Downstream;
        self.anchor = lo;
        self.anchor_affinity = self.caret_affinity;
    }

    /// Insert `s` at the caret, replacing any selection first; collapses the
    /// caret after the inserted text.
    pub fn insert_str(&mut self, s: &str) {
        self.reset_goal();
        self.delete_selection();
        let at = self.byte_of(self.caret);
        self.text.insert_str(at, s);
        self.caret += s.graphemes(true).count();
        self.caret_affinity = CaretAffinity::Downstream;
        self.anchor = self.caret;
        self.anchor_affinity = self.caret_affinity;
    }

    /// Backspace: delete the selection if any, else the character before the
    /// caret. No-op at the start of an unselected buffer.
    pub fn backspace(&mut self) {
        self.reset_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        if self.caret == 0 {
            return;
        }
        let start = self.byte_of(self.caret - 1);
        let end = self.byte_of(self.caret);
        self.text.replace_range(start..end, "");
        self.caret -= 1;
        self.caret_affinity = CaretAffinity::Downstream;
        self.anchor = self.caret;
        self.anchor_affinity = self.caret_affinity;
    }

    /// Delete: remove the selection if any, else the character after the caret.
    /// No-op at the end of an unselected buffer.
    pub fn delete(&mut self) {
        self.reset_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        if self.caret >= self.grapheme_count() {
            return;
        }
        let start = self.byte_of(self.caret);
        let end = self.byte_of(self.caret + 1);
        self.text.replace_range(start..end, "");
        self.caret_affinity = CaretAffinity::Downstream;
        self.anchor = self.caret;
        self.anchor_affinity = self.caret_affinity;
    }

    /// Move the caret one character left. `extend` keeps the anchor (growing the
    /// selection, Shift+←); otherwise it collapses — to the selection's left edge
    /// if one exists, else one char left.
    pub fn move_left(&mut self, extend: bool) {
        self.reset_goal();
        if !extend && self.has_selection() {
            self.caret = self.selection().0;
        } else {
            self.caret = self.caret.saturating_sub(1);
        }
        self.caret_affinity = CaretAffinity::Downstream;
        if !extend {
            self.anchor = self.caret;
            self.anchor_affinity = self.caret_affinity;
        }
    }

    /// Move the caret one character right. `extend` keeps the anchor (Shift+→);
    /// otherwise it collapses to the selection's right edge if one exists, else
    /// one char right.
    pub fn move_right(&mut self, extend: bool) {
        self.reset_goal();
        if !extend && self.has_selection() {
            self.caret = self.selection().1;
        } else if self.caret < self.grapheme_count() {
            self.caret += 1;
        }
        self.caret_affinity = CaretAffinity::Downstream;
        if !extend {
            self.anchor = self.caret;
            self.anchor_affinity = self.caret_affinity;
        }
    }

    /// Move the caret to the start (Home). `extend` keeps the anchor (selecting
    /// to the start).
    pub fn home(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = 0;
        self.caret_affinity = CaretAffinity::Downstream;
        if !extend {
            self.anchor = 0;
            self.anchor_affinity = self.caret_affinity;
        }
    }

    /// Move the caret to the end (End). `extend` keeps the anchor (selecting to
    /// the end).
    pub fn end(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = self.grapheme_count();
        self.caret_affinity = CaretAffinity::Upstream;
        if !extend {
            self.anchor = self.caret;
            self.anchor_affinity = self.caret_affinity;
        }
    }

    /// Set the caret to the character boundary at byte offset `byte` (clamped to
    /// a valid boundary and the buffer end). `extend` keeps the anchor, growing
    /// the selection. The host drives this from the laid-out text — soft-wrap
    /// ArrowUp/ArrowDown and click-to-place hit-test parley and yield a byte
    /// offset, which maps back to this grapheme-index model here.
    pub fn set_caret_byte(&mut self, byte: usize, extend: bool) {
        self.set_caret_position(
            CaretPosition {
                byte,
                affinity: CaretAffinity::Downstream,
            },
            extend,
        );
    }

    /// Set the caret from a layout result, preserving visual affinity at bidi
    /// and soft-wrap boundaries.
    pub fn set_caret_position(&mut self, position: CaretPosition, extend: bool) {
        self.reset_goal();
        let byte = position.byte.min(self.text.len());
        let byte = if byte == self.text.len() {
            byte
        } else {
            self.text
                .grapheme_indices(true)
                .map(|(candidate, _)| candidate)
                .take_while(|candidate| *candidate <= byte)
                .last()
                .unwrap_or(0)
        };
        self.caret = self.grapheme_of_byte(byte);
        self.caret_affinity = position.affinity;
        if !extend {
            self.anchor = self.caret;
            self.anchor_affinity = self.caret_affinity;
        }
    }

    /// Replace both selection endpoints from layout byte space.
    pub fn set_caret_selection(&mut self, selection: CaretSelection) {
        self.set_caret_position(selection.anchor, false);
        self.set_caret_position(selection.focus, true);
    }

    /// Whether the default command path has an undo entry.
    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    /// Whether the default command path has a redo entry.
    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub(crate) fn snapshot(&self) -> TextSnapshot {
        TextSnapshot {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
            caret_affinity: self.caret_affinity,
            anchor_affinity: self.anchor_affinity,
        }
    }

    pub(crate) fn restore(&mut self, snapshot: TextSnapshot) {
        self.text = snapshot.text;
        self.caret = snapshot.caret;
        self.anchor = snapshot.anchor;
        self.caret_affinity = snapshot.caret_affinity;
        self.anchor_affinity = snapshot.anchor_affinity;
        self.composition = None;
        self.ghost.clear();
        self.goal_col = None;
    }

    /// The buffer with a `CARET_MARKER` inserted at the caret — the field's
    /// rendered text (a placeholder visible cursor). Render-only: [`text`](Self::text)
    /// is unchanged.
    pub fn display(&self) -> String {
        let at = self.byte_of(self.caret);
        let mut shown = self.text.clone();
        shown.insert(at, CARET_MARKER);
        shown
    }
}
