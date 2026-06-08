/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Subresource Integrity (W3C SRI): verify a response body against the request's
//! `integrity` metadata.
//!
//! The metadata is a space-separated list of `alg-base64` entries. Only the
//! strongest algorithm present is checked (sha512 > sha384 > sha256), and the
//! body's digest must equal at least one of that algorithm's listed hashes. The
//! base64 may be standard or URL-safe, padded or not.

use base64::Engine;
use ring::digest::{digest, SHA256, SHA384, SHA512, Algorithm};

/// Verify `body` against SRI `metadata`. Returns `true` when the metadata has no
/// valid `alg-hash` entry (no integrity requested, so no check) or the body's
/// strongest-algorithm digest matches a listed hash; `false` on a mismatch.
pub(crate) fn verify(metadata: &str, body: &[u8]) -> bool {
    let mut best_rank = 0u8;
    let mut expected: Vec<Vec<u8>> = Vec::new();
    for token in metadata.split_whitespace() {
        let Some((alg, b64)) = token.split_once('-') else {
            continue;
        };
        let rank = match alg {
            "sha256" => 1,
            "sha384" => 2,
            "sha512" => 3,
            _ => 0,
        };
        if rank == 0 {
            continue;
        }
        let Some(bytes) = decode(b64) else {
            continue;
        };
        if rank > best_rank {
            best_rank = rank;
            expected.clear();
        }
        if rank == best_rank {
            expected.push(bytes);
        }
    }
    if best_rank == 0 {
        return true; // no parseable metadata: nothing to enforce
    }
    let alg: &Algorithm = match best_rank {
        1 => &SHA256,
        2 => &SHA384,
        _ => &SHA512,
    };
    let actual = digest(alg, body);
    expected.iter().any(|e| e.as_slice() == actual.as_ref())
}

/// True if `metadata` contains at least one valid `alg-hash` entry (so an
/// integrity check would be enforced). Used to reject an opaque response, whose
/// body cannot be verified.
pub(crate) fn is_enforced(metadata: &str) -> bool {
    metadata.split_whitespace().any(|token| {
        token
            .split_once('-')
            .is_some_and(|(alg, b64)| matches!(alg, "sha256" | "sha384" | "sha512") && decode(b64).is_some())
    })
}

/// Decode an SRI base64 hash: standard or URL-safe alphabet, padding optional.
fn decode(s: &str) -> Option<Vec<u8>> {
    let normalized: String = s
        .chars()
        .filter(|&c| c != '=')
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(normalized.as_bytes())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // sha256("abc") = ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=
    const ABC_SHA256: &str = "sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=";

    #[test]
    fn empty_metadata_passes() {
        assert!(verify("", b"anything"));
        assert!(!is_enforced(""));
    }

    #[test]
    fn matching_sha256_passes_mismatch_fails() {
        assert!(verify(ABC_SHA256, b"abc"));
        assert!(!verify(ABC_SHA256, b"abcd"));
        assert!(is_enforced(ABC_SHA256));
    }

    #[test]
    fn strongest_algorithm_wins() {
        // Valid sha256 but an invalid (bogus) sha512: the stronger sha512 governs.
        let meta = format!("{ABC_SHA256} sha512-bogusbogusbogus");
        // sha512-bogus decodes? "bogusbogusbogus" is valid base64 chars → decodes
        // to some bytes that won't match → enforced and fails.
        assert!(is_enforced(&meta));
        assert!(!verify(&meta, b"abc"));
    }

    #[test]
    fn url_safe_and_unpadded_accepted() {
        // Same hash, '+'/'/' swapped to '-'/'_' and padding stripped.
        let url = ABC_SHA256.replace('+', "-").replace('/', "_").replace('=', "");
        assert!(verify(&url, b"abc"));
    }
}
