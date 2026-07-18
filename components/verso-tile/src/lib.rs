/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `verso-tile` — the engine-flip tile.
//!
//! A *flip* re-presents the same page through a different engine with the
//! user's place and session carried across (the compatibility-view charter;
//! mere's `verso_docs/technical_architecture/2026-06-10_compatibility_view_charter.md`
//! is the design record). Where inker's multiplexer picks an engine per
//! address, the flip is its **dynamic** counterpart: it swaps engines
//! mid-session, carrying cookies, scroll, and forms from a glass-box donor to
//! a black-box receiver and back.
//!
//! One crate, four modules — consolidated from the mere workspace's four-crate
//! family (2026-07-09, inker-genet adoption plan):
//!
//! * [`api`] (was `verso-api`) — the engine-agnostic contract: [`api::PortableViewState`],
//!   the layer lattice, and the donor / back / receiver traits. Dependency-free
//!   so an external black-box implementor reaches it without engine deps.
//! * [`flip`] (was `verso`) — the orchestrator: masks the carry to the layers
//!   both sides support (degrade, never block) and runs the one-hop
//!   forward/back choreography. A secondary never implements
//!   [`api::FlipDonor`], so flips cannot chain.
//! * [`scry`] (was `verso-scry`) — the black-box receiver: a two-phase
//!   forward-inject state machine (cookies + navigate, then restore on load)
//!   over the thin [`scry::ScrySurface`] seam a host implements on its
//!   concrete WebView producer.
//! * [`genet`] (was `verso-genet`; behind the `genet-donor` feature) — the
//!   glass-box donor over genet's scripted DOM plus host-fed runtime and
//!   session state.

pub mod api;
pub mod flip;
#[cfg(feature = "genet-donor")]
pub mod genet;
pub mod scry;
