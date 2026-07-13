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
//!
//! Split by concern, each an `impl TextInput` over the same struct (defined in
//! [`core`]): [`core`] is the buffer, IME/ghost state, single-char editing, and
//! basic caret motion; [`multiline`] is `\n`-line navigation with the sticky
//! goal column (Tier 2); [`word_motion`] is UAX#29 word boundaries (Ctrl/Alt +
//! arrows, Backspace, Delete). The struct's fields are `pub(super)` (visible
//! within this module tree only) so the split files can edit them directly,
//! exactly as the one original impl block did.

mod core;
mod multiline;
mod word_motion;

pub use core::TextInput;
