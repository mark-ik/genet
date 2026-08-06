//! Scrolltext parser — re-exported from
//! [`scroll-protocol`](https://crates.io/crates/scroll-protocol).
//!
//! The grammar lives with its protocol under the smolweb home decision; the
//! path here keeps errand's parse family in one place. Richer than gemtext by
//! design: five heading levels, nested quotes and lists with verbatim ordered
//! markers, tagged code blocks, input links, link relations, inline markup
//! ([`spans`]), and linetype escaping.

pub use scroll_protocol::scrolltext::{
    Polarity, Relation, ScrollLine, Span, SpanKind, parse, spans,
};
