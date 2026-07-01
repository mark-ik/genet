/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`TextInput`]: the buffer + caret state behind every text control
//! ([`text_field`](crate::text_field) / [`textarea`](crate::textarea) and their
//! styled siblings in [`styled_field`](crate::styled_field)).
//!
//! ## Caret
//!
//! The field is a real insertion-point editor, not append-only: [`TextInput`]
//! carries a `caret` (a *character* index, so editing is Unicode-correct), and
//! the handler inserts at the caret, deletes before/after it (Backspace/Delete),
//! and moves it (←/→ and Home/End). The field renders the **clean** buffer; the
//! host paints the caret as a thin bar at the cursor via
//! `serval_layout::caret_rect` overlaid on the scene (see `pelt-live`'s render
//! path). [`TextInput::display`] — the buffer with a `|` at the caret — is a
//! *textual* representation for headless tests / debug, not what the field
//! renders on screen.

use unicode_segmentation::UnicodeSegmentation;

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
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextInput {
    text: String,
    /// The caret — the *moving* end of the selection (where the caret paints and
    /// where insertion happens once collapsed). A char index in `0..=char_count`.
    caret: usize,
    /// The selection's *fixed* end. `anchor == caret` means no selection (a
    /// collapsed caret); otherwise the selection spans
    /// `[min(anchor, caret), max(anchor, caret))`.
    anchor: usize,
    /// In-progress IME composition shown inline at the caret but **not** in the
    /// committed `text` (IME T2). Empty when not composing. The host sets it from
    /// `Ime::Preedit` and clears it on `Ime::Commit` (folding the committed text
    /// into the buffer). [`render_text`](Self::render_text) splices it at the
    /// caret; [`caret_with_preedit`](Self::caret_with_preedit) is where the caret
    /// then sits.
    preedit: String,
    /// An inline autocomplete suffix shown dim **after** the text but **not** in
    /// the committed `text` — fish/omnibar-style ghost completion. The host sets it
    /// from whatever vocabulary it completes against; [`accept_ghost`](Self::accept_ghost)
    /// (the host's → / Tab) splices it into the buffer. It is deliberately outside
    /// [`render_text`](Self::render_text) and the caret geometry, so submitting
    /// evaluates only what was actually typed, never the suggestion.
    ghost: String,
    /// The sticky **goal column** for vertical motion (ArrowUp/ArrowDown): the char
    /// column the caret aims for, preserved across a *run* of up/down presses so the
    /// caret does not drift toward shorter lines (Tier 2). `Some` only mid-run; any
    /// horizontal move or edit clears it ([`reset_goal`](Self::reset_goal)) so the
    /// next vertical move re-seeds it from the caret's actual column.
    goal_col: Option<usize>,
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
    fn reset_goal(&mut self) {
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
        (self.text[..at].to_string(), self.preedit.clone(), self.text[at..].to_string())
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
    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// Byte offset of the `i`-th character boundary, or the buffer end when
    /// `i == char_count` (the past-the-last-char insertion point).
    fn byte_of(&self, i: usize) -> usize {
        self.text
            .char_indices()
            .nth(i)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Delete the selected range and collapse the caret to its start. No-op when
    /// nothing is selected.
    fn delete_selection(&mut self) {
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

    // --- multi-line navigation (textarea) -------------------------------------
    //
    // Lines are `\n`-delimited in the buffer (serval feeds the raw text to
    // parley, which breaks at `\n`); a column is the char offset within a line.
    // Up/down keep a sticky goal column ([`goal_col`](Self::goal_col)) across a run
    // (Tier 2). These walk hard `\n` lines, not parley's soft-wrap visual rows — the
    // soft-wrap caret (`serval_layout::caret_byte_vertical`) is a separate, layout-aware
    // path a host can wire instead, where the goal would be an x-position not a column.

    /// Char offsets where each line begins: 0, then one past each `\n`.
    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        for (i, ch) in self.text.chars().enumerate() {
            if ch == '\n' {
                starts.push(i + 1);
            }
        }
        starts
    }

    /// The caret's `(line, column)`: `line` counts `\n`s before it, `column` is
    /// the char offset since that line's start.
    fn line_col(&self) -> (usize, usize) {
        let starts = self.line_starts();
        let line = starts.iter().rposition(|&s| s <= self.caret).unwrap_or(0);
        (line, self.caret - starts[line])
    }

    /// The caret char-offset at `(line, column)`, clamping the column to the
    /// line's length and the line to the last line.
    fn offset_at(&self, line: usize, column: usize) -> usize {
        let starts = self.line_starts();
        let line = line.min(starts.len() - 1);
        let start = starts[line];
        // Line end: the char before the next line's start (the `\n`), or buffer
        // end on the last line.
        let end = starts.get(line + 1).map(|&s| s - 1).unwrap_or(self.char_count());
        start.saturating_add(column).min(end)
    }

    /// Move the caret up one line, keeping the **goal column** (ArrowUp). At the
    /// first line it goes to the buffer start. `extend` grows the selection.
    ///
    /// The goal column ([`goal_col`](Self::goal_col)) is seeded from the caret's real
    /// column at the start of a vertical run and reused (clamped per line) thereafter,
    /// so arrowing up/down through varying-length lines does not drift toward the short
    /// ones (Tier 2). It is *not* cleared here — only a horizontal move or edit clears
    /// it (via [`reset_goal`](Self::reset_goal)).
    pub fn move_up(&mut self, extend: bool) {
        let (line, col) = self.line_col();
        let goal = self.goal_col.unwrap_or(col);
        self.goal_col = Some(goal);
        self.caret = if line == 0 { 0 } else { self.offset_at(line - 1, goal) };
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret down one line, keeping the **goal column** (ArrowDown). At the
    /// last line it goes to the buffer end. `extend` grows the selection. See
    /// [`move_up`](Self::move_up) for the goal-column contract.
    pub fn move_down(&mut self, extend: bool) {
        let (line, col) = self.line_col();
        let goal = self.goal_col.unwrap_or(col);
        self.goal_col = Some(goal);
        let last = self.line_starts().len() - 1;
        self.caret = if line == last { self.char_count() } else { self.offset_at(line + 1, goal) };
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret to the start of its line (Home, multi-line). `extend`
    /// grows the selection.
    pub fn home_line(&mut self, extend: bool) {
        self.reset_goal();
        let (line, _) = self.line_col();
        self.caret = self.offset_at(line, 0);
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret to the end of its line (End, multi-line). `extend` grows
    /// the selection.
    pub fn end_line(&mut self, extend: bool) {
        self.reset_goal();
        let (line, _) = self.line_col();
        self.caret = self.offset_at(line, usize::MAX);
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
        let byte = (0..=byte).rev().find(|&b| self.text.is_char_boundary(b)).unwrap_or(0);
        self.caret = self.text[..byte].chars().count();
        if !extend {
            self.anchor = self.caret;
        }
    }

    // --- word motion (Ctrl/Alt + ←/→, Backspace, Delete) ----------------------
    //
    // Word boundaries are UAX#29 (`unicode-segmentation`), computed over the buffer
    // here — the buffer owns segmentation; the host only routes the modified key. A
    // "word" is any non-whitespace segment, so motion stops at the edges of words and
    // of punctuation runs (e.g. djot `**`, backticks), skipping the whitespace between.

    /// The char index at byte offset `byte` (clamped) — inverse of
    /// [`byte_of`](Self::byte_of), mapping a word boundary back to the char model.
    fn char_of_byte(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len());
        self.text[..byte].chars().count()
    }

    /// Byte offset one word right of byte `from`: skip whitespace at/after `from`, then
    /// land at the end of the next non-whitespace segment. Buffer end when none remains.
    fn word_boundary_right(&self, from: usize) -> usize {
        for (start, seg) in self.text.split_word_bound_indices() {
            let end = start + seg.len();
            if end <= from || seg.chars().all(char::is_whitespace) {
                continue;
            }
            return end;
        }
        self.text.len()
    }

    /// Byte offset one word left of byte `from`: the start of the nearest non-whitespace
    /// segment beginning before `from`. `0` when none precedes it.
    fn word_boundary_left(&self, from: usize) -> usize {
        let mut target = 0;
        for (start, seg) in self.text.split_word_bound_indices() {
            if start >= from {
                break;
            }
            if !seg.chars().all(char::is_whitespace) {
                target = start;
            }
        }
        target
    }

    /// Move the caret one word left (Ctrl/Alt+←). `extend` grows the selection.
    pub fn move_word_left(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = self.char_of_byte(self.word_boundary_left(self.byte_of(self.caret)));
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret one word right (Ctrl/Alt+→). `extend` grows the selection.
    pub fn move_word_right(&mut self, extend: bool) {
        self.reset_goal();
        self.caret = self.char_of_byte(self.word_boundary_right(self.byte_of(self.caret)));
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Delete to the previous word boundary (Ctrl/Alt+Backspace): the selection if one
    /// exists, else the word before the caret. No-op at the buffer start.
    pub fn delete_word_left(&mut self) {
        self.reset_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let from = self.byte_of(self.caret);
        let target = self.word_boundary_left(from);
        if target < from {
            self.text.replace_range(target..from, "");
            self.caret = self.char_of_byte(target);
            self.anchor = self.caret;
        }
    }

    /// Delete to the next word boundary (Ctrl/Alt+Delete): the selection if one exists,
    /// else the word after the caret. No-op at the buffer end.
    pub fn delete_word_right(&mut self) {
        self.reset_goal();
        if self.has_selection() {
            self.delete_selection();
            return;
        }
        let from = self.byte_of(self.caret);
        let target = self.word_boundary_right(from);
        if target > from {
            self.text.replace_range(from..target, "");
            self.anchor = self.caret; // the caret stays; the text after it shrank
        }
    }

    /// The buffer with a [`CARET_MARKER`] inserted at the caret — the field's
    /// rendered text (a placeholder visible cursor). Render-only: [`text`](Self::text)
    /// is unchanged.
    pub fn display(&self) -> String {
        let at = self.byte_of(self.caret);
        let mut shown = self.text.clone();
        shown.insert(at, CARET_MARKER);
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::TextInput;

    /// A `TextInput` with the caret at char index `caret`. Fixtures are ASCII, so a
    /// char index equals a byte offset for [`TextInput::set_caret_byte`].
    fn at(text: &str, caret: usize) -> TextInput {
        let mut t = TextInput::new(text);
        t.set_caret_byte(caret, false);
        t
    }

    // --- sticky goal column (Tier 2) ------------------------------------------

    #[test]
    fn goal_column_sticks_through_a_short_line() {
        // Line lengths 5 / 2 / 5; start at column 3 of the first line.
        let mut t = at("aaaaa\nbb\nccccc", 3);
        t.move_down(false); // line "bb" clamps the column to its end (2)
        assert_eq!(t.caret(), 8); // index 8 == end of "bb"
        t.move_down(false); // line 3: the *goal* (3) is restored, not the clamped 2
        assert_eq!(t.caret(), 12); // line-3 start (9) + 3
    }

    #[test]
    fn a_horizontal_move_resets_the_goal_column() {
        let mut t = at("aaaaa\nbb\nccccc", 3);
        t.move_down(false); // -> index 8, clamped onto "bb"
        t.move_left(false); // resets the goal; caret now index 7, column 1
        t.move_down(false); // line 3 at the *new* column 1, not the old goal 3
        assert_eq!(t.caret(), 10); // 9 + 1
    }

    #[test]
    fn shift_vertical_extends_the_selection_keeping_the_goal() {
        let mut t = at("aaaaa\nbb\nccccc", 3);
        t.move_down(true); // extend onto "bb"; anchor stays at 3
        assert!(t.has_selection());
        assert_eq!(t.selection(), (3, 8));
        t.move_down(true); // goal 3 restored on "ccccc" (index 12); anchor still 3
        assert_eq!(t.selection(), (3, 12));
    }

    #[test]
    fn goal_persists_across_a_top_bounce() {
        let mut t = at("aaaaa\nbb\nccccc", 3); // line 0, column 3
        t.move_up(false); // already the top line -> buffer start; the goal (3) stays
        assert_eq!(t.caret(), 0);
        t.move_down(false); // the goal 3 (not the landed col 0) drives the descent
        assert_eq!(t.caret(), 8); // line "bb" clamps column 3 to its end (index 8)
    }

    // --- word motion (UAX#29) -------------------------------------------------

    #[test]
    fn word_motion_skips_whitespace_and_stops_at_word_edges() {
        let mut t = at("foo bar baz", 0);
        t.move_word_right(false);
        assert_eq!(t.caret(), 3); // end of "foo"
        t.move_word_right(false);
        assert_eq!(t.caret(), 7); // skip the space, end of "bar"
        t.move_word_left(false);
        assert_eq!(t.caret(), 4); // back to the start of "bar"
        t.move_word_left(false);
        assert_eq!(t.caret(), 0); // start of "foo"
    }

    #[test]
    fn dotted_token_is_one_word_uax29() {
        // UAX#29 joins a period between letters (URLs, filenames, `foo.bar()` in code),
        // so Ctrl+→ jumps the whole token rather than stopping at each '.'.
        let mut t = at("see foo.bar end", 0);
        t.move_word_right(false);
        assert_eq!(t.caret(), 3); // "see"
        t.move_word_right(false);
        assert_eq!(t.caret(), 11); // "foo.bar" as one token, not split at '.'
    }

    #[test]
    fn shift_word_right_extends_the_selection() {
        let mut t = at("foo bar", 0);
        t.move_word_right(true);
        assert!(t.has_selection());
        assert_eq!(t.selection(), (0, 3));
    }

    #[test]
    fn kill_word_left_removes_the_preceding_word() {
        let mut t = TextInput::new("foo bar baz"); // caret at the end
        t.delete_word_left();
        assert_eq!(t.text(), "foo bar ");
        assert_eq!(t.caret(), 8);
    }

    #[test]
    fn kill_word_right_removes_the_following_word() {
        let mut t = at("foo bar baz", 0);
        t.delete_word_right();
        assert_eq!(t.text(), " bar baz");
        assert_eq!(t.caret(), 0);
    }

    #[test]
    fn word_motion_handles_multibyte_utf8() {
        // "héllo wörld": multi-byte é/ö, so byte offsets != char indices. Word motion must
        // return CHAR indices and never split a codepoint.
        let mut t = at("héllo wörld", 0);
        t.move_word_right(false);
        assert_eq!(t.caret(), 5); // end of "héllo" (5 chars), past the 2-byte é
        t.move_word_right(false);
        assert_eq!(t.caret(), 11); // end of "wörld" (11 chars total)
        t.move_word_left(false);
        assert_eq!(t.caret(), 6); // back to the start of "wörld"
    }

    #[test]
    fn kill_word_deletes_the_selection_when_one_exists() {
        let mut t = at("foo bar baz", 0);
        t.move_word_right(true); // select "foo"
        t.move_word_right(true); // select "foo bar"
        assert_eq!(t.selection(), (0, 7));
        t.delete_word_left(); // the selection wins; it is not a word-find beyond it
        assert_eq!(t.text(), " baz");
        assert_eq!(t.caret(), 0);
    }
}
