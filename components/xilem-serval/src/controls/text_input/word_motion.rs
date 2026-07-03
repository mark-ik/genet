/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Word motion (Ctrl/Alt + ←/→, Backspace, Delete).
//!
//! Word boundaries are UAX#29 (`unicode-segmentation`), computed over the buffer
//! here — the buffer owns segmentation; the host only routes the modified key. A
//! "word" is any non-whitespace segment, so motion stops at the edges of words and
//! of punctuation runs (e.g. djot `**`, backticks), skipping the whitespace between.

use unicode_segmentation::UnicodeSegmentation;

use super::TextInput;

impl TextInput {
    /// The char index at byte offset `byte` (clamped) — inverse of
    /// [`byte_of`](super::core::TextInput::byte_of), mapping a word boundary back
    /// to the char model.
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
