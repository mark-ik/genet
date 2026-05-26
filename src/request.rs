/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The Fetch-spec [`Request`] — mode, credentials, redirect policy, body.
//!
//! Distinct from the *wire-level* `http::Request`: this carries the
//! Fetch-algorithm concepts (request mode, credentials mode, response tainting
//! inputs). Whether to thin-wrap the `http` crate instead is **plan open
//! question #1** — deferred; for now netfetcher owns its types.

use bytes::Bytes;
use url::Url;

/// A Fetch request.
#[derive(Clone, Debug)]
pub struct Request {
    pub url: Url,
    pub method: Method,
    /// Header list. Placeholder shape — becomes an ordered header map once the
    /// `http`-crate-vs-own-types question (plan OQ#1) is settled.
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
    pub mode: RequestMode,
    pub credentials: Credentials,
    pub redirect: RedirectMode,
}

impl Request {
    /// A plain `GET` with default modes — the increment-1 starting point.
    pub fn get(url: Url) -> Self {
        Self {
            url,
            method: Method::Get,
            headers: Vec::new(),
            body: None,
            mode: RequestMode::default(),
            credentials: Credentials::default(),
            redirect: RedirectMode::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Patch,
    Options,
}

/// Fetch request mode (WHATWG Fetch §2.2.5) — gates CORS and response tainting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RequestMode {
    #[default]
    Cors,
    SameOrigin,
    NoCors,
    Navigate,
}

/// Credentials mode — whether cookies/auth travel with the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Credentials {
    #[default]
    SameOrigin,
    Omit,
    Include,
}

/// Redirect handling (WHATWG Fetch §2.2.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}
