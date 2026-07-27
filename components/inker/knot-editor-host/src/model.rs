// Copyright 2026 Mark AB (markik)
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The authority-free retained Knot editor model.

use cambium::{CaretSelection, TextCommand, TextInput};
use illume::{Fold, OutlineItem, Span};
#[cfg(feature = "preview")]
use inker::EngineDocument;

use crate::KnotReadout;

/// What changed when one platform-neutral command was applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditOutcome {
    pub state_changed: bool,
    pub source_changed: bool,
}

/// A retained Knot source buffer and its derived readout.
///
/// This type has no path, store, key, or writer. A host receives source from
/// an authority, edits locally, then returns the text through that authority's
/// save intent.
pub struct KnotEditor {
    #[cfg_attr(not(feature = "preview"), allow(dead_code))]
    address: String,
    original: String,
    input: TextInput,
    readout: KnotReadout,
}

impl KnotEditor {
    pub fn scratch(address: impl Into<String>, source: impl Into<String>) -> Self {
        let source = source.into();
        Self {
            address: address.into(),
            original: source.clone(),
            input: TextInput::new(source),
            readout: KnotReadout::new(),
        }
    }

    pub fn input(&self) -> &TextInput {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut TextInput {
        &mut self.input
    }

    pub fn source(&self) -> &str {
        self.input.text()
    }

    pub fn selection(&self) -> CaretSelection {
        self.input.caret_selection()
    }

    pub fn apply(&mut self, command: TextCommand) -> EditOutcome {
        let before = self.input.text().to_string();
        let state_changed = self.input.apply(command);
        EditOutcome {
            state_changed,
            source_changed: self.input.text() != before,
        }
    }

    pub fn apply_layout_selection(&mut self, selection: CaretSelection) -> EditOutcome {
        self.apply(TextCommand::SetSelection(selection))
    }

    pub fn highlights(&self) -> Vec<Span> {
        self.readout.highlights(self.input.text())
    }

    pub fn outline(&self) -> Vec<OutlineItem> {
        self.readout.outline(self.input.text())
    }

    pub fn folds(&self) -> Vec<Fold> {
        self.readout.folds(self.input.text())
    }

    #[cfg(feature = "preview")]
    pub fn preview(&self) -> Result<EngineDocument, String> {
        self.readout
            .rendered(&self.address, self.input.text())
            .map_err(|error| format!("could not render Knot preview: {error}"))
    }

    pub fn is_dirty(&self) -> bool {
        self.input.text() != self.original
    }

    /// Record the exact source an external authority accepted.
    ///
    /// The local buffer may already contain later edits; those remain dirty.
    pub fn accept_saved_source(&mut self, source: &str) {
        self.original = source.to_string();
    }
}

#[cfg(test)]
mod tests {
    use cambium::{CaretAffinity, CaretPosition};

    use super::*;

    #[test]
    fn one_buffer_drives_edits_and_every_readout() {
        let mut editor = KnotEditor::scratch("memory:note", "# One\n");
        assert_eq!(editor.outline().len(), 1);

        let outcome = editor.apply(TextCommand::Insert("\n## Two\n".into()));
        assert!(outcome.source_changed);
        assert_eq!(editor.outline().len(), 2);
        assert!(!editor.highlights().is_empty());
        #[cfg(feature = "preview")]
        assert!(!editor.preview().unwrap().blocks.is_empty());

        editor.apply(TextCommand::Undo);
        assert_eq!(editor.source(), "# One\n");
    }

    #[test]
    fn an_async_save_only_accepts_the_source_the_authority_saw() {
        let mut editor = KnotEditor::scratch("memory:note", "one");
        editor.apply(TextCommand::Insert(" two".into()));
        let submitted = editor.source().to_string();
        editor.apply(TextCommand::Insert(" three".into()));

        editor.accept_saved_source(&submitted);

        assert!(editor.is_dirty());
        assert_eq!(editor.source(), "one two three");
    }

    #[test]
    fn layout_selection_preserves_byte_affinity() {
        let mut editor = KnotEditor::scratch("memory:note", "abc");
        let selection = CaretSelection {
            anchor: CaretPosition {
                byte: 0,
                affinity: CaretAffinity::Downstream,
            },
            focus: CaretPosition {
                byte: 2,
                affinity: CaretAffinity::Upstream,
            },
        };
        editor.apply_layout_selection(selection);
        assert_eq!(editor.selection(), selection);
    }
}
