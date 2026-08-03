//! Gopher menu parser — re-exported from
//! [`gopher-protocol`](https://crates.io/crates/gopher-protocol).
//!
//! The grammar moved out on 2026-08-03 under the smolweb home decision: RFC
//! 1436 menus and RFC 4266 URL synthesis are the spec, so they belong to the
//! protocol's crate rather than to the client that composes it. The path stays
//! `errand::parse::gopher` so consumers do not have to care, and the types are
//! the same types, not a copy.
//!
//! [`GopherPlus`] is new with the move: gopher's successor marks items in a
//! fifth field that RFC 1436 menus do not have. Gopher+ attribute blocks,
//! alternate views, and ASK forms live in `gopher_protocol::plus`.

pub use gopher_protocol::menu::{GopherItem, GopherKind, GopherPlus, parse};
