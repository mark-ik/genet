//! Clean-room, generated CSS property and cascade engine.
//!
//! The generated catalog is the executable contract for Livery's first lane:
//! Cambium structural UI. Value parsing and cascade behavior grow against this
//! bounded property set.

#![forbid(unsafe_code)]

pub mod cascade;
pub mod media;
pub mod selector;
pub mod stylesheet;
pub mod values;

include!(concat!(env!("OUT_DIR"), "/properties.rs"));
