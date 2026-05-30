/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Text views: bare strings (and numbers) become text nodes.
//!
//! `xilem_core` provides blanket `View` impls for `&'static str`, `String`,
//! `Cow<'static, str>`, and the integer/float primitives, *if* the context
//! implements [`OrphanView`] for that type. This mirrors `xilem_web`'s
//! `text.rs`: a string view creates a `ScriptedDom` text node and, on rebuild,
//! resets its character data when the value changes.

use crate::pod::{ServalElement, ServalElementMut};
use crate::{ServalCtx, DomHandle};
use layout_dom_api::LayoutDomMut;
use xilem_core::{MessageCtx, MessageResult, Mut, OrphanView};

/// Create a text view holding `s` — the text half of the Element/Text split.
///
/// Symmetric with [`el`](crate::el) for readability. The returned `String` is a
/// text view via `xilem_core`'s `OrphanView` (bare `&str`/`String` work as text
/// views too). Text views are deliberately *not*
/// [`ElementView`](crate::ElementView), so element-only operations
/// (`on_click` / `on_key` / `El::attr`) reject them at compile time.
pub fn text(s: impl Into<String>) -> String {
    s.into()
}

/// Create a text node holding `data` and wrap it as a [`ServalElement`].
fn build_text(dom: &DomHandle, data: &str) -> ServalElement {
    let node = dom.borrow_mut().create_text(data);
    ServalElement::new(node, dom.clone())
}

/// Reset a text node's character data if it changed.
fn rebuild_text(element: &ServalElementMut<'_>, prev: &str, next: &str) {
    if prev != next {
        element.dom.borrow_mut().set_text(*element.node, next);
    }
}

macro_rules! impl_string_view {
    ($ty:ty) => {
        impl<State: 'static, Action> OrphanView<$ty, State, Action> for ServalCtx {
            type OrphanElement = ServalElement;
            type OrphanViewState = ();

            fn orphan_build(
                view: &$ty,
                ctx: &mut ServalCtx,
                _: &mut State,
            ) -> (Self::OrphanElement, Self::OrphanViewState) {
                (build_text(&ctx.dom(), view), ())
            }

            fn orphan_rebuild(
                new: &$ty,
                prev: &$ty,
                (): &mut Self::OrphanViewState,
                _ctx: &mut ServalCtx,
                element: Mut<'_, Self::OrphanElement>,
                _: &mut State,
            ) {
                rebuild_text(&element, prev, new);
            }

            fn orphan_teardown(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _ctx: &mut ServalCtx,
                _element: Mut<'_, Self::OrphanElement>,
            ) {
            }

            fn orphan_message(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _message: &mut MessageCtx,
                _element: Mut<'_, Self::OrphanElement>,
                _app_state: &mut State,
            ) -> MessageResult<Action> {
                MessageResult::Stale
            }
        }
    };
}

impl_string_view!(&'static str);
impl_string_view!(String);
impl_string_view!(std::borrow::Cow<'static, str>);

macro_rules! impl_to_string_view {
    ($ty:ty) => {
        impl<State: 'static, Action> OrphanView<$ty, State, Action> for ServalCtx {
            type OrphanElement = ServalElement;
            type OrphanViewState = ();

            fn orphan_build(
                view: &$ty,
                ctx: &mut ServalCtx,
                _: &mut State,
            ) -> (Self::OrphanElement, Self::OrphanViewState) {
                (build_text(&ctx.dom(), &view.to_string()), ())
            }

            fn orphan_rebuild(
                new: &$ty,
                prev: &$ty,
                (): &mut Self::OrphanViewState,
                _ctx: &mut ServalCtx,
                element: Mut<'_, Self::OrphanElement>,
                _: &mut State,
            ) {
                if prev != new {
                    element.dom.borrow_mut().set_text(*element.node, &new.to_string());
                }
            }

            fn orphan_teardown(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _ctx: &mut ServalCtx,
                _element: Mut<'_, Self::OrphanElement>,
            ) {
            }

            fn orphan_message(
                _view: &$ty,
                _view_state: &mut Self::OrphanViewState,
                _message: &mut MessageCtx,
                _element: Mut<'_, Self::OrphanElement>,
                _app_state: &mut State,
            ) -> MessageResult<Action> {
                MessageResult::Stale
            }
        }
    };
}

impl_to_string_view!(f32);
impl_to_string_view!(f64);
impl_to_string_view!(i8);
impl_to_string_view!(u8);
impl_to_string_view!(i16);
impl_to_string_view!(u16);
impl_to_string_view!(i32);
impl_to_string_view!(u32);
impl_to_string_view!(i64);
impl_to_string_view!(u64);
impl_to_string_view!(u128);
impl_to_string_view!(isize);
impl_to_string_view!(usize);
