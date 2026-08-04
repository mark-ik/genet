//! Gemtext parser — re-exported from
//! [`gemini-protocol`](https://crates.io/crates/gemini-protocol).
//!
//! The grammar moved out on 2026-08-03 under the smolweb home decision. It
//! lives with gemini because gemini's specification defines it, and it sits
//! behind that crate's dependency-free layer, so pulling the grammar does not
//! pull a TLS stack. The path stays `errand::parse::gemtext` so consumers do
//! not have to care, and the types are the same types, not a copy.
//!
//! Five other smolweb formats in this crate's cone serve `text/gemini` bodies
//! (spartan, guppy, scroll, misfin, and titan), and all of them parse through
//! exactly this grammar.

pub use gemini_protocol::gemtext::{GemLine, parse};
