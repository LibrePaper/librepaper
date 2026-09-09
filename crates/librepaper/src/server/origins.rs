//! Documents are served from a different hostname than the reader, so an
//! uploaded file is a stranger to the page framing it. The browser then refuses
//! it any access to the reader's DOM or its session, which is what lets the
//! document run its own scripts safely: charts, maps, anything.
//!
//! A different port would not do. Cookies ignore ports, so a document on
//! another port could still make requests carrying the reader's session. It
//! has to be a different host.
//!
//! The names are derived rather than configured: whatever host the reader is
//! on, documents live on "docs." in front of it. On a real domain that is one
//! DNS record and one certificate; in development, browsers resolve anything
//! ending in .localhost by themselves, so it needs no setup at all.

use axum::http::HeaderMap;

pub const DOCS_PREFIX: &str = "docs.";

/// What a request tells about where it arrived: the host it was addressed to
/// and the scheme it arrived over, read once from the headers so every check
/// below sees the same answer.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub host: String,
    pub scheme: &'static str,
}

impl Arrival {
    /// Forwarded TLS metadata is trusted only from a local proxy peer.
    pub fn from_peer(headers: &HeaderMap, peer: std::net::IpAddr) -> Arrival {
        let mut arrival = Self::from_headers(headers);
        if !super::local_peer(peer) {
            arrival.scheme = "http";
        }
        arrival
    }

    pub fn from_headers(headers: &HeaderMap) -> Arrival {
        let host = header(headers, "host").unwrap_or_default();
        let scheme = if header(headers, "x-forwarded-proto").as_deref() == Some("https") {
            "https"
        } else {
            "http"
        };
        Arrival { host, scheme }
    }

    /// Whether this request arrived on the document hostname.
    pub fn is_docs_host(&self) -> bool {
        self.host.to_lowercase().starts_with(DOCS_PREFIX)
    }

    /// The origin the reader frames documents from, and the only origin it
    /// accepts postMessage traffic from.
    pub fn docs_origin(&self) -> String {
        format!("{}://{}", self.scheme, docs_host(&self.host))
    }

    pub fn reader_origin(&self) -> String {
        format!("{}://{}", self.scheme, reader_host(&self.host))
    }

    pub fn is_https(&self) -> bool {
        self.scheme == "https"
    }

    pub fn callback_url(&self) -> String {
        format!("{}://{}/auth/callback", self.scheme, self.host)
    }

    /// Google's own callback. GitHub keeps the bare `/auth/callback` so that
    /// no OAuth app already registered against it has to be edited.
    pub fn google_callback_url(&self) -> String {
        format!("{}://{}/auth/callback/google", self.scheme, self.host)
    }
}

pub fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Where documents for this deployment live.
pub fn docs_host(host: &str) -> String {
    if host.to_lowercase().starts_with(DOCS_PREFIX) {
        host.to_string()
    } else {
        format!("{DOCS_PREFIX}{host}")
    }
}

/// The inverse: the reader that owns a document hostname.
pub fn reader_host(host: &str) -> String {
    host.strip_prefix(DOCS_PREFIX).unwrap_or(host).to_string()
}

/// Rule A, for a state-changing route a browser can reach with cookies
/// attached: docs.<host> is same-site with the reader, so SameSite cookies
/// alone do not stop a hostile document from posting here. A bearer token
/// (the CLI) skips all of this -- it is never attached to a request
/// automatically, so a hostile page cannot forge one. Otherwise all three must
/// hold: any Origin header sent must be this reader's own origin, any
/// Sec-Fetch-Site header must say the request was not cross-site, and a custom
/// header must be present, which a browser cannot attach to a cross-origin
/// request without a CORS preflight that is never granted.
pub fn cross_site_refused(headers: &HeaderMap, arrival: &Arrival) -> bool {
    if header(headers, "authorization").is_some_and(|value| value.starts_with("Bearer ")) {
        return false;
    }
    if let Some(origin) = header(headers, "origin") {
        if !origin.is_empty() && origin != arrival.reader_origin() {
            return true;
        }
    }
    if let Some(site) = header(headers, "sec-fetch-site") {
        if !site.is_empty() && site != "same-origin" && site != "none" {
            return true;
        }
    }
    header(headers, "x-librepaper-client").is_none_or(|value| value.is_empty())
}

/// Rule A's WebSocket variant: browsers always send Origin on a WebSocket
/// handshake and cannot be made to skip it or to attach a custom header, so
/// the custom-header check does not apply here -- an absent Origin is not
/// itself suspicious, but a foreign one is refused.
pub fn ws_origin_refused(headers: &HeaderMap, arrival: &Arrival) -> bool {
    match header(headers, "origin") {
        Some(origin) => !origin.is_empty() && origin != arrival.reader_origin(),
        None => false,
    }
}

/// The JSON body every refusal under rule A answers with.
pub fn cross_site_refusal() -> serde_json::Value {
    serde_json::json!({"error": "cross-site request refused"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forwarded_scheme_requires_a_trusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "reader.example".parse().unwrap());
        headers.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(!Arrival::from_peer(&headers, "203.0.113.1".parse().unwrap()).is_https());
        assert!(Arrival::from_peer(&headers, "127.0.0.1".parse().unwrap()).is_https());
    }
}
