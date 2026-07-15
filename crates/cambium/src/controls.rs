/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Reusable form controls built on the [`on_key`](crate::on_key) foundation.
//!
//! [`text_field`] and [`textarea`] use a [`TextInput`] state containing the
//! buffer, caret, selection, composition, and edit history. They compose onto
//! larger application state through [`lens`](crate::lens). Buttons, checkboxes,
//! and switches share the same focus and event-routing foundation.
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
