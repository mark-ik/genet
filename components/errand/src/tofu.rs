//! Trust-on-first-use certificate pinning for the TLS schemes.
//!
//! Gemini capsules are conventionally self-signed, so there is no CA to
//! anchor trust. The real TOFU model pins the SHA-256 of a host's leaf
//! certificate on first contact and requires every later visit to present
//! the same one — so a *changed* certificate (a man-in-the-middle, a key
//! rotation, a moved host) is surfaced rather than silently accepted.
//!
//! The pin store is a [`TofuStore`] trait so the embedder chooses
//! durability: [`InMemoryTofu`] holds pins for the process; a host with a
//! profile supplies its own durable store (a file, a database, an engram).
//! The store is installed once via [`set_trust_store`]; until then errand
//! is [`PermissiveTofu`] — accept-any, byte-for-byte the pre-pinning
//! behaviour — so adding this module changes nothing for callers that do
//! not opt in.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

/// A store of pinned leaf-certificate fingerprints, keyed by host.
///
/// `Send + Sync` so one store serves every concurrent fetch. Implementations
/// decide where pins live; the two `errand` ships are [`InMemoryTofu`] and
/// [`PermissiveTofu`].
pub trait TofuStore: Send + Sync {
    /// The pinned SHA-256 fingerprint for `host`, or `None` if this is a
    /// first contact.
    fn fingerprint(&self, host: &str) -> Option<[u8; 32]>;
    /// Record (pin) a fingerprint for `host` — called after a clean first
    /// contact, or after the embedder accepts a change.
    fn pin(&self, host: &str, fingerprint: [u8; 32]);
}

/// A process-lifetime, in-memory [`TofuStore`]. Pins last as long as the
/// store; the right choice for a session, a test, or any host that does not
/// need pins to survive a restart.
#[derive(Debug, Default)]
pub struct InMemoryTofu {
    pins: Mutex<HashMap<String, [u8; 32]>>,
}

impl InMemoryTofu {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TofuStore for InMemoryTofu {
    fn fingerprint(&self, host: &str) -> Option<[u8; 32]> {
        self.pins.lock().unwrap().get(host).copied()
    }
    fn pin(&self, host: &str, fingerprint: [u8; 32]) {
        self.pins
            .lock()
            .unwrap()
            .insert(host.to_string(), fingerprint);
    }
}

/// The accept-any store: never reports a pin, never records one. With it
/// installed (the default when none is set), every TLS handshake is treated
/// as a first contact and accepted — the permissive TOFU `errand` used
/// before pinning existed.
#[derive(Debug, Default)]
pub struct PermissiveTofu;

impl TofuStore for PermissiveTofu {
    fn fingerprint(&self, _host: &str) -> Option<[u8; 32]> {
        None
    }
    fn pin(&self, _host: &str, _fingerprint: [u8; 32]) {}
}

static TRUST_STORE: RwLock<Option<Arc<dyn TofuStore>>> = RwLock::new(None);

/// Install the process-wide [`TofuStore`] errand's TLS schemes pin against.
///
/// Call once at startup (e.g. with an [`InMemoryTofu`] or a durable store).
/// Until a store is installed, errand uses [`PermissiveTofu`]. The most
/// recent call wins.
pub fn set_trust_store(store: Arc<dyn TofuStore>) {
    *TRUST_STORE.write().unwrap() = Some(store);
}

/// The installed trust store, or a [`PermissiveTofu`] when none is set.
pub(crate) fn trust_store() -> Arc<dyn TofuStore> {
    TRUST_STORE
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Arc::new(PermissiveTofu))
}

/// The SHA-256 fingerprint of a certificate's DER bytes, via rustls's ring
/// provider (already in the dependency cone).
pub(crate) fn fingerprint(cert_der: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, cert_der);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// Lowercase-hex of a fingerprint, for the [`crate::Error::CertificateChanged`]
/// message.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_pins_and_recalls_per_host() {
        let tofu = InMemoryTofu::new();
        assert!(tofu.fingerprint("a.example").is_none());
        tofu.pin("a.example", [1u8; 32]);
        assert_eq!(tofu.fingerprint("a.example"), Some([1u8; 32]));
        assert!(tofu.fingerprint("b.example").is_none(), "pins are per host");
    }

    #[test]
    fn permissive_never_pins() {
        let tofu = PermissiveTofu;
        tofu.pin("a.example", [1u8; 32]);
        assert!(
            tofu.fingerprint("a.example").is_none(),
            "permissive store treats every visit as first contact"
        );
    }

    #[test]
    fn fingerprint_is_stable_and_hex_round_trips() {
        assert_eq!(fingerprint(b"cert"), fingerprint(b"cert"));
        assert_ne!(fingerprint(b"cert"), fingerprint(b"other"));
        assert_eq!(hex(&[0xde, 0xad, 0x01]), "dead01");
    }
}
