/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The [`TextInput`] struct, its IME/ghost state, single-character editing, and
//! basic (non-line-aware) caret motion. [`super::multiline`] and
//! [`super::word_motion`] add more `impl TextInput` blocks over this same type.

/// The caret marker inserted into [`TextInput::display`]'s *textual* rendering
/// (never into the buffer). The on-screen field paints a real caret bar instead;
/// this is for headless tests / debug.
const CARET_MARKER: char = '|';

/// The state of an editable text field: the `text` buffer plus a `caret`
/// insertion point.
///
/// `caret` is a **character** index in `0..=text.chars().count()` — it can sit
/// before the first char (`0`) or after the last (`char_count`). Keeping it in
/// char units (not bytes) makes every edit correct for multi-byte UTF-8 (e.g.
/// inserting between the two chars of `"é!"`). The caret is genuinely part of
/// the field's logical state (a host can read or set the cursor), so it lives
/// here rather than in ephemeral view state — which also keeps the field a plain
/// `on_key` + `fn` rather than a bespoke `View`.
///
/// Fields are `pub(super)`: private to the outside world, but visible to the
/// `multiline` and `word_motion` sibling impls split this
/// type's behaviour across files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    pub(super) text: String,
    /// The caret — the *moving* end of the selection (where the caret paints and
    /// where insertion happens once collapsed). A char index in `0..=char_count`.
    pub(super) caret: usize,
    /// The selection's *fixed* end. `anchor == caret` means no selection (a
    /// collapsed caret); otherwise the selection spans
    /// `[min(anchor, caret), max(anchor, caret))`.
    pub(super) anchor: usize,
    /// In-progress IME composition shown inline at the caret but **not** in the
    /// committed `text` (IME T2). Empty when not composing. The host sets it from
    /// `Ime::Preedit` and clears it on `Ime::Commit` (folding the committed text
    /// into the buffer). [`render_text`](Self::render_text) splices it at the
    /// caret; [`caret_with_preedit`](Self::caret_with_preedit) is where the caret
    /// then sits.
    pub(super) preedit: String,
    /// An inline autocomplete suffix shown dim **after** the text but **not** in
    /// the committed `text` — fish/omnibar-style ghost completion. The host sets it
    /// from whatever vocabulary it completes against; [`accept_ghost`](Self::accept_ghost)
    /// (the host's → / Tab) splices it into the buffer. It is deliberately outside
    /// [`render_text`](Self::render_text) and the caret geometry, so submitting
    /// evaluates only what was actually typed, never the suggestion.
    pub(super) ghost: String,
    /// The sticky **goal column** for vertical motion (ArrowUp/ArrowDown): the char
    /// column the caret aims for, preserved across a *run* of up/down presses so the
    /// caret does not drift toward shorter lines (Tier 2). `Some` only mid-run; any
    /// horizontal move or edit clears it ([`reset_goal`](Self::reset_goal)) so the
    /// next vertical move re-seeds it from the caret's actual column.
    pub(super) goal_col: Option<usize>,
}

impl TextInput {
    /// A field holding `text`, with the caret collapsed at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.chars().count();
        Self {
            text,
            caret,
            anchor: caret,
            preedit: String::new(),
            ghost: String::new(),
            goal_col: None,
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
        &self.preedit
    }

    /// Set the IME composing text (from `Ime::Preedit`). Shown inline at the
    /// caret by [`render_text`](Self::render_text); not in the committed buffer.
    pub fn set_preedit(&mut self, text: impl Into<String>) {
        self.preedit = text.into();
    }

    /// Clear the IME composition (on `Ime::Commit` / `Ime::Disabled`).
    pub fn clear_preedit(&mut self) {
        self.preedit.clear();
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
        self.caret = self.char_count();
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
        self.caret = self.char_count();
        self.anchor = self.caret;
    }

    /// The text to render: the buffer with any IME preedit spliced in at the
    /// caret. Equals the buffer when not composing.
    pub fn render_text(&self) -> String {
        if self.preedit.is_empty() {
            return self.text.clone();
        }
        let at = self.byte_of(self.caret);
        let mut s = self.text.clone();
        s.insert_str(at, &self.preedit);
        s
    }

    /// The caret's byte offset within [`render_text`](Self::render_text) — after
    /// the spliced preedit while composing, else the plain caret. This is where
    /// the painted caret and the IME candidate area sit.
    pub fn caret_byte_in_render(&self) -> usize {
        self.byte_of(self.caret) + self.preedit.len()
    }

    /// The rendered text split at the caret into `(before, preedit, after)`, so
    /// the field can render the IME preedit as a distinct (underlined) span. The
    /// three concatenate to [`render_text`](Self::render_text); `preedit` is empty
    /// when not composing.
    pub fn render_parts(&self) -> (String, String, String) {
        let at = self.byte_of(self.caret);
        (
            self.text[..at].to_string(),
            self.preedit.clone(),
            self.text[at..].to_string(),
        )
    }

    /// The caret (moving end): a character index in `0..=char_count`.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selection's fixed end (anchor); equals [`caret`](Self::caret) when
    /// nothing is selected.
    pub fn anchor(&self) -> usize {
        self.anchor
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
        let (lo, hi) = self.selection();
        &self.text[self.byte_of(lo)..self.byte_of(hi)]
    }

    /// The number of characters in the buffer (the caret's upper bound).
    pub(super) fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of the `i`-th character boundary, or the buffer end when
    /// `i == char_count` (the past-the-last-char insertion point).
    pub(super) fn byte_of(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
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
        self.anchor = lo;
    }

    /// Insert `s` at the caret, replacing any selection first; collapses the
    /// caret after the inserted text.
    pub fn insert_str(&mut self, s: &str) {
        self.reset_goal();
        self.delete_selection();
        let at = self.byte_of(self.caret);
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
        self.anchor = self.caret;
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
        self.anchor = self.caret;
    }

    /// Delete: remove the selection if any, else the character after the caret.
    /// No-op at the end of an unselected buffer.
    pub fn delete(&mut self) {
        self.reset_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        if self.caret >= self.char_count() {
            return;
        }
        let start = self.byte_of(self.caret);
        let end = self.byte_of(self.caret + 1);
        self.text.replace_range(start..end, "");
        self.anchor = self.caret;
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
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret one character right. `extend` keeps the anchor (Shift+→);
    /// otherwise it collapses to the selection's right edge if one exists, else
    /// one char right.
    pub fn move_right(&mut self, extend: bool) {
        self.reset_goal();
        if !extend && self.has_selection() {
            self.caret = self.selection().1;
        } else if self.caret < self.char_count() {
            self.caret += 1;
        }
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret to the start (Home). `extend` keeps the anchor (selecting
    /// to the start).
    pub fn home(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = 0;
        if !extend {
            self.anchor = 0;
        }
    }

    /// Move the caret to the end (End). `extend` keeps the anchor (selecting to
    /// the end).
    pub fn end(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = self.char_count();
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Set the caret to the character boundary at byte offset `byte` (clamped to
    /// a valid boundary and the buffer end). `extend` keeps the anchor, growing
    /// the selection. The host drives this from the laid-out text — soft-wrap
    /// ArrowUp/ArrowDown and click-to-place hit-test parley and yield a byte
    /// offset, which maps back to this char-index model here.
    pub fn set_caret_byte(&mut self, byte: usize, extend: bool) {
        self.reset_goal();
        let byte = byte.min(self.text.len());
        // Snap to the char boundary at or below `byte` before counting chars
        // (parley returns cluster boundaries, but clamp defensively).
        let byte = (0..=byte)
            .rev()
            .find(|&b| self.text.is_char_boundary(b))
            .unwrap_or(0);
        self.caret = self.text[..byte].chars().count();
        if !extend {
            self.anchor = self.caret;
        }
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
