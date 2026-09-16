//! Documents are served from a different hostname than the reader, so an
//! uploaded file is a stranger to the page framing it. The browser then refuses
//! it any access to the reader's DOM or its session, which is what lets the
//! document run its own scripts safely: charts, maps, anything.
//!
//! A different port would not do. Cookies ignore ports, so a document on
//! another port could still make requests carrying the reader's session. It
//! has to be a different host.
//!
//! Which two hostnames those are is settled at startup rather than read off
//! each request. A deployment states its reader origin and its document
//! origin; a request addressed to anything else is refused rather than
//! answered on a guess. That matters because the alternative -- believing the
//! `Host` header -- makes the boundary a property of whatever a proxy or a
//! visitor put in a header, and the browser boundary is the scheme/host/port
//! origin tuple, which DNS alone does not prove.
//!
//! A deployment given no origins answers on loopback alone. That is what
//! development and the test suite use, and it keeps `docs.localhost` working
//! with no setup, since browsers resolve anything ending in .localhost by
//! themselves.

use axum::http::HeaderMap;

pub const DOCS_PREFIX: &str = "docs.";

/// Which of a deployment's two origins a request arrived on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Reader,
    Docs,
}

/// One origin, kept as its parts and as the string a browser would compare
/// against, worked out once so no request has to build it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Origin {
    scheme: &'static str,
    authority: String,
    origin: String,
}

impl Origin {
    /// Parses an operator-supplied origin. Anything a browser would not treat
    /// as an origin is refused here rather than misunderstood later, so a typo
    /// is a startup error in front of the person who made it.
    pub fn parse(value: &str) -> Result<Origin, String> {
        let value = value.trim();
        if value.is_empty() {
            return Err("an origin cannot be empty".into());
        }
        let parsed = url::Url::parse(value)
            .map_err(|_| format!("{value:?} is not a URL; write it as https://paper.example"))?;
        let scheme = match parsed.scheme() {
            "https" => "https",
            "http" => "http",
            other => {
                return Err(format!(
                    "an origin must be http: or https:, not {other}: (got {value:?})"
                ))
            }
        };
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || !matches!(parsed.path(), "" | "/")
        {
            return Err(format!(
                "an origin is a scheme and a host only, with no path, query, fragment or credentials (got {value:?})"
            ));
        }
        let host = parsed
            .host_str()
            .ok_or_else(|| format!("{value:?} names no host"))?
            .to_lowercase();
        Ok(Origin::assemble(scheme, &host, parsed.port()))
    }

    fn assemble(scheme: &'static str, host: &str, port: Option<u16>) -> Origin {
        let authority = match port {
            Some(port) if !default_port(scheme, port) => format!("{host}:{port}"),
            _ => host.to_string(),
        };
        Origin {
            scheme,
            origin: format!("{scheme}://{authority}"),
            authority,
        }
    }

    pub fn host(&self) -> &str {
        match self.authority.rsplit_once(':') {
            Some((host, _)) => host,
            None => &self.authority,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.origin
    }

    /// Whether a request's authority names this origin. A browser leaves the
    /// default port off and a proxy sometimes puts it back, and those are the
    /// same origin written two ways.
    fn matches(&self, authority: &str) -> bool {
        if authority == self.authority {
            return true;
        }
        match authority.strip_prefix(self.authority.as_str()) {
            Some(":443") => self.scheme == "https",
            Some(":80") => self.scheme == "http",
            _ => false,
        }
    }
}

fn default_port(scheme: &str, port: u16) -> bool {
    (scheme == "https" && port == 443) || (scheme == "http" && port == 80)
}

/// The two origins this deployment answers on. `None` is a deployment the
/// operator gave no origins, which answers on loopback alone.
#[derive(Clone, Debug, Default)]
pub struct Origins {
    configured: Option<(Origin, Origin)>,
}

impl Origins {
    /// A deployment that answers on loopback alone: development, the test
    /// suite, and `librepaper admin serve` with no `--origin`.
    pub fn loopback_only() -> Origins {
        Origins { configured: None }
    }

    /// Settles the pair an operator asked for. The document origin defaults to
    /// the reader's host behind `docs.`, which is the one DNS record and one
    /// certificate the manual asks for; an operator who wants an unrelated
    /// name says so and it is honored.
    pub fn configure(reader: &str, docs: Option<&str>) -> Result<Origins, String> {
        let reader = Origin::parse(reader).map_err(|error| format!("--origin: {error}"))?;
        let docs = match docs {
            Some(value) => {
                Origin::parse(value).map_err(|error| format!("--docs-origin: {error}"))?
            }
            None => Origin::assemble(
                reader.scheme,
                &format!("{DOCS_PREFIX}{}", reader.host()),
                port_of(&reader.authority),
            ),
        };
        if reader.host() == docs.host() {
            return Err(format!(
                "the reader origin and the document origin must be different hosts, but both are {host}.\n\
                 A document is hostile code, and what stops it reaching the reader's session is that\n\
                 the browser sees a different host. Serve documents from {DOCS_PREFIX}{host}: create a\n\
                 DNS record for it pointing at this deployment, and a certificate covering it.",
                host = reader.host()
            ));
        }
        Ok(Origins {
            configured: Some((reader, docs)),
        })
    }

    pub fn reader(&self) -> Option<&Origin> {
        self.configured.as_ref().map(|(reader, _)| reader)
    }

    pub fn docs(&self) -> Option<&Origin> {
        self.configured.as_ref().map(|(_, docs)| docs)
    }

    /// Which origin a request addressed to `host` is on, or nothing if this
    /// deployment does not answer on that name.
    ///
    /// Loopback is always accepted, whatever the configuration says. The
    /// operator's own `admin status`, a container health check and the test
    /// suite all reach the server that way, and a loopback authority cannot be
    /// used to blur the two origins: a browser sends the authority it was
    /// pointed at, and a document reached over loopback is still served only
    /// under the document side of the pair below.
    pub fn resolve(&self, host: &str) -> Option<Arrival> {
        let authority = normalize(host)?;
        if let Some((reader, docs)) = &self.configured {
            if reader.matches(&authority) {
                return Some(Arrival::settled(Side::Reader, reader.clone(), docs.clone()));
            }
            if docs.matches(&authority) {
                return Some(Arrival::settled(Side::Docs, reader.clone(), docs.clone()));
            }
        }
        self.loopback(&authority)
    }

    /// A loopback request stands on its own origin pair, derived from the name
    /// it arrived on, so the reader that answers it agrees with the address in
    /// the browser's bar rather than with a public name it cannot reach.
    fn loopback(&self, authority: &str) -> Option<Arrival> {
        let (name, port) = split_authority(authority);
        let bare = name.strip_prefix(DOCS_PREFIX).unwrap_or(name);
        if !is_loopback_name(bare) {
            return None;
        }
        let side = if name.starts_with(DOCS_PREFIX) {
            Side::Docs
        } else {
            Side::Reader
        };
        let reader = Origin::assemble("http", bare, port);
        let docs = Origin::assemble("http", &format!("{DOCS_PREFIX}{bare}"), port);
        Some(Arrival::settled(side, reader, docs))
    }
}

fn port_of(authority: &str) -> Option<u16> {
    authority.rsplit_once(':').and_then(|(_, p)| p.parse().ok())
}

fn split_authority(authority: &str) -> (&str, Option<u16>) {
    match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse() {
            Ok(port) => (host, Some(port)),
            Err(_) => (authority, None),
        },
        None => (authority, None),
    }
}

/// An IPv6 authority is bracketed, so the last colon is part of the address
/// rather than a port separator unless it follows the closing bracket.
fn normalize(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty() || host.len() > 255 || host.contains('/') || host.contains('@') {
        return None;
    }
    let lowered = host.to_lowercase();
    let (name, port) = if lowered.starts_with('[') {
        match lowered.split_once("]:") {
            Some((name, port)) => (format!("{name}]"), port.parse::<u16>().ok()?),
            None => return Some(lowered),
        }
    } else {
        match lowered.rsplit_once(':') {
            Some((name, port)) => (name.to_string(), port.parse::<u16>().ok()?),
            None => return Some(lowered),
        }
    };
    if name.is_empty() {
        return None;
    }
    Some(format!("{name}:{port}"))
}

fn is_loopback_name(name: &str) -> bool {
    let name = name.trim_start_matches('[').trim_end_matches(']');
    name == "localhost"
        || name == "::1"
        || name
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// What a request tells about where it arrived: which of the deployment's two
/// origins it is on, and what those origins are, settled once so every check
/// below sees the same answer.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub scheme: &'static str,
    side: Side,
    reader: Origin,
    docs: Origin,
}

impl Arrival {
    fn settled(side: Side, reader: Origin, docs: Origin) -> Arrival {
        let here = match side {
            Side::Reader => &reader,
            Side::Docs => &docs,
        };
        Arrival {
            scheme: here.scheme,
            side,
            reader,
            docs,
        }
    }

    /// Whether this request arrived on the document origin. Decided when the
    /// host was matched against the configured pair, not by reading the name
    /// again: an operator may serve documents from a name that does not begin
    /// with `docs.`, and the answer has to be the same either way.
    pub fn is_docs_host(&self) -> bool {
        self.side == Side::Docs
    }

    /// The origin the reader frames documents from, and the only origin it
    /// accepts postMessage traffic from.
    pub fn docs_origin(&self) -> String {
        self.docs.origin.clone()
    }

    pub fn reader_origin(&self) -> String {
        self.reader.origin.clone()
    }

    pub fn is_https(&self) -> bool {
        self.scheme == "https"
    }

    /// Built from the validated origin rather than from the `Host` header, so
    /// a request cannot name its own OAuth redirect.
    pub fn callback_url(&self) -> String {
        format!("{}/auth/callback", self.here())
    }

    /// Google's own callback. GitHub keeps the bare `/auth/callback` so that
    /// no OAuth app already registered against it has to be edited.
    pub fn google_callback_url(&self) -> String {
        format!("{}/auth/callback/google", self.here())
    }

    fn here(&self) -> &str {
        match self.side {
            Side::Reader => &self.reader.origin,
            Side::Docs => &self.docs.origin,
        }
    }
}

pub fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
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

    fn configured() -> Origins {
        Origins::configure("https://paper.example", None).unwrap()
    }

    #[test]
    fn the_document_origin_defaults_to_the_reader_behind_the_prefix() {
        let origins = configured();
        assert_eq!(origins.reader().unwrap().as_str(), "https://paper.example");
        assert_eq!(
            origins.docs().unwrap().as_str(),
            "https://docs.paper.example"
        );
    }

    #[test]
    fn one_host_for_both_origins_is_refused_with_the_record_to_create() {
        let error = Origins::configure("https://paper.example", Some("https://paper.example"))
            .expect_err("a single origin is not a supported configuration");
        assert!(error.contains("must be different hosts"), "{error}");
        assert!(error.contains("docs.paper.example"), "{error}");
    }

    #[test]
    fn an_origin_is_a_scheme_and_a_host_only() {
        for value in [
            "paper.example",
            "ftp://paper.example",
            "https://paper.example/reader",
            "https://user:pw@paper.example",
            "https://paper.example?x=1",
        ] {
            assert!(Origin::parse(value).is_err(), "{value} should be refused");
        }
    }

    #[test]
    fn a_configured_deployment_answers_on_its_two_origins_and_refuses_the_rest() {
        let origins = configured();
        assert!(!origins.resolve("paper.example").unwrap().is_docs_host());
        assert!(origins
            .resolve("docs.paper.example")
            .unwrap()
            .is_docs_host());
        // The default port is the same authority written the long way.
        assert!(origins.resolve("paper.example:443").is_some());
        for host in [
            "evil.example",
            "paper.example.evil.example",
            "paper.example:8443",
            "",
        ] {
            assert!(
                origins.resolve(host).is_none(),
                "{host:?} should not be answered"
            );
        }
    }

    /// Published documents are reachable only under the document side of the
    /// pair, so the guarantee that hostile document HTML never reaches the
    /// reader origin reduces to this: nothing but the configured document
    /// origin ever resolves to `Side::Docs`.
    #[test]
    fn nothing_but_the_document_origin_is_ever_the_document_side() {
        let origins = configured();
        for host in [
            "paper.example",
            "paper.example:443",
            "PAPER.EXAMPLE",
            "docs.paper.example.evil.example",
            "evil.example",
            "docs.evil.example",
            "127.0.0.1:8080",
            "localhost",
        ] {
            assert!(
                !origins
                    .resolve(host)
                    .is_some_and(|arrival| arrival.is_docs_host()),
                "{host:?} must not be served as the document origin"
            );
        }
        for host in [
            "docs.paper.example",
            "DOCS.PAPER.EXAMPLE",
            "docs.paper.example:443",
        ] {
            assert!(
                origins.resolve(host).unwrap().is_docs_host(),
                "{host:?} is the document origin"
            );
        }
    }

    #[test]
    fn a_refused_host_cannot_name_its_own_redirect_or_frame_origin() {
        let origins = configured();
        let arrival = origins.resolve("paper.example").unwrap();
        assert_eq!(
            arrival.callback_url(),
            "https://paper.example/auth/callback"
        );
        assert_eq!(arrival.docs_origin(), "https://docs.paper.example");
        assert!(arrival.is_https());
        assert!(origins.resolve("attacker.example").is_none());
    }

    #[test]
    fn loopback_is_answered_whatever_the_configuration_says() {
        for origins in [configured(), Origins::loopback_only()] {
            let arrival = origins.resolve("127.0.0.1:8080").unwrap();
            assert!(!arrival.is_docs_host());
            assert_eq!(arrival.reader_origin(), "http://127.0.0.1:8080");
            assert_eq!(arrival.docs_origin(), "http://docs.127.0.0.1:8080");
            assert!(!arrival.is_https());
            assert!(origins
                .resolve("docs.localhost:8080")
                .unwrap()
                .is_docs_host());
            assert_eq!(
                origins.resolve("localhost").unwrap().reader_origin(),
                "http://localhost"
            );
        }
    }

    #[test]
    fn a_deployment_without_origins_answers_on_nothing_else() {
        let origins = Origins::loopback_only();
        for host in ["paper.example", "docs.paper.example", "10.0.0.5"] {
            assert!(
                origins.resolve(host).is_none(),
                "{host:?} should not be answered"
            );
        }
    }

    #[test]
    fn a_document_origin_need_not_begin_with_the_prefix() {
        let origins =
            Origins::configure("https://paper.example", Some("https://sandbox.example")).unwrap();
        let arrival = origins.resolve("sandbox.example").unwrap();
        assert!(arrival.is_docs_host());
        assert_eq!(arrival.reader_origin(), "https://paper.example");
    }

    #[test]
    fn a_scheme_is_the_deployments_own_rather_than_a_forwarded_header() {
        let origins = Origins::configure("https://paper.example", None).unwrap();
        assert!(origins.resolve("paper.example").unwrap().is_https());
        let plain = Origins::configure("http://paper.example", None).unwrap();
        assert!(!plain.resolve("paper.example").unwrap().is_https());
    }
}
