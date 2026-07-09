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
//! One module per control family: [`text_input`] is the buffer/caret state,
//! [`field`] the `text_field` / `textarea` views and their key handlers,
//! [`toggle`] the checkbox/toggle control, [`button`] the button view.

mod button;
mod field;
mod text_input;
mod toggle;

pub use button::{button, button_with};
pub use field::{TextField, text_field, text_field_typed, textarea, textarea_typed};
pub(crate) use field::{edit, edit_multiline};
pub use text_input::TextInput;
pub use toggle::{Checkbox, checkbox, checkbox_typed, toggle};
