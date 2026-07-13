//! Clean-room, generated CSS property and cascade engine.
//!
//! The generated catalog is the executable contract for Livery's first lane:
//! Cambium structural UI. Value parsing and cascade behavior grow against this
//! bounded property set.

#![forbid(unsafe_code)]

include!(concat!(env!("OUT_DIR"), "/properties.rs"));
