/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Reusable form controls built on the [`on_key`](crate::on_key) foundation.
//!
//! Stage 3 of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md` (the
//! text-field slice, deepened with a caret): the first *control* on top of the
//! keyboard/focus foundation. [`text_field`] is the editable-text analogue of
//! the Stage 3a `counter_button` component — its state is a [`TextInput`]
//! (buffer + caret), so it composes onto a larger app's field through
//! [`lens`](crate::lens), exactly as `counter_button` composes onto a `u32`.
//!
//! There is no browser `<input>` machinery here: serval lays out a plain
//! element whose text content is the buffer, and [`on_key`](crate::on_key)
//! makes that element focusable and routes typed keys to an edit handler that
//! mutates the [`TextInput`]. The host (`pelt-live`) maps winit key events to
//! the native [`KeyEvent`](crate::KeyEvent); the runner's focus + dispatch
//! ([`dispatch_key`](crate::ServalAppRunner::dispatch_key)) deliver them here.
//!
//! ## Caret
//!
//! The field is a real insertion-point editor, not append-only: [`TextInput`]
//! carries a `caret` (a *character* index, so editing is Unicode-correct), and
//! the handler inserts at the caret, deletes before/after it (Backspace/Delete),
//! and moves it (←/→). Rendering shows the caret as a `|` marker
//! ([`TextInput::display`]) — a placeholder visible cursor; real caret painting
//! (a measured glyph-position rect, blinking) is a later slice. The marker is
//! render-only; [`TextInput::text`] stays the clean buffer.

use crate::pod::ServalElement;
use crate::{El, Key, KeyEvent, NamedKey, OnKey, ServalCtx, View, el, on_key};

/// The placeholder caret marker inserted into the *rendered* string (never into
/// the buffer). A stand-in for real caret painting.
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
    caret: usize,
}

impl TextInput {
    /// A field holding `text`, with the caret at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.chars().count();
        Self { text, caret }
    }

    /// The buffer, without the caret marker.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The caret position: a character index in `0..=char_count`.
    pub fn caret(&self) -> usize {
        self.caret
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

    /// Insert `s` at the caret, advancing the caret past it.
    pub fn insert_str(&mut self, s: &str) {
        let at = self.byte_of(self.caret);
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
    }

    /// Delete the character *before* the caret and step left (Backspace). No-op
    /// at the start of the buffer.
    pub fn backspace(&mut self) {
        if self.caret == 0 {
            return;
        }
        let start = self.byte_of(self.caret - 1);
        let end = self.byte_of(self.caret);
        self.text.replace_range(start..end, "");
        self.caret -= 1;
    }

    /// Delete the character *after* the caret, leaving the caret put (Delete).
    /// No-op at the end of the buffer.
    pub fn delete(&mut self) {
        if self.caret >= self.char_count() {
            return;
        }
        let start = self.byte_of(self.caret);
        let end = self.byte_of(self.caret + 1);
        self.text.replace_range(start..end, "");
    }

    /// Move the caret one character left (clamped at the start).
    pub fn move_left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    /// Move the caret one character right (clamped at the end).
    pub fn move_right(&mut self) {
        if self.caret < self.char_count() {
            self.caret += 1;
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

/// The edit handler for [`text_field`]: apply one [`KeyEvent`] to the
/// [`TextInput`].
///
/// A free function (not a closure) so [`text_field`]'s return type names a `fn`
/// pointer rather than an unnameable closure — the same reason the test views
/// use `fn`-pointer handlers. The editing model:
///
/// * [`Key::Character`] inserts the produced text at the caret (so `"h"`, `"H"`,
///   `"é"`, and multi-character IME input all insert verbatim).
/// * [`NamedKey::Space`] inserts a literal space — per Stage 3b, the space bar
///   arrives as [`NamedKey::Space`], *not* `Character(" ")`, so the field handles
///   it explicitly.
/// * [`NamedKey::Backspace`] / [`NamedKey::Delete`] remove the char before / after
///   the caret; [`NamedKey::ArrowLeft`] / [`NamedKey::ArrowRight`] move it.
/// * [`NamedKey::Enter`], `Tab`, `Escape`, ↑/↓, and `Other` have no effect in a
///   single-line field yet (multi-line / commit / Home-End are later slices).
fn edit(input: &mut TextInput, ev: KeyEvent) {
    match ev.key {
        Key::Character(s) => input.insert_str(&s),
        Key::Named(NamedKey::Space) => input.insert_str(" "),
        Key::Named(NamedKey::Backspace) => input.backspace(),
        Key::Named(NamedKey::Delete) => input.delete(),
        Key::Named(NamedKey::ArrowLeft) => input.move_left(),
        Key::Named(NamedKey::ArrowRight) => input.move_right(),
        Key::Named(_) => {},
    }
}

/// The concrete view type the field produces.
///
/// An [`on_key`](crate::on_key)-wrapped `<input>` whose text content is the
/// field's rendered buffer ([`TextInput::display`]). [`text_field`] returns this
/// *behind* an `impl View` (the reusable-component shape), while
/// [`text_field_typed`] returns it *named* — for a host that must spell its
/// concrete view type (e.g. a runner stored in a struct field, which needs a
/// nameable `V`).
pub type TextField = OnKey<El<String, TextInput, ()>, TextInput, (), fn(&mut TextInput, KeyEvent)>;

/// Build the concrete [`TextField`] for `input` (the shared implementation
/// behind both [`text_field`] and [`text_field_typed`]).
fn build_text_field(input: &TextInput) -> TextField {
    let handler: fn(&mut TextInput, KeyEvent) = edit;
    on_key(el::<_, TextInput, ()>("input", input.display()), handler)
}

/// A reusable, editable text field whose state *is* a [`TextInput`].
///
/// Renders the field's [`display`](TextInput::display) (buffer + caret marker) as
/// the text content of an `<input>` element wrapped in [`on_key`](crate::on_key);
/// the `on_key` makes the element focusable and routes typed keys to [`edit`],
/// which mutates the `&mut TextInput`. Knowing nothing but its own
/// [`TextInput`], the field composes onto any larger app state through
/// [`lens`](crate::lens) — `lens(|s| text_field(s), |app| &mut app.name)` — just
/// as the Stage 3a `counter_button` composes onto a `u32`.
///
/// `+ use<>` keeps the opaque return type from capturing the `input` borrow's
/// lifetime, so it is a single `V` usable as a `FnMut(&_) -> V` app logic. A host
/// that needs the *named* concrete type (to store the runner's `V`) uses
/// [`text_field_typed`] instead.
///
/// The element is an `<input>` so author CSS can target the field (e.g. a
/// border/background) and so it reads as a control; serval lays it out as
/// whatever the cascade resolves. It carries no browser `<input>` value
/// semantics — its text is just its content, diffed like any other text on
/// rebuild.
pub fn text_field(input: &TextInput) -> impl View<TextInput, (), ServalCtx, Element = ServalElement> + use<> {
    build_text_field(input)
}

/// [`text_field`] with its concrete return type named.
///
/// Identical behaviour to [`text_field`]; the only difference is the signature
/// returns the named [`TextField`] rather than `impl View`. A host that stores
/// its runner in a struct field needs a nameable view type `V`, so it composes
/// `lens(text_field_typed, …)` (whose `Lens<…>` type can then be spelled) rather
/// than `lens(|s| text_field(s), …)` (whose inner view is opaque).
pub fn text_field_typed(input: &TextInput) -> TextField {
    build_text_field(input)
}
