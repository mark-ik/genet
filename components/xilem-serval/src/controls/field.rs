/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The [`text_field`] / [`textarea`] views: an [`on_key`](crate::on_key)-wrapped
//! element over a [`TextInput`], plus the [`edit`] / [`edit_multiline`] handlers
//! that apply a [`KeyEvent`] to it.
//!
//! There is no browser `<input>` machinery here: serval lays out a plain
//! element whose text content is the buffer, and [`on_key`](crate::on_key)
//! makes that element focusable and routes typed keys to an edit handler that
//! mutates the [`TextInput`]. The host (`pelt-live`) maps winit key events to
//! the native [`KeyEvent`]; the runner's focus + dispatch
//! ([`dispatch_key`](crate::ServalAppRunner::dispatch_key)) deliver them here.

use super::text_input::TextInput;
use crate::pod::ServalElement;
use crate::{El, Key, KeyEvent, NamedKey, OnKey, ServalCtx, View, el, on_key};

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
///   [`NamedKey::End`] jump to the buffer ends — and with **Shift held**
///   (`ev.mods.shift`) they *extend the selection* instead of collapsing it.
/// * With **Ctrl** (or **Alt/Option** on macOS) held, ←/→ move by word and
///   Backspace/Delete delete a word (UAX#29 boundaries, via
///   [`move_word_left`](TextInput::move_word_left) /
///   [`delete_word_left`](TextInput::delete_word_left) and their right twins).
/// * Any `Key::Character` / `Space` insert replaces a non-empty selection first.
/// * [`NamedKey::Enter`], `Tab`, `Escape`, ↑/↓, and `Other` have no effect in a
///   single-line field (multi-line / commit are the [`textarea`] / host's job).
pub(crate) fn edit(input: &mut TextInput, ev: KeyEvent) {
    let extend = ev.mods.shift;
    // Word motion on Ctrl (Win/Linux) or Alt/Option (macOS); `select_all` already
    // takes Ctrl/Cmd. The `if word` arms sit before the plain ones so they win.
    let word = ev.mods.ctrl || ev.mods.alt;
    match ev.key {
        Key::Character(ref s) if (ev.mods.ctrl || ev.mods.meta) && s.eq_ignore_ascii_case("a") => {
            input.select_all()
        },
        Key::Character(s) => input.insert_str(&s),
        Key::Named(NamedKey::Space) => input.insert_str(" "),
        Key::Named(NamedKey::Backspace) if word => input.delete_word_left(),
        Key::Named(NamedKey::Backspace) => input.backspace(),
        Key::Named(NamedKey::Delete) if word => input.delete_word_right(),
        Key::Named(NamedKey::Delete) => input.delete(),
        Key::Named(NamedKey::ArrowLeft) if word => input.move_word_left(extend),
        Key::Named(NamedKey::ArrowLeft) => input.move_left(extend),
        Key::Named(NamedKey::ArrowRight) if word => input.move_word_right(extend),
        Key::Named(NamedKey::ArrowRight) => input.move_right(extend),
        Key::Named(NamedKey::Home) => input.home(extend),
        Key::Named(NamedKey::End) => input.end(extend),
        Key::Named(_) => {},
    }
}

/// The edit handler for [`textarea`]: like [`edit`] but multi-line. `Enter`
/// inserts a newline; `ArrowUp` / `ArrowDown` move between lines keeping a sticky
/// goal column; bare `Home` / `End` scope to the current line (`home_line` /
/// `end_line`) while `Ctrl`/`Cmd`+`Home`/`End` jump to the buffer ends. Word motion
/// (`Ctrl`/`Alt`+`←`/`→`, `Ctrl`/`Alt`+`Backspace`/`Delete`) and everything else
/// (typing, Shift to extend) matches the single-line field.
pub(crate) fn edit_multiline(input: &mut TextInput, ev: KeyEvent) {
    let extend = ev.mods.shift;
    // Word motion on Ctrl (Win/Linux) or Alt/Option (macOS); document motion (Ctrl/Cmd
    // + Home/End → buffer start/end) on Ctrl or Cmd, while bare Home/End stay line-scoped.
    // On Win/Linux a held Ctrl satisfies *both* flags, which is harmless: `word` gates only
    // ←/→/Backspace/Delete and `doc` gates only Home/End (disjoint keys), and each guarded
    // arm precedes its plain fallback, so the modified action always wins for its key.
    let word = ev.mods.ctrl || ev.mods.alt;
    let doc = ev.mods.ctrl || ev.mods.meta;
    match ev.key {
        Key::Character(ref s) if (ev.mods.ctrl || ev.mods.meta) && s.eq_ignore_ascii_case("a") => {
            input.select_all()
        },
        Key::Character(s) => input.insert_str(&s),
        Key::Named(NamedKey::Space) => input.insert_str(" "),
        Key::Named(NamedKey::Enter) => input.insert_str("\n"),
        Key::Named(NamedKey::Backspace) if word => input.delete_word_left(),
        Key::Named(NamedKey::Backspace) => input.backspace(),
        Key::Named(NamedKey::Delete) if word => input.delete_word_right(),
        Key::Named(NamedKey::Delete) => input.delete(),
        Key::Named(NamedKey::ArrowLeft) if word => input.move_word_left(extend),
        Key::Named(NamedKey::ArrowLeft) => input.move_left(extend),
        Key::Named(NamedKey::ArrowRight) if word => input.move_word_right(extend),
        Key::Named(NamedKey::ArrowRight) => input.move_right(extend),
        Key::Named(NamedKey::ArrowUp) => input.move_up(extend),
        Key::Named(NamedKey::ArrowDown) => input.move_down(extend),
        Key::Named(NamedKey::Home) if doc => input.home(extend),
        Key::Named(NamedKey::Home) => input.home_line(extend),
        Key::Named(NamedKey::End) if doc => input.end(extend),
        Key::Named(NamedKey::End) => input.end_line(extend),
        Key::Named(_) => {},
    }
}

/// The concrete view type the field produces.
///
/// An [`on_key`](crate::on_key)-wrapped element whose children are the rendered
/// text split at the caret into `(before, preedit, after)` — the middle being
/// the IME preedit as an underlined `<span>` (empty when not composing). The
/// host paints the caret over it. [`text_field`] returns this *behind* an `impl
/// View`; [`text_field_typed`] returns it *named*, for a host that must spell
/// its concrete view type.
pub type TextField = OnKey<
    El<Vec<crate::styled_field::FieldChild>, TextInput, ()>,
    TextInput,
    (),
    fn(&mut TextInput, KeyEvent),
>;

/// Build the field's `<input>` / `<textarea>` body: the text as the element's
/// children, split at the caret to splice the IME preedit, then the ghost suffix.
/// Delegates to the one style-aware body in [`styled_field`](crate::styled_field)
/// with no styles (the plain case); [`styled_textarea`](crate::styled_textarea) is
/// the same body with highlight classes, so the plain and styled fields share one
/// implementation.
fn field_body(
    tag: &str,
    input: &TextInput,
) -> El<Vec<crate::styled_field::FieldChild>, TextInput, ()> {
    el::<_, TextInput, ()>(tag, crate::styled_field::field_children(input, &[]))
}

/// Build the concrete [`TextField`] for `input` (the shared implementation
/// behind both [`text_field`] and [`text_field_typed`]).
fn build_text_field(input: &TextInput) -> TextField {
    let handler: fn(&mut TextInput, KeyEvent) = edit;
    on_key(field_body("input", input), handler)
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
pub fn text_field(
    input: &TextInput,
) -> impl View<TextInput, (), ServalCtx, Element = ServalElement> + use<> {
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

/// Build the concrete view for a multi-line [`textarea`]. Structurally identical
/// to a [`TextField`] (an `on_key`-wrapped element over a [`TextInput`]); the
/// difference is the [`edit_multiline`] handler and a `<textarea>` tag. With
/// `\n`s in the buffer, serval/parley break it into lines (serval feeds raw text
/// to parley, which honors `\n`).
fn build_textarea(input: &TextInput) -> TextField {
    let handler: fn(&mut TextInput, KeyEvent) = edit_multiline;
    on_key(field_body("textarea", input), handler)
}

/// A reusable multi-line text field over a [`TextInput`] — [`text_field`]'s
/// multi-line sibling. `Enter` inserts a newline (which renders as a line break),
/// `ArrowUp` / `ArrowDown` move between lines, `Home` / `End` scope to the line.
/// Composable via [`lens`](crate::lens) like [`text_field`].
///
/// Lines are `\n`-delimited in the buffer; up/down navigate those hard lines with a
/// sticky goal column. (Soft-wrap visual-line navigation needs the layout — the
/// separate `serval_layout::caret_byte_vertical` path a host can wire instead.)
pub fn textarea(
    input: &TextInput,
) -> impl View<TextInput, (), ServalCtx, Element = ServalElement> + use<> {
    build_textarea(input)
}

/// [`textarea`] with its concrete return type named (for a host storing the
/// runner in a struct field; see [`text_field_typed`]).
pub fn textarea_typed(input: &TextInput) -> TextField {
    build_textarea(input)
}

#[cfg(test)]
mod tests {
    use super::{TextInput, edit, edit_multiline};
    use crate::{Key, KeyEvent, Modifiers, NamedKey};

    /// A `TextInput` with the caret at char index `caret`. Fixtures are ASCII, so a
    /// char index equals a byte offset for [`TextInput::set_caret_byte`].
    fn at(text: &str, caret: usize) -> TextInput {
        let mut t = TextInput::new(text);
        t.set_caret_byte(caret, false);
        t
    }

    fn named(k: NamedKey, mods: Modifiers) -> KeyEvent {
        KeyEvent::with_mods(Key::Named(k), mods)
    }

    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    };

    #[test]
    fn ctrl_arrow_routes_to_word_motion() {
        let mut t = at("foo bar", 0);
        edit_multiline(&mut t, named(NamedKey::ArrowRight, CTRL));
        assert_eq!(t.caret(), 3); // word-right, not one char right
    }

    #[test]
    fn home_end_are_line_scoped_but_ctrl_home_end_span_the_buffer() {
        let buf = "line one\nline two";
        let mut t = at(buf, 12); // somewhere on line two
        edit_multiline(&mut t, named(NamedKey::Home, Modifiers::default()));
        assert_eq!(t.caret(), 9); // bare Home: start of line two
        edit_multiline(&mut t, named(NamedKey::Home, CTRL));
        assert_eq!(t.caret(), 0); // Ctrl+Home: buffer start
        edit_multiline(&mut t, named(NamedKey::End, CTRL));
        assert_eq!(t.caret(), buf.chars().count()); // Ctrl+End: buffer end
    }

    #[test]
    fn single_line_field_supports_word_motion() {
        let mut t = at("foo bar", 0);
        edit(&mut t, named(NamedKey::ArrowRight, CTRL));
        assert_eq!(t.caret(), 3); // word-right in the single-line field too
        edit(&mut t, named(NamedKey::Backspace, CTRL));
        assert_eq!(t.text(), " bar"); // Ctrl+Backspace kills "foo"
    }
}
