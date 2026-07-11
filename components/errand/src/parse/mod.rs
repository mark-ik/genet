//! Smolweb document parsers — the format structure of the protocols errand's
//! transport speaks, as a model-free, host-agnostic layer.
//!
//! Each submodule parses one protocol's payload into a small AST: bytes/str in,
//! a per-format value out. There is no render model and no document model here —
//! a consumer maps the AST to its own (a native viewer to widgets, a notes engine
//! to its block model). Parse is independent of transport: compose them (fetch a
//! capsule, then parse its body) or parse a local file with no fetch at all.
//!
//! The dep-free parsers here compile unconditionally; only the feed parser (which
//! needs an XML reader) sits behind the `parse-feed` feature.

pub mod feed;
pub mod gemtext;
pub mod gopher;
pub mod nex;
