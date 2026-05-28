/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Reusable form controls built on the [`on_key`](crate::on_key) foundation.
//!
//! Stage 3 of `docs/2026-05-27_serval_as_host_xilem_serval_plan.md` (the
//! text-field slice): the first *control* on top of the keyboard/focus
//! foundation. [`text_field`] is the editable-text analogue of the Stage 3a
//! `counter_button` component — its state is the field's [`String`], so it
//! composes onto a larger app's field through [`lens`](crate::lens), exactly as
//! `counter_button` composes onto a `u32`.
//!
//! There is no browser `<input>` machinery here: serval lays out a plain
//! element whose text content is the buffer, and [`on_key`](crate::on_key)
//! makes that element focusable and routes typed keys to an edit handler that
//! mutates the [`String`]. The host (`pelt-live`) maps winit key events to the
//! native [`KeyEvent`](crate::KeyEvent); the runner's focus + dispatch
//! ([`dispatch_key`](crate::ServalAppRunner::dispatch_key)) deliver them here.

use crate::pod::ServalElement;
use crate::{El, Key, KeyEvent, NamedKey, OnKey, ServalCtx, View, el, on_key};

/// The edit handler for [`text_field`]: apply one [`KeyEvent`] to `value`.
///
/// A free function (not a closure) so [`text_field`]'s return type names a `fn`
/// pointer rather than an unnameable closure — the same reason the test views
/// use `fn`-pointer handlers. The editing model is the minimal one the
/// foundation needs:
///
/// * [`Key::Character`] appends the produced text (so `"h"`, `"H"`, `"é"`, and
///   any multi-character input from an IME all push verbatim).
/// * [`NamedKey::Backspace`] pops the last `char`.
/// * [`NamedKey::Space`] pushes a literal space — per Stage 3b, the space bar
///   arrives as [`NamedKey::Space`], *not* `Character(" ")`, so the field has
///   to handle it explicitly to be typeable.
/// * [`NamedKey::Enter`] and every other named key are ignored: this is a
///   single-line buffer with no commit/navigation behaviour yet.
fn edit(value: &mut String, ev: KeyEvent) {
    match ev.key {
        Key::Character(s) => value.push_str(&s),
        Key::Named(NamedKey::Backspace) => {
            value.pop();
        },
        Key::Named(NamedKey::Space) => value.push(' '),
        // Enter / Tab / Escape / arrows / Delete / Other: no edit yet. A
        // single-line buffer commits nothing and navigates nowhere; future
        // slices (cursor, selection) give these meaning.
        Key::Named(_) => {},
    }
}

/// The concrete view type the field produces.
///
/// An [`on_key`](crate::on_key)-wrapped `<input>` whose text content is the
/// current buffer. [`text_field`] returns this *behind* an `impl View` (the
/// reusable-component shape), while [`text_field_typed`] returns it *named* —
/// for a host that must spell its concrete view type (e.g. a runner stored in a
/// struct field, which needs a nameable `V`).
pub type TextField = OnKey<El<String, String, ()>, String, (), fn(&mut String, KeyEvent)>;

/// Build the concrete [`TextField`] for `value` (the shared implementation
/// behind both [`text_field`] and [`text_field_typed`]).
fn build_text_field(value: &str) -> TextField {
    let handler: fn(&mut String, KeyEvent) = edit;
    on_key(el::<_, String, ()>("input", value.to_string()), handler)
}

/// A reusable, editable text field whose state *is* the field's [`String`].
///
/// Renders `value` as the text content of an `<input>` element wrapped in
/// [`on_key`](crate::on_key); the `on_key` makes the element focusable and
/// routes typed keys to [`edit`], which mutates the `&mut String`. Knowing
/// nothing but its own `String`, the field composes onto any larger app state
/// through [`lens`](crate::lens) — `lens(|s| text_field(s), |app| &mut app.name)`
/// — just as the Stage 3a `counter_button` composes onto a `u32`.
///
/// `+ use<>` keeps the opaque return type from capturing the `value` borrow's
/// lifetime, so it is a single `V` usable as a `FnMut(&_) -> V` app logic. A
/// host that needs the *named* concrete type (to store the runner's `V`) uses
/// [`text_field_typed`] instead.
///
/// The element is an `<input>` so author CSS can target the field (e.g. a
/// border/background) and so it reads as a control; serval lays it out as
/// whatever the cascade resolves (a `display: block`/`inline-block` box in the
/// host's sheet). It carries no browser `<input>` value semantics — its text is
/// just its content, diffed like any other text on rebuild.
pub fn text_field(value: &str) -> impl View<String, (), ServalCtx, Element = ServalElement> + use<> {
    build_text_field(value)
}

/// [`text_field`] with its concrete return type named.
///
/// Identical behaviour to [`text_field`]; the only difference is the signature
/// returns the named [`TextField`] rather than `impl View`. A host that stores
/// its runner in a struct field needs a nameable view type `V`, so it composes
/// `lens(text_field_typed, …)` (whose `Lens<…>` type can then be spelled) rather
/// than `lens(|s| text_field(s), …)` (whose inner view is opaque).
pub fn text_field_typed(value: &str) -> TextField {
    build_text_field(value)
}
