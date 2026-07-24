/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Multi-line (`textarea`) navigation: hard `\n`-delimited lines, with a sticky
//! goal column across a run of vertical moves.
//!
//! Lines are `\n`-delimited in the buffer (Genet feeds the raw text to
//! parley, which breaks at `\n`); a column is the char offset within a line.
//! Up/down keep a sticky goal column ([`TextInput::goal_col`](super::core::TextInput))
//! across a run (Tier 2). These walk hard `\n` lines, not parley's soft-wrap visual
//! rows — the soft-wrap caret (`genet_layout::caret_byte_vertical`) is a separate,
//! layout-aware path a host can wire instead, where the goal would be an x-position
//! not a column.

use super::TextInput;

impl TextInput {
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
        let end = starts
            .get(line + 1)
            .map(|&s| s - 1)
            .unwrap_or(self.char_count());
        start.saturating_add(column).min(end)
    }

    /// Move the caret up one line, keeping the **goal column** (ArrowUp). At the
    /// first line it goes to the buffer start. `extend` grows the selection.
    ///
    /// The goal column ([`goal_col`](super::core::TextInput)) is seeded from the
    /// caret's real column at the start of a vertical run and reused (clamped per
    /// line) thereafter, so arrowing up/down through varying-length lines does not
    /// drift toward the short ones (Tier 2). It is *not* cleared here — only a
    /// horizontal move or edit clears it (via `reset_goal`).
    pub fn move_up(&mut self, extend: bool) {
        let (line, col) = self.line_col();
        let goal = self.goal_col.unwrap_or(col);
        self.goal_col = Some(goal);
        self.caret = if line == 0 {
            0
        } else {
            self.offset_at(line - 1, goal)
        };
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
        self.caret = if line == last {
            self.char_count()
        } else {
            self.offset_at(line + 1, goal)
        };
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
}
