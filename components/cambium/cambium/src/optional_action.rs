/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! [`OptionalAction`]: let an event handler return `()`, an `Action`, or an
//! `Option<Action>`, uniformly.
//!
//! A sealed marker trait [`Action`] tags the types an app uses as bubbling actions, and
//! [`OptionalAction`] lets [`on_click`](crate::on_click)'s handler be
//! polymorphic on its return type so the same view covers both
//!   * a *unit* handler (`Fn(&mut State, _)` returning `()`,
//!     which yields no action), and
//!   * an action-bubbling handler (`Fn(&mut State, _) -> A`, which feeds
//!     [`MessageResult::Action`](meristem::MessageResult::Action) and composes
//!     up through [`map_action`](meristem::map_action)).
//!
//! A `()` return becomes `None`, yielding `MessageResult::Nop`.

/// Implement this (empty) marker for any type you want to bubble as an action.
///
/// It exists so the blanket [`OptionalAction`] impls for `A` / `Option<A>` do
/// not overlap the impl for `()`: only types the app explicitly opts in are
/// treated as actions.
pub trait Action {}

/// Allows a handler callback to be polymorphic in its return type — `()`, `A`,
/// or `Option<A>` — exposing all three as `Option<A>`.
///
/// An implementation detail of the event views; sealed so downstream crates do
/// not add surprising impls that would change dispatch semantics.
pub trait OptionalAction<A>: sealed::Sealed {
    /// The action this return value carries, if any.
    fn action(self) -> Option<A>;
}

mod sealed {
    #[expect(
        unnameable_types,
        reason = "see https://predr.ag/blog/definitive-guide-to-sealed-traits-in-rust/"
    )]
    pub trait Sealed {}
}

impl sealed::Sealed for () {}
impl<A> OptionalAction<A> for () {
    fn action(self) -> Option<A> {
        None
    }
}

impl<A: Action> sealed::Sealed for A {}
impl<A: Action> OptionalAction<A> for A {
    fn action(self) -> Option<A> {
        Some(self)
    }
}

impl<A: Action> sealed::Sealed for Option<A> {}
impl<A: Action> OptionalAction<A> for Option<A> {
    fn action(self) -> Self {
        self
    }
}
