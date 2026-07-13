/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `engine_observables_api` — the common-minimum query traits every
//! Hekate render lane (Nematic / Genet / Scrying) publishes for
//! downstream consumers (mere-host, Apparatus inspector, Hekate
//! indexing/extract pipeline).
//!
//! See [docs/2026-05-17_hekate_lanes_observables.md](../../../docs/2026-05-17_hekate_lanes_observables.md)
//! for the design. The "raw plane storage stays implementation
//! detail of each lane; the permanent ABI is these query traits"
//! framing is load-bearing — internal plane shapes can evolve, the
//! trait surface here is what consumers depend on.
//!
//! ## Modules
//!
//! - [`semantic`] — common semantic facts (title, headings, links,
//!   anchors) + engine-specific extensions (HTML / Nematic / feed).
//! - [`fragment`] — laid-out geometry queries (hit-test, box-model,
//!   anchor → fragments, selection rects).
//! - [`interaction`] — focus, selection, affordances at a point,
//!   activation target lookup.
//! - [`loading`] — request/response state, redirects, MIME, TLS,
//!   cache origin, errors.
//!
//! Each trait carries a `generation_id() -> u64` epoch (or its
//! equivalent) so consumers can cache against it: the value rolls
//! whenever the underlying plane regenerates.

#![deny(unsafe_code)]

pub mod fragment;
pub mod interaction;
pub mod loading;
pub mod quiescence;
pub mod semantic;
pub mod stats;
pub mod types;

pub use fragment::*;
pub use interaction::*;
pub use loading::*;
pub use quiescence::*;
pub use semantic::*;
pub use stats::*;
pub use types::*;
