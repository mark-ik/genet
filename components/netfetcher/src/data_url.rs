/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! `data:` URL processing (WHATWG) with a minimal MIME-type parser/serializer.
//!
//! `data:[<mediatype>][;base64],<data>`: the data is percent-decoded, then
//! forgiving-base64-decoded when `;base64`. The media type is parsed and
//! re-serialized (lowercased type/subtype/parameter names, canonical form),
//! defaulting to `text/plain;charset=US-ASCII` when absent or unparseable.

use bytes::Bytes;
use url::Url;

use crate::response::{Response, ResponseBody, ResponseType};

const DEFAULT_MIME: &str = "text/plain;charset=US-ASCII";

/// Process a `data:` URL into a basic 200 response (or a network error).
pub(crate) fn process(url: &Url, url_list: Vec<Url>) -> Response {
    // Serialize the URL without its fragment, then strip the "data:" scheme.
    let mut u = url.clone();
    u.set_fragment(None);
    let serialized = u.as_str();
    let Some(rest) = serialized.strip_prefix("data:") else {
        return Response::network_error();
    };
    let Some(comma) = rest.find(',') else {
        return Response::network_error();
    };
    // Strip leading and trailing ASCII whitespace from the MIME string (data:
    // URL processor: "remove leading and trailing ASCII whitespace").
    let mut meta = rest[..comma].trim_matches(is_http_ws).to_owned();
    let body = percent_decode(&rest[comma + 1..]);

    // A trailing ";" + zero-or-more U+0020 SPACE + "base64" (ASCII
    // case-insensitive) base64-decodes the body.
    let body = if let Some(stripped) = strip_base64_suffix(&meta) {
        meta = stripped;
        match forgiving_base64(&body) {
            Some(b) => b,
            None => return Response::network_error(),
        }
    } else {
        body
    };

    if meta.starts_with(';') {
        meta = format!("text/plain{meta}");
    }
    let mime = parse_mime(&meta).unwrap_or_else(|| DEFAULT_MIME.to_owned());

    Response {
        status: 200,
        headers: vec![("content-type".to_owned(), mime)],
        body: ResponseBody::from_bytes(Bytes::from(body)),
        url_list,
        response_type: ResponseType::Basic,
    }
}

fn is_http_ws(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

/// If `meta` ends with U+003B (;) + zero-or-more U+0020 SPACE + "base64" (ASCII
/// case-insensitive), return `meta` with that whole suffix removed (the spaces
/// and the `;` too). Otherwise `None`. (data: URL processor base64 step.)
fn strip_base64_suffix(meta: &str) -> Option<String> {
    const B64: &[u8] = b"base64";
    let b = meta.as_bytes();
    if b.len() < B64.len() || !b[b.len() - B64.len()..].eq_ignore_ascii_case(B64) {
        return None;
    }
    let mut end = b.len() - B64.len();
    while end > 0 && b[end - 1] == b' ' {
        end -= 1;
    }
    if end == 0 || b[end - 1] != b';' {
        return None;
    }
    Some(meta[..end - 1].to_owned())
}

fn is_ascii_ws(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.' | b'^' | b'_'
                | b'`' | b'|' | b'~'
        )
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(is_token_byte)
}

/// An HTTP quoted-string token code point: tab, or 0x20..=0x7E, or 0x80..=0xFF.
fn is_quoted_value(s: &str) -> bool {
    s.bytes().all(|b| b == 0x09 || (0x20..=0x7E).contains(&b) || b >= 0x80)
}

/// Parse + re-serialize a MIME type (MIME Sniffing "parse a MIME type" +
/// "serialize a MIME type"). `None` on an invalid type/subtype.
fn parse_mime(input: &str) -> Option<String> {
    let s = input.trim_matches(is_http_ws);
    let b = s.as_bytes();
    let mut i = 0;

    let type_start = i;
    while i < b.len() && b[i] != b'/' {
        i += 1;
    }
    if i >= b.len() {
        return None; // no '/'
    }
    let typ = &s[type_start..i];
    if !is_token(typ) {
        return None;
    }
    i += 1; // skip '/'

    let sub_start = i;
    while i < b.len() && b[i] != b';' {
        i += 1;
    }
    let subtype = s[sub_start..i].trim_end_matches(is_http_ws);
    if !is_token(subtype) {
        return None;
    }

    let mut out = format!("{}/{}", typ.to_ascii_lowercase(), subtype.to_ascii_lowercase());
    let mut seen: Vec<String> = Vec::new();

    while i < b.len() {
        i += 1; // skip ';'
        while i < b.len() && is_ascii_ws(b[i]) {
            i += 1;
        }
        let name_start = i;
        while i < b.len() && b[i] != b';' && b[i] != b'=' {
            i += 1;
        }
        let name = s[name_start..i].to_ascii_lowercase();
        if i >= b.len() || b[i] == b';' {
            continue; // no value
        }
        i += 1; // skip '='

        let (value, quoted) = if i < b.len() && b[i] == b'"' {
            i += 1;
            let mut v = String::new();
            while i < b.len() {
                if b[i] == b'\\' && i + 1 < b.len() {
                    v.push(b[i + 1] as char);
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                v.push(b[i] as char);
                i += 1;
            }
            while i < b.len() && b[i] != b';' {
                i += 1;
            }
            (v, true)
        } else {
            let val_start = i;
            while i < b.len() && b[i] != b';' {
                i += 1;
            }
            (s[val_start..i].trim_end_matches(is_http_ws).to_owned(), false)
        };

        // An unquoted empty value is dropped; a quoted empty value (`a=""`) is kept.
        if is_token(&name)
            && !seen.contains(&name)
            && is_quoted_value(&value)
            && (quoted || !value.is_empty())
        {
            seen.push(name.clone());
            out.push(';');
            out.push_str(&name);
            out.push('=');
            if is_token(&value) {
                out.push_str(&value);
            } else {
                out.push('"');
                out.push_str(&value.replace('\\', "\\\\").replace('"', "\\\""));
                out.push('"');
            }
        }
    }
    Some(out)
}

/// Percent-decode a string to bytes (`%XX` → byte; other bytes pass through).
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// WHATWG forgiving-base64 decode: strip ASCII whitespace, remove up to 2
/// trailing `=` only when the length is a multiple of 4, reject `len % 4 == 1`
/// and any non-alphabet byte, then decode.
fn forgiving_base64(data: &[u8]) -> Option<Vec<u8>> {
    use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
    use base64::Engine;
    let mut s: Vec<u8> = data.iter().copied().filter(|&b| !is_ascii_ws(b)).collect();
    if s.len() % 4 == 0 {
        if s.ends_with(b"==") {
            s.truncate(s.len() - 2);
        } else if s.ends_with(b"=") {
            s.truncate(s.len() - 1);
        }
    }
    if s.len() % 4 == 1 {
        return None;
    }
    if !s.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/') {
        return None;
    }
    // Forgiving-base64 keeps the leading bytes even when a final group has
    // non-zero trailing bits (e.g. "ab" -> 1 byte), and ignores padding (already
    // stripped above).
    let cfg = GeneralPurposeConfig::new()
        .with_decode_allow_trailing_bits(true)
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
    GeneralPurpose::new(&base64::alphabet::STANDARD, cfg).decode(&s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mime(input: &str) -> String {
        if input.starts_with(';') {
            parse_mime(&format!("text/plain{input}")).unwrap_or_else(|| DEFAULT_MIME.to_owned())
        } else {
            parse_mime(input).unwrap_or_else(|| DEFAULT_MIME.to_owned())
        }
    }

    #[test]
    fn mime_lowercasing_and_defaults() {
        assert_eq!(mime("IMAGE/gif"), "image/gif");
        assert_eq!(mime("IMAGE/gif;CHARSET=x"), "image/gif;charset=x");
        assert_eq!(mime("text/plain;Charset=UTF-8"), "text/plain;charset=UTF-8");
        assert_eq!(mime("text/plain;"), "text/plain");
        assert_eq!(mime("text/plain%0C"), "text/plain%0c");
        // empty *quoted* value is kept (an unterminated quote yields `a=""`)
        assert_eq!(mime("text/plain;a=\""), "text/plain;a=\"\"");
        // empty *unquoted* value is dropped
        assert_eq!(mime("text/plain;a="), "text/plain");
        // invalid → default
        assert_eq!(mime("//test/"), DEFAULT_MIME);
        assert_eq!(mime("%20"), DEFAULT_MIME);
    }

    #[test]
    fn base64_suffix_detection() {
        assert_eq!(strip_base64_suffix("text/plain;base64"), Some("text/plain".to_owned()));
        // zero-or-more U+0020 SPACE may sit between ';' and "base64"
        assert_eq!(strip_base64_suffix("; base64"), Some(String::new()));
        assert_eq!(strip_base64_suffix(";  base64"), Some(String::new()));
        assert_eq!(strip_base64_suffix(";BASE64"), Some(String::new())); // case-insensitive
        assert_eq!(strip_base64_suffix("text/plain"), None);
        assert_eq!(strip_base64_suffix("base64"), None); // missing ';'
    }

    #[test]
    fn forgiving_base64_padding_rules() {
        assert_eq!(forgiving_base64(b"YWJj"), Some(b"abc".to_vec()));
        assert!(forgiving_base64(b"ab").is_some()); // 1 byte, valid
        assert!(forgiving_base64(b"=").is_none()); // len%4==1
        assert!(forgiving_base64(b"==").is_none()); // stray '='
        assert!(forgiving_base64(b"abcd===").is_none()); // excess padding
        assert!(forgiving_base64(b" abcd=== ").is_none());
    }
}
