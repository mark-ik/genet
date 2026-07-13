/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Host-agnostic text-editing helpers over [`TextInput`]: an undo/redo
//! [`EditHistory`] and an auto-pair [`wrap_selection`]. These need nothing but the
//! buffer, so any genet host gets undoable, bracket-wrapping fields for free — the
//! omnibar, a note editor, a chat compose box, a form field. Prose- or
//! grammar-specific editing (list continuation, structural selection over a djot
//! container tree) stays with the host that knows the grammar.

use crate::controls::TextInput;

/// The byte offset of char index `ci` in `text` (the buffer end when past the last
/// char), bridging [`TextInput`]'s char-index caret to its byte-offset selection setters.
fn byte_of_char(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(text.len())
}

/// Undo/redo history for a [`TextInput`]: a bounded stack of whole-buffer snapshots
/// (`TextInput` is `Clone`, so a snapshot captures text + caret + selection). A run of
/// consecutive character inserts coalesces into one entry — so a burst of typing undoes
/// as a unit — while a delete, a newline, or a caret move starts a fresh group.
///
/// The history lives *beside* the buffer, not wrapping it, so a host keeps its
/// `TextInput` field for rendering and drives undo through this companion:
///
/// ```ignore
/// // before a mutating edit:
/// history.snapshot(&field, /* coalesce_insert = */ true);
/// field.insert_str("x");
/// // on Ctrl+Z / Ctrl+Y:
/// history.undo(&mut field);
/// history.redo(&mut field);
/// ```
#[derive(Clone, Debug, Default)]
pub struct EditHistory {
    undo: Vec<TextInput>,
    redo: Vec<TextInput>,
    /// Whether the current run of character inserts is coalescing into one entry.
    coalescing: bool,
    /// Depth cap; the oldest entry is dropped past it. `0` means unbounded.
    cap: usize,
}

impl EditHistory {
    /// A fresh history with a sensible default depth cap (200 — a text field is short and
    /// each entry is a whole-buffer clone, so a bounded stack keeps memory flat).
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            coalescing: false,
            cap: 200,
        }
    }

    /// A history with an explicit depth `cap` (`0` = unbounded).
    pub fn with_cap(cap: usize) -> Self {
        Self { cap, ..Self::new() }
    }

    /// Record `input`'s pre-edit state, to call *before* a mutating edit. `coalesce_insert`
    /// is true for a character/space insert: a run of them coalesces into one entry (the
    /// push is skipped while already coalescing). A non-insert edit (delete, newline) passes
    /// `false`, so it is its own step and ends the run. Any push clears the redo stack.
    pub fn snapshot(&mut self, input: &TextInput, coalesce_insert: bool) {
        if coalesce_insert && self.coalescing {
            return;
        }
        self.undo.push(input.clone());
        if self.cap != 0 && self.undo.len() > self.cap {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.coalescing = coalesce_insert;
    }

    /// End the current insert-coalescing run without snapshotting — for a caret move, so
    /// the next insert starts a fresh group even though nothing was deleted.
    pub fn break_coalesce(&mut self) {
        self.coalescing = false;
    }

    /// Undo the last edit: restore the top undo snapshot into `input`, moving the current
    /// buffer onto the redo stack. Returns whether anything was undone.
    pub fn undo(&mut self, input: &mut TextInput) -> bool {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(input, prev));
                self.coalescing = false;
                true
            }
            None => false,
        }
    }

    /// Redo the last undone edit: restore the top redo snapshot into `input`, moving the
    /// current buffer back onto the undo stack. Returns whether anything was redone.
    pub fn redo(&mut self, input: &mut TextInput) -> bool {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(input, next));
                self.coalescing = false;
                true
            }
            None => false,
        }
    }

    /// Drop all history (on a field reset, so a fresh document never undoes into a prior one).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.coalescing = false;
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// The closing delimiter that pairs with `open` for an auto-pair wrap, or `None` if `open`
/// is not a wrapping delimiter. Covers brackets, quotes, and the common markup emphasis /
/// code delimiters.
pub fn pair_close(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        '"' => '"',
        '\'' => '\'',
        '`' => '`',
        '*' => '*',
        '_' => '_',
        '~' => '~',
        _ => return None,
    })
}

/// Auto-pair: wrap the current selection with the delimiter pair `open`…`close` and keep the
/// inner text selected, so wraps nest (typing `*` then `_` over a word gives `*_word_*`).
/// Returns whether it wrapped — `false` (no selection, or `open` is not a pair delimiter)
/// means the caller should insert `open` normally. Snapshot for undo before calling.
pub fn wrap_selection(input: &mut TextInput, open: char) -> bool {
    let Some(close) = pair_close(open) else {
        return false;
    };
    if !input.has_selection() {
        return false;
    }
    let (lo, hi) = input.selection();
    let inner: String = input.text().chars().skip(lo).take(hi - lo).collect();
    input.insert_str(&format!("{open}{inner}{close}"));
    // Re-select the inner text (between the new delimiters), so a repeat wrap nests.
    let text = input.text().to_string();
    let start = byte_of_char(&text, lo + 1);
    let end = byte_of_char(&text, lo + 1 + (hi - lo));
    input.set_caret_byte(start, false);
    input.set_caret_byte(end, true);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, caret_byte: usize) -> TextInput {
        let mut t = TextInput::new(text);
        t.set_caret_byte(caret_byte, false);
        t
    }

    #[test]
    fn undo_redo_round_trips() {
        let mut input = TextInput::new("");
        let mut h = EditHistory::new();
        h.snapshot(&input, false);
        input.insert_str("hello");
        assert!(h.can_undo());
        assert!(h.undo(&mut input));
        assert_eq!(input.text(), "");
        assert!(h.redo(&mut input));
        assert_eq!(input.text(), "hello");
    }

    #[test]
    fn inserts_coalesce_into_one_undo() {
        let mut input = TextInput::new("");
        let mut h = EditHistory::new();
        for ch in ["h", "i"] {
            h.snapshot(&input, true); // coalescing run
            input.insert_str(ch);
        }
        // One undo removes the whole run.
        assert!(h.undo(&mut input));
        assert_eq!(input.text(), "");
        assert!(!h.can_undo());
    }

    #[test]
    fn a_new_edit_clears_redo() {
        let mut input = TextInput::new("a");
        let mut h = EditHistory::new();
        h.snapshot(&input, false);
        input.insert_str("b");
        h.undo(&mut input); // back to "a", redo has "ab"
        assert!(h.can_redo());
        h.snapshot(&input, false);
        input.insert_str("c"); // a fresh edit
        assert!(!h.can_redo(), "a new edit drops the redo stack");
    }

    #[test]
    fn wrap_selection_wraps_and_nests() {
        let mut input = at("hello world", 0);
        input.set_caret_byte(0, false);
        input.set_caret_byte(5, true); // select "hello"
        assert!(wrap_selection(&mut input, '*'));
        assert_eq!(input.text(), "*hello* world");
        assert!(wrap_selection(&mut input, '_')); // inner still selected → nests
        assert_eq!(input.text(), "*_hello_* world");
    }

    #[test]
    fn wrap_without_selection_or_pair_is_declined() {
        let mut input = at("hello", 5);
        assert!(!wrap_selection(&mut input, '*'), "no selection");
        input.set_caret_byte(0, false);
        input.set_caret_byte(5, true);
        assert!(!wrap_selection(&mut input, 'x'), "not a pair delimiter");
        assert_eq!(input.text(), "hello");
    }
}
