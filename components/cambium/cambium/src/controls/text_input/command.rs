/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Platform-neutral text commands and the one mutation path that applies them.
//!
//! DOM fields translate focused [`KeyEvent`]s into these commands. A host with
//! its own action spine can lower the same command as application intent and
//! call [`TextInput::apply`] later, without creating a second editor.

use crate::{CompositionEvent, Key, KeyEvent, NamedKey};

use super::{CaretSelection, TextInput};

/// Whether a field accepts line breaks and vertical navigation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFieldMode {
    #[default]
    SingleLine,
    Multiline,
}

/// Logical caret movement that needs no layout. A layout-aware host can instead
/// compute a [`CaretSelection`] through Parley and send
/// [`TextCommand::SetSelection`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    GraphemeLeft,
    GraphemeRight,
    WordLeft,
    WordRight,
    LineUp,
    LineDown,
    LineHome,
    LineEnd,
    BufferHome,
    BufferEnd,
}

/// One platform-neutral editing intent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextCommand {
    Insert(String),
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    Move {
        movement: CaretMove,
        extend: bool,
    },
    SelectAll,
    SetSelection(CaretSelection),
    SetComposition {
        text: String,
        selection: Option<(usize, usize)>,
    },
    CommitComposition(String),
    CancelComposition,
    AcceptGhost,
    Undo,
    Redo,
}

/// Translate a focused Cambium event into an editing command.
///
/// Primary-modifier shortcuts not owned by the editor return `None`, so
/// Ctrl/Cmd+C/V/X can be handled by the host clipboard instead of inserting
/// literal `c`, `v`, or `x` into the field.
pub fn text_command_from_key(event: &KeyEvent, mode: TextFieldMode) -> Option<TextCommand> {
    let extend = event.mods.shift;
    let primary = event.mods.ctrl || event.mods.meta;
    let word = event.mods.ctrl || event.mods.alt;
    match &event.key {
        Key::Composition(CompositionEvent::Enabled) => None,
        Key::Composition(CompositionEvent::Preedit { text, selection }) => {
            Some(TextCommand::SetComposition {
                text: text.clone(),
                selection: *selection,
            })
        },
        Key::Composition(CompositionEvent::Commit(text)) => {
            Some(TextCommand::CommitComposition(text.clone()))
        },
        Key::Composition(CompositionEvent::Disabled) => Some(TextCommand::CancelComposition),
        Key::Character(value) if primary && value.eq_ignore_ascii_case("a") => {
            Some(TextCommand::SelectAll)
        },
        Key::Character(value) if primary && value.eq_ignore_ascii_case("z") && event.mods.shift => {
            Some(TextCommand::Redo)
        },
        Key::Character(value) if primary && value.eq_ignore_ascii_case("z") => {
            Some(TextCommand::Undo)
        },
        Key::Character(value) if primary && value.eq_ignore_ascii_case("y") => {
            Some(TextCommand::Redo)
        },
        Key::Character(_) if primary || event.mods.alt => None,
        Key::Character(value) => Some(TextCommand::Insert(value.clone())),
        Key::Named(NamedKey::Space) => Some(TextCommand::Insert(" ".to_owned())),
        Key::Named(NamedKey::Enter) if mode == TextFieldMode::Multiline => {
            Some(TextCommand::Insert("\n".to_owned()))
        },
        Key::Named(NamedKey::Backspace) if word => Some(TextCommand::DeleteWordLeft),
        Key::Named(NamedKey::Backspace) => Some(TextCommand::Backspace),
        Key::Named(NamedKey::Delete) if word => Some(TextCommand::DeleteWordRight),
        Key::Named(NamedKey::Delete) => Some(TextCommand::Delete),
        Key::Named(NamedKey::ArrowLeft) if word => Some(TextCommand::Move {
            movement: CaretMove::WordLeft,
            extend,
        }),
        Key::Named(NamedKey::ArrowLeft) => Some(TextCommand::Move {
            movement: CaretMove::GraphemeLeft,
            extend,
        }),
        Key::Named(NamedKey::ArrowRight) if word => Some(TextCommand::Move {
            movement: CaretMove::WordRight,
            extend,
        }),
        Key::Named(NamedKey::ArrowRight) => Some(TextCommand::Move {
            movement: CaretMove::GraphemeRight,
            extend,
        }),
        Key::Named(NamedKey::ArrowUp) if mode == TextFieldMode::Multiline => {
            Some(TextCommand::Move {
                movement: CaretMove::LineUp,
                extend,
            })
        },
        Key::Named(NamedKey::ArrowDown) if mode == TextFieldMode::Multiline => {
            Some(TextCommand::Move {
                movement: CaretMove::LineDown,
                extend,
            })
        },
        Key::Named(NamedKey::Home) if mode == TextFieldMode::Multiline && primary => {
            Some(TextCommand::Move {
                movement: CaretMove::BufferHome,
                extend,
            })
        },
        Key::Named(NamedKey::Home) if mode == TextFieldMode::Multiline => Some(TextCommand::Move {
            movement: CaretMove::LineHome,
            extend,
        }),
        Key::Named(NamedKey::Home) => Some(TextCommand::Move {
            movement: CaretMove::BufferHome,
            extend,
        }),
        Key::Named(NamedKey::End) if mode == TextFieldMode::Multiline && primary => {
            Some(TextCommand::Move {
                movement: CaretMove::BufferEnd,
                extend,
            })
        },
        Key::Named(NamedKey::End) if mode == TextFieldMode::Multiline => Some(TextCommand::Move {
            movement: CaretMove::LineEnd,
            extend,
        }),
        Key::Named(NamedKey::End) => Some(TextCommand::Move {
            movement: CaretMove::BufferEnd,
            extend,
        }),
        Key::Named(_) => None,
    }
}

impl TextInput {
    /// Apply one editing command. Returns whether logical or composition state
    /// changed. Mutating commands enter the built-in bounded undo journal;
    /// motion breaks typing coalescence without creating an undo step.
    pub fn apply(&mut self, command: TextCommand) -> bool {
        match command {
            TextCommand::Undo => {
                let current = self.snapshot();
                let Some(previous) = self.history.undo_snapshot(current) else {
                    return false;
                };
                self.restore(previous);
                true
            },
            TextCommand::Redo => {
                let current = self.snapshot();
                let Some(next) = self.history.redo_snapshot(current) else {
                    return false;
                };
                self.restore(next);
                true
            },
            TextCommand::SetComposition { text, selection } => {
                let before = self.composition.clone();
                self.set_composition(text, selection);
                self.composition != before
            },
            TextCommand::CancelComposition => {
                let changed = self.composition.is_some();
                self.clear_preedit();
                changed
            },
            TextCommand::Move { movement, extend } => {
                let before = self.caret_selection();
                self.clear_preedit();
                self.history.break_coalesce();
                match movement {
                    CaretMove::GraphemeLeft => self.move_left(extend),
                    CaretMove::GraphemeRight => self.move_right(extend),
                    CaretMove::WordLeft => self.move_word_left(extend),
                    CaretMove::WordRight => self.move_word_right(extend),
                    CaretMove::LineUp => self.move_up(extend),
                    CaretMove::LineDown => self.move_down(extend),
                    CaretMove::LineHome => self.home_line(extend),
                    CaretMove::LineEnd => self.end_line(extend),
                    CaretMove::BufferHome => self.home(extend),
                    CaretMove::BufferEnd => self.end(extend),
                }
                self.caret_selection() != before
            },
            TextCommand::SelectAll => {
                let before = self.caret_selection();
                self.clear_preedit();
                self.history.break_coalesce();
                self.select_all();
                self.caret_selection() != before
            },
            TextCommand::SetSelection(selection) => {
                let before = self.caret_selection();
                self.clear_preedit();
                self.history.break_coalesce();
                self.set_caret_selection(selection);
                self.caret_selection() != before
            },
            TextCommand::CommitComposition(text) => {
                self.apply_committed_edit(false, move |input| {
                    input.clear_preedit();
                    input.insert_str(&text);
                })
            },
            TextCommand::Insert(text) => {
                let coalesce = !self.has_selection() && !text.contains('\n');
                self.apply_committed_edit(coalesce, move |input| {
                    input.clear_preedit();
                    input.insert_str(&text);
                })
            },
            TextCommand::Backspace => self.apply_committed_edit(false, |input| input.backspace()),
            TextCommand::Delete => self.apply_committed_edit(false, |input| input.delete()),
            TextCommand::DeleteWordLeft => {
                self.apply_committed_edit(false, |input| input.delete_word_left())
            },
            TextCommand::DeleteWordRight => {
                self.apply_committed_edit(false, |input| input.delete_word_right())
            },
            TextCommand::AcceptGhost => {
                self.apply_committed_edit(false, |input| input.accept_ghost())
            },
        }
    }

    /// Apply a focused key/IME event using the shared command mapping.
    pub fn apply_key(&mut self, event: &KeyEvent, mode: TextFieldMode) -> bool {
        text_command_from_key(event, mode).is_some_and(|command| self.apply(command))
    }

    fn apply_committed_edit(
        &mut self,
        coalesce_insert: bool,
        edit: impl FnOnce(&mut TextInput),
    ) -> bool {
        let before = self.snapshot();
        let composition_before = self.composition.clone();
        let ghost_before = self.ghost.clone();
        edit(self);
        let committed_changed = self.snapshot() != before;
        if committed_changed {
            self.history.record_snapshot(before, coalesce_insert);
        } else {
            self.history.break_coalesce();
        }
        committed_changed
            || self.composition != composition_before
            || self.ghost != ghost_before
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompositionEvent, Key, KeyEvent, Modifiers};

    use super::*;

    #[test]
    fn primary_shortcuts_do_not_insert_literal_characters() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let event = KeyEvent::with_mods(Key::Character("v".to_owned()), ctrl);
        assert_eq!(
            text_command_from_key(&event, TextFieldMode::SingleLine),
            None
        );
    }

    #[test]
    fn composition_round_trips_through_the_focused_event_shape() {
        let mut input = TextInput::new("ab");
        input.set_caret_byte(1, false);
        input.apply_key(
            &KeyEvent::new(Key::Composition(CompositionEvent::Preedit {
                text: "漢".to_owned(),
                selection: Some((3, 3)),
            })),
            TextFieldMode::SingleLine,
        );
        assert_eq!(input.render_text(), "a漢b");
        assert_eq!(input.caret_byte_in_render(), 4);

        input.apply_key(
            &KeyEvent::new(Key::Composition(CompositionEvent::Commit("漢".to_owned()))),
            TextFieldMode::SingleLine,
        );
        assert_eq!(input.text(), "a漢b");
        assert!(input.composition().is_none());
        assert!(input.apply(TextCommand::Undo));
        assert_eq!(input.text(), "ab");
    }

    #[test]
    fn composition_replaces_the_active_selection_in_render_and_commit() {
        let mut input = TextInput::new("abcd");
        input.set_caret_byte(1, false);
        input.set_caret_byte(3, true);
        input.apply(TextCommand::SetComposition {
            text: "漢".to_owned(),
            selection: Some((0, 0)),
        });
        assert_eq!(
            input.render_parts(),
            ("a".to_owned(), "漢".to_owned(), "d".to_owned())
        );
        assert_eq!(input.render_text(), "a漢d");
        assert_eq!(input.caret_byte_in_render(), 1);

        input.apply(TextCommand::CommitComposition("漢".to_owned()));
        assert_eq!(input.text(), "a漢d");
        assert!(input.apply(TextCommand::Undo));
        assert_eq!(input.text(), "abcd");
        assert_eq!(input.selected_text(), "bc");
    }

    #[test]
    fn transient_composition_clears_without_creating_an_undo_step() {
        let mut input = TextInput::new("base");
        input.apply(TextCommand::SetComposition {
            text: "候補".to_owned(),
            selection: None,
        });

        assert!(input.apply(TextCommand::CommitComposition(String::new())));
        assert!(input.composition().is_none());
        assert!(!input.can_undo());

        input.apply(TextCommand::Insert("!".to_owned()));
        input.apply(TextCommand::SetComposition {
            text: "x".to_owned(),
            selection: None,
        });
        assert!(input.apply(TextCommand::Undo));
        assert_eq!(input.text(), "base");
        assert!(input.composition().is_none());
    }

    #[test]
    fn default_field_history_coalesces_typing_and_separates_deletes() {
        let mut input = TextInput::default();
        input.apply(TextCommand::Insert("h".to_owned()));
        input.apply(TextCommand::Insert("i".to_owned()));
        input.apply(TextCommand::Backspace);
        assert_eq!(input.text(), "h");
        assert!(input.apply(TextCommand::Undo));
        assert_eq!(input.text(), "hi");
        assert!(input.apply(TextCommand::Undo));
        assert_eq!(input.text(), "");
    }

    #[test]
    fn grapheme_motion_and_deletion_never_split_combining_or_zwj_sequences() {
        let family = "👨‍👩‍👧‍👦";
        let mut input = TextInput::new(format!("e\u{301}{family}x"));
        assert_eq!(input.caret(), 3, "three user-perceived characters");

        input.apply(TextCommand::Move {
            movement: CaretMove::GraphemeLeft,
            extend: false,
        });
        assert_eq!(input.caret(), 2, "left crosses the x grapheme");
        input.apply(TextCommand::Backspace);
        assert_eq!(input.text(), "e\u{301}x", "the whole ZWJ family is deleted");
        input.apply(TextCommand::Backspace);
        assert_eq!(
            input.text(),
            "x",
            "base plus combining mark is one deletion"
        );
    }

    /// Exhaustive finite operation wall: every four-command sequence over
    /// insertion, deletion, and movement must undo back to byte-identical text.
    #[test]
    fn undo_restores_buffer_byte_exactly_for_edit_sequences() {
        fn command(index: usize) -> TextCommand {
            match index {
                0 => TextCommand::Insert("a".to_owned()),
                1 => TextCommand::Insert("é".to_owned()),
                2 => TextCommand::Insert("👩‍🚀".to_owned()),
                3 => TextCommand::Backspace,
                4 => TextCommand::Delete,
                5 => TextCommand::Move {
                    movement: CaretMove::GraphemeLeft,
                    extend: false,
                },
                _ => TextCommand::Move {
                    movement: CaretMove::GraphemeRight,
                    extend: true,
                },
            }
        }

        let original = "A👨‍👩‍👧‍👦e\u{301}Z";
        for a in 0..7 {
            for b in 0..7 {
                for c in 0..7 {
                    for d in 0..7 {
                        let mut input = TextInput::new(original);
                        for op in [a, b, c, d] {
                            input.apply(command(op));
                        }
                        while input.can_undo() {
                            assert!(input.apply(TextCommand::Undo));
                        }
                        assert_eq!(
                            input.text().as_bytes(),
                            original.as_bytes(),
                            "sequence [{a}, {b}, {c}, {d}]"
                        );
                    }
                }
            }
        }
    }
}
