//! Misfin (`misfin://`, port 1958): gemini-style mail **send**, via the
//! [`misfin`](https://crates.io/crates/misfin) crate's client (on the same
//! ring provider this crate builds rustls with).
//!
//! Misfin is gemini-flavoured peer-to-peer mail: a single TLS transaction
//! presenting a client certificate (the sender's identity *is* its
//! certificate), a `misfin://<mailbox>@<host> <message>\r\n` request line,
//! and a `<status> <meta>\r\n` reply. It is the write companion for mail, as
//! [`titan`](crate::titan_upload) is for gemini.
//!
//! errand owns only the client side and the mapping onto [`Response`]. The
//! certificate is supplied by the caller ([`ClientIdentity`]); errand never
//! generates or stores certs — that is the sender's identity layer.
//! **Receiving** misfin (serving a mailbox) is the misfin crate's `server`
//! feature, outside errand's client-transport scope.

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use url::Url;

use crate::{Error, Response, Status};

/// Misfin's well-known port.
pub const MISFIN_PORT: u16 = 1958;

/// A client identity for a misfin send: the certificate chain + private key,
/// in DER, supplied by the caller. The leaf comes first; further chain
/// certificates (the spec's mailbox-signed-by-host setup) follow. The key
/// must be PKCS#8 (the format Mere's vault-derived identities produce).
pub struct ClientIdentity {
    /// The client certificate chain (leaf first), DER-encoded.
    pub cert_chain: Vec<CertificateDer<'static>>,
    /// The client private key, DER-encoded (PKCS#8).
    pub private_key: PrivateKeyDer<'static>,
}

/// Send `message` to a misfin `recipient` (`misfin://mailbox@host[:port]`),
/// presenting `identity` as the client certificate. Returns the recipient
/// host's gemini-format [`Response`] (2x delivered, 3x redirect, 4x/5x
/// failure, 6x certificate). errand does not follow misfin redirects; the
/// caller decides.
pub async fn send(
    recipient: &Url,
    message: &str,
    identity: ClientIdentity,
) -> Result<Response, Error> {
    if recipient.scheme() != "misfin" {
        return Err(Error::UnsupportedScheme(recipient.scheme().to_string()));
    }
    let address = misfin::MisfinAddress::from_url(recipient).map_err(Error::BadUrl)?;

    let options = misfin::SendOptions {
        identity: Some(identity_material(&identity)?),
        extra_chain_der: identity
            .cert_chain
            .iter()
            .skip(1)
            .map(|cert| cert.as_ref().to_vec())
            .collect(),
        // The addr-spec omits the port; an explicit URL port is only the
        // dial target.
        port: recipient.port(),
        ..Default::default()
    };
    let receipt = misfin::send(&address, message, &options)
        .await
        .map_err(map_error)?;
    Ok(map_receipt(recipient, receipt))
}

fn identity_material(identity: &ClientIdentity) -> Result<misfin::MisfinIdentityMaterial, Error> {
    let leaf = identity
        .cert_chain
        .first()
        .ok_or_else(|| Error::Protocol("misfin identity has an empty certificate chain".into()))?;
    let PrivateKeyDer::Pkcs8(key) = &identity.private_key else {
        return Err(Error::Protocol(
            "misfin send requires a PKCS#8 private key".into(),
        ));
    };
    Ok(misfin::MisfinIdentityMaterial {
        certificate_der: leaf.as_ref().to_vec(),
        private_key_pkcs8_der: key.secret_pkcs8_der().to_vec(),
    })
}

/// Map a misfin receipt onto errand's normalized response, by the status
/// code's first digit (the same axis the gemini parser uses).
fn map_receipt(url: &Url, receipt: misfin::SendReceipt) -> Response {
    let code = receipt.status.code();
    let status = match code / 10 {
        1 => Status::Input,
        2 => Status::Success,
        3 => Status::Redirect,
        6 => Status::CertRequired,
        _ => Status::Failure,
    };
    Response {
        url: url.clone(),
        status,
        raw_status: Some(code),
        meta: receipt.meta,
        // A misfin reply is a single status line; there is no body.
        body: Vec::new(),
    }
}

fn map_error(error: misfin::SendError) -> Error {
    use misfin::SendError;
    match error {
        SendError::MessageTooLong { request_bytes, max } => Error::Protocol(format!(
            "misfin request is {request_bytes} bytes (max {max}); split the message"
        )),
        SendError::MessageContainsCarriageReturn => {
            Error::Protocol("misfin messages must not contain carriage returns".into())
        },
        SendError::InvalidHost(host) => Error::BadUrl(format!("invalid misfin host '{host}'")),
        SendError::FingerprintMismatch { expected, found } => Error::Protocol(format!(
            "misfin server fingerprint mismatch: expected {expected}, found {found}"
        )),
        SendError::Tls(message) => Error::Connect(message),
        SendError::Io(message) => Error::Io(message),
        SendError::Timeout(_) => Error::Timeout,
        SendError::BadResponse(message) => Error::Protocol(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use misfin::{MisfinStatus, SendReceipt};

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn receipt(status: MisfinStatus, meta: &str) -> SendReceipt {
        SendReceipt {
            status,
            meta: meta.to_string(),
            server_fingerprint: "aa".repeat(32),
        }
    }

    #[test]
    fn receipts_map_by_status_category() {
        let u = url("misfin://alice@example.test");
        let delivered = map_receipt(&u, receipt(MisfinStatus::Delivered, "fp"));
        assert_eq!(delivered.status, Status::Success);
        assert_eq!(delivered.raw_status, Some(20));
        assert_eq!(delivered.meta, "fp");
        assert!(delivered.body.is_empty());

        let moved = map_receipt(
            &u,
            receipt(MisfinStatus::SendHereForever, "misfin://b@c.test"),
        );
        assert_eq!(moved.status, Status::Redirect);
        assert_eq!(moved.raw_status, Some(31));

        let full = map_receipt(&u, receipt(MisfinStatus::MailboxFull, "later"));
        assert_eq!(full.status, Status::Failure);

        let liar = map_receipt(&u, receipt(MisfinStatus::FingerprintChanged, "no"));
        assert_eq!(liar.status, Status::CertRequired);
        assert_eq!(liar.raw_status, Some(63));
    }

    #[tokio::test]
    async fn rejects_a_non_misfin_scheme() {
        let identity = ClientIdentity {
            cert_chain: vec![],
            private_key: PrivateKeyDer::Pkcs8(vec![0u8; 4].into()),
        };
        assert!(matches!(
            send(&url("gemini://example.test/"), "x", identity).await,
            Err(Error::UnsupportedScheme(_))
        ));
    }

    #[tokio::test]
    async fn rejects_a_missing_mailbox() {
        let identity = ClientIdentity {
            cert_chain: vec![],
            private_key: PrivateKeyDer::Pkcs8(vec![0u8; 4].into()),
        };
        assert!(matches!(
            send(&url("misfin://example.test"), "x", identity).await,
            Err(Error::BadUrl(_))
        ));
    }

    #[test]
    fn an_empty_chain_is_rejected_before_any_network() {
        let identity = ClientIdentity {
            cert_chain: vec![],
            private_key: PrivateKeyDer::Pkcs8(vec![0u8; 4].into()),
        };
        assert!(matches!(
            identity_material(&identity),
            Err(Error::Protocol(_))
        ));
    }
}
