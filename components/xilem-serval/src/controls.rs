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
//! and moves it (←/→ and Home/End). The field renders the **clean** buffer; the
//! host paints the caret as a thin bar at the cursor via
//! `serval_layout::caret_rect` overlaid on the scene (see `pelt-live`'s render
//! path). [`TextInput::display`] — the buffer with a `|` at the caret — is a
//! *textual* representation for headless tests / debug, not what the field
//! renders on screen.

use crate::pod::ServalElement;
use crate::{
    El, Key, KeyEvent, NamedKey, OnClick, OnKey, OptionalAction, PointerClick, ServalCtx, View, el,
    on_click, on_key,
};

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
}

impl TextInput {
    /// A field holding `text`, with the caret collapsed at the end.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.chars().count();
        Self { text, caret, anchor: caret }
    }

    /// The buffer, without the caret marker.
    pub fn text(&self) -> &str {
        &self.text
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
        self.delete_selection();
        let at = self.byte_of(self.caret);
        self.text.insert_str(at, s);
        self.caret += s.chars().count();
        self.anchor = self.caret;
    }

    /// Backspace: delete the selection if any, else the character before the
    /// caret. No-op at the start of an unselected buffer.
    pub fn backspace(&mut self) {
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
        self.caret = 0;
        if !extend {
            self.anchor = 0;
        }
    }

    /// Move the caret to the end (End). `extend` keeps the anchor (selecting to
    /// the end).
    pub fn end(&mut self, extend: bool) {
        self.caret = self.char_count();
        if !extend {
            self.anchor = self.caret;
        }
    }

    // --- multi-line navigation (textarea) -------------------------------------
    //
    // Lines are `\n`-delimited in the buffer (serval feeds the raw text to
    // parley, which breaks at `\n`); a column is the char offset within a line.
    // No sticky goal column: up/down recompute the column each step (Tier 1).

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

    /// Move the caret up one line, keeping the column (ArrowUp). At the first
    /// line it goes to the buffer start. `extend` grows the selection.
    pub fn move_up(&mut self, extend: bool) {
        let (line, col) = self.line_col();
        self.caret = if line == 0 { 0 } else { self.offset_at(line - 1, col) };
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret down one line, keeping the column (ArrowDown). At the last
    /// line it goes to the buffer end. `extend` grows the selection.
    pub fn move_down(&mut self, extend: bool) {
        let (line, col) = self.line_col();
        let last = self.line_starts().len() - 1;
        self.caret = if line == last { self.char_count() } else { self.offset_at(line + 1, col) };
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret to the start of its line (Home, multi-line). `extend`
    /// grows the selection.
    pub fn home_line(&mut self, extend: bool) {
        let (line, _) = self.line_col();
        self.caret = self.offset_at(line, 0);
        if !extend {
            self.anchor = self.caret;
        }
    }

    /// Move the caret to the end of its line (End, multi-line). `extend` grows
    /// the selection.
    pub fn end_line(&mut self, extend: bool) {
        let (line, _) = self.line_col();
        self.caret = self.offset_at(line, usize::MAX);
        if !extend {
            self.anchor = self.caret;
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
/// * [`NamedKey::Backspace`] / [`NamedKey::Delete`] remove the selection if any,
///   else the char before / after the caret. [`NamedKey::ArrowLeft`] /
///   [`NamedKey::ArrowRight`] move one char and [`NamedKey::Home`] /
///   [`NamedKey::End`] jump to the line ends — and with **Shift held**
///   (`ev.mods.shift`) they *extend the selection* instead of collapsing it.
/// * Any `Key::Character` / `Space` insert replaces a non-empty selection first.
/// * [`NamedKey::Enter`], `Tab`, `Escape`, ↑/↓, and `Other` have no effect in a
///   single-line field yet (multi-line / commit are later slices).
fn edit(input: &mut TextInput, ev: KeyEvent) {
    let extend = ev.mods.shift;
    match ev.key {
        Key::Character(s) => input.insert_str(&s),
        Key::Named(NamedKey::Space) => input.insert_str(" "),
        Key::Named(NamedKey::Backspace) => input.backspace(),
        Key::Named(NamedKey::Delete) => input.delete(),
        Key::Named(NamedKey::ArrowLeft) => input.move_left(extend),
        Key::Named(NamedKey::ArrowRight) => input.move_right(extend),
        Key::Named(NamedKey::Home) => input.home(extend),
        Key::Named(NamedKey::End) => input.end(extend),
        Key::Named(_) => {},
    }
}

/// The edit handler for [`textarea`]: like [`edit`] but multi-line. `Enter`
/// inserts a newline; `ArrowUp` / `ArrowDown` move between lines; `Home` / `End`
/// scope to the current line (`home_line` / `end_line`). Everything else
/// (typing, Backspace/Delete, ←/→, Shift to extend) matches the single-line
/// field.
fn edit_multiline(input: &mut TextInput, ev: KeyEvent) {
    let extend = ev.mods.shift;
    match ev.key {
        Key::Character(s) => input.insert_str(&s),
        Key::Named(NamedKey::Space) => input.insert_str(" "),
        Key::Named(NamedKey::Enter) => input.insert_str("\n"),
        Key::Named(NamedKey::Backspace) => input.backspace(),
        Key::Named(NamedKey::Delete) => input.delete(),
        Key::Named(NamedKey::ArrowLeft) => input.move_left(extend),
        Key::Named(NamedKey::ArrowRight) => input.move_right(extend),
        Key::Named(NamedKey::ArrowUp) => input.move_up(extend),
        Key::Named(NamedKey::ArrowDown) => input.move_down(extend),
        Key::Named(NamedKey::Home) => input.home_line(extend),
        Key::Named(NamedKey::End) => input.end_line(extend),
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
    // Render the clean buffer; the host paints the caret over it (see the module
    // docs). `display()` (with the `|` marker) is the textual representation, not
    // what the field shows on screen.
    on_key(el::<_, TextInput, ()>("input", input.text().to_string()), handler)
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

// --- MARK: textarea ----------------------------------------------------------

/// Build the concrete view for a multi-line [`textarea`]. Structurally identical
/// to a [`TextField`] (an `on_key`-wrapped element over a [`TextInput`]); the
/// difference is the [`edit_multiline`] handler and a `<textarea>` tag. With
/// `\n`s in the buffer, serval/parley break it into lines (serval feeds raw text
/// to parley, which honors `\n`).
fn build_textarea(input: &TextInput) -> TextField {
    let handler: fn(&mut TextInput, KeyEvent) = edit_multiline;
    on_key(el::<_, TextInput, ()>("textarea", input.text().to_string()), handler)
}

/// A reusable multi-line text field over a [`TextInput`] — [`text_field`]'s
/// multi-line sibling. `Enter` inserts a newline (which renders as a line break),
/// `ArrowUp` / `ArrowDown` move between lines, `Home` / `End` scope to the line.
/// Composable via [`lens`](crate::lens) like [`text_field`].
///
/// Tier 1: lines are `\n`-delimited in the buffer; up/down navigate those hard
/// lines (no soft-wrap visual-line navigation, which would need the layout).
pub fn textarea(input: &TextInput) -> impl View<TextInput, (), ServalCtx, Element = ServalElement> + use<> {
    build_textarea(input)
}

/// [`textarea`] with its concrete return type named (for a host storing the
/// runner in a struct field; see [`text_field_typed`]).
pub fn textarea_typed(input: &TextInput) -> TextField {
    build_textarea(input)
}

// --- MARK: checkbox / toggle -------------------------------------------------

/// The toggle handler for [`checkbox`] / [`toggle`]: flip the bool on click.
fn flip(checked: &mut bool, _: PointerClick) {
    *checked = !*checked;
}

/// The concrete view type a checkbox / toggle produces: an `on_click`-wrapped
/// element reflecting the checked state.
pub type Checkbox = OnClick<El<&'static str, bool, ()>, bool, (), fn(&mut bool, PointerClick)>;

/// Build a checkbox-style control with the given `kind` class (`"checkbox"` /
/// `"toggle"`), reflecting `checked` as a textual indicator, an ARIA state, and
/// a `checked` class for styling.
fn build_check(kind: &'static str, checked: bool) -> Checkbox {
    // ASCII indicator (reliably renders without special fonts); the host styles
    // the `kind` / `checked` classes for the real look.
    let indicator = if checked { "[x]" } else { "[ ]" };
    let class = if checked { kind_checked(kind) } else { kind };
    let aria = if checked { "true" } else { "false" };
    let handler: fn(&mut bool, PointerClick) = flip;
    on_click(
        el::<_, bool, ()>("span", indicator)
            .attr("role", "checkbox")
            .attr("aria-checked", aria)
            .attr("class", class),
        handler,
    )
}

/// `"checkbox checked"` / `"toggle checked"` — the class string for a checked
/// control of the given `kind`.
fn kind_checked(kind: &'static str) -> &'static str {
    match kind {
        "toggle" => "toggle checked",
        _ => "checkbox checked",
    }
}

/// A reusable checkbox whose state *is* a `bool`. Clicking it toggles the bool;
/// it reflects the state as `role="checkbox"` + `aria-checked` (for the a11y
/// tree) and a `checkbox` / `checkbox checked` class (for host styling), with an
/// ASCII `[x]` / `[ ]` fallback indicator. Composes onto an app's bool field via
/// [`lens`](crate::lens), like [`text_field`] onto a `TextInput`.
pub fn checkbox(checked: bool) -> impl View<bool, (), ServalCtx, Element = ServalElement> + use<> {
    build_check("checkbox", checked)
}

/// [`checkbox`] with its concrete return type named (for a host storing the
/// runner in a struct field; see [`text_field_typed`]).
pub fn checkbox_typed(checked: bool) -> Checkbox {
    build_check("checkbox", checked)
}

/// A toggle switch — behaviourally a [`checkbox`] (a `bool`, flipped on click),
/// distinguished only by a `toggle` class so the host styles it as a switch.
pub fn toggle(checked: bool) -> impl View<bool, (), ServalCtx, Element = ServalElement> + use<> {
    build_check("toggle", checked)
}

// --- MARK: button ------------------------------------------------------------

/// A `<button>` view: `label` text plus an `on_click` handler — the ergonomic
/// form of `on_click(el("button", label), handler)`. The handler may return an
/// action (it is an [`OptionalAction`]) exactly as [`on_click`].
pub fn button<State, Action, OA, F>(
    label: impl Into<String>,
    handler: F,
) -> OnClick<El<String, State, Action>, State, Action, F>
where
    State: 'static,
    Action: 'static,
    OA: OptionalAction<Action>,
    F: Fn(&mut State, PointerClick) -> OA + 'static,
{
    on_click(el::<_, State, Action>("button", label.into()), handler)
}
