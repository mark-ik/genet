//! The spartan protocol (`spartan://`, port 300), via the
//! [`spartan-protocol`](https://crates.io/crates/spartan-protocol) crate.
//!
//! Spartan is gemini's plaintext cousin: `<host> <path> <length>\r\n`
//! answered by a single-digit `<code> <meta>\r\n` header (2 success,
//! 3 redirect, 4 client error, 5 server error) and a body. The crate owns
//! the transaction, including the spec's URL mapping (IDN → punycode, a URL
//! query %-decodes into the upload data block); this module maps its
//! response onto errand's [`Response`].

use url::Url;

use crate::{Error, Response, Status};

/// Fetch a `spartan://` URL.
pub(crate) async fn fetch(url: &Url) -> Result<Response, Error> {
    let response = spartan_protocol::fetch(url.as_str(), &Default::default())
        .await
        .map_err(map_error)?;
    Ok(map_response(url, response))
}

fn map_response(url: &Url, response: spartan_protocol::Response) -> Response {
    use spartan_protocol::Status as Spartan;
    let status = match response.status {
        Spartan::Success => Status::Success,
        Spartan::Redirect => Status::Redirect,
        Spartan::ClientError | Spartan::ServerError => Status::Failure,
    };
    Response {
        url: url.clone(),
        status,
        raw_status: Some(response.status.code()),
        meta: response.meta,
        body: response.body,
    }
}

fn map_error(error: spartan_protocol::ClientError) -> Error {
    use spartan_protocol::ClientError as Spartan;
    match error {
        Spartan::BadUrl(message) => Error::BadUrl(message),
        Spartan::Io(message) => Error::Io(message),
        Spartan::Timeout(_) => Error::Timeout,
        Spartan::Protocol(message) => Error::Protocol(message),
        Spartan::BodyTooLarge { max } => {
            Error::Protocol(format!("spartan response exceeds {max} bytes"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Scheme;

    fn u() -> Url {
        Url::parse("spartan://example.org/").unwrap()
    }

    #[test]
    fn success_carries_mime_and_body() {
        let r = map_response(
            &u(),
            spartan_protocol::Response {
                status: spartan_protocol::Status::Success,
                meta: "text/gemini".to_string(),
                body: b"# Hi\n".to_vec(),
            },
        );
        assert_eq!(r.status, Status::Success);
        assert_eq!(r.raw_status, Some(2));
        assert_eq!(r.mime(), Some("text/gemini"));
        assert_eq!(r.body, b"# Hi\n");
    }

    #[test]
    fn redirects_and_errors_map_with_their_digit() {
        let redirect = map_response(
            &u(),
            spartan_protocol::Response {
                status: spartan_protocol::Status::Redirect,
                meta: "/elsewhere".to_string(),
                body: Vec::new(),
            },
        );
        assert_eq!(redirect.status, Status::Redirect);
        assert_eq!(redirect.raw_status, Some(3));
        assert_eq!(redirect.meta, "/elsewhere");

        let client_error = map_response(
            &u(),
            spartan_protocol::Response {
                status: spartan_protocol::Status::ClientError,
                meta: "bad request".to_string(),
                body: Vec::new(),
            },
        );
        assert_eq!(client_error.status, Status::Failure);
        assert_eq!(client_error.raw_status, Some(4));

        let server_error = map_response(
            &u(),
            spartan_protocol::Response {
                status: spartan_protocol::Status::ServerError,
                meta: "boom".to_string(),
                body: Vec::new(),
            },
        );
        assert_eq!(server_error.status, Status::Failure);
        assert_eq!(server_error.raw_status, Some(5));
    }

    #[test]
    fn spartan_scheme_routes_to_port_300() {
        assert_eq!(Scheme::Spartan.default_port(), 300);
    }
}
