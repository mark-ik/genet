/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! HSTS + mixed-content upgrade/blocking, and the SameSite same-site test.

use url::Url;

use crate::hsts;
use crate::FetchContext;

/// Resolve HSTS + mixed content for `url`, returning `true` if the request must
/// be **blocked** (a network error).
///
/// HSTS is host-keyed and independent of content type: a known host upgrades and
/// proceeds. Otherwise, in a secure (https-origin) context the mixed-content
/// active/passive split applies — optionally-blockable destinations (image /
/// audio / video) are auto-upgraded http→https, and blockable ones (script /
/// style / font / document / the empty fetch() destination) are blocked. Outside
/// a secure context with no HSTS entry, plain http is left as-is.
pub(super) fn resolve_mixed_content(
    url: &mut Url,
    destination: crate::Destination,
    secure_context: bool,
    cx: &FetchContext,
) -> bool {
    if url.scheme() != "http" {
        return false;
    }
    if hsts::should_upgrade(url, cx.hsts.as_ref()) {
        let _ = url.set_scheme("https");
        return false;
    }
    if !secure_context {
        return false;
    }
    if destination.is_optionally_blockable() {
        let _ = url.set_scheme("https");
        false
    } else {
        true
    }
}

/// Same-site test for SameSite cookie gating: equal registrable domains, via the
/// Public Suffix List. No initiator origin = a top-level request → same-site.
pub(super) fn is_same_site(origin: Option<&url::Origin>, target: &Url) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    match (origin_host(origin), target.host_str()) {
        (Some(oh), Some(th)) => same_registrable_domain(&oh, th),
        _ => false,
    }
}

fn origin_host(origin: &url::Origin) -> Option<String> {
    match origin {
        url::Origin::Tuple(_, host, _) => Some(host.to_string()),
        url::Origin::Opaque(_) => None,
    }
}

/// Whether two hosts share a registrable domain (eTLD+1) per the PSL. Hosts the
/// PSL can't resolve to a registrable domain — IP literals, single labels,
/// unlisted TLDs — fall back to an exact host match.
fn same_registrable_domain(a: &str, b: &str) -> bool {
    match (psl::domain_str(a), psl::domain_str(b)) {
        (Some(da), Some(db)) => da.eq_ignore_ascii_case(db),
        _ => a.eq_ignore_ascii_case(b),
    }
}
