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

/// Which of a deployment's origins a request arrived on. `Admin` exists only
/// when the operator configured the graphs page's own name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Reader,
    Docs,
    Admin,
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

/// The origins this deployment answers on: the reader and documents, and
/// optionally the operator's graphs page. `None` is a deployment the operator
/// gave no origins, which answers on loopback alone.
#[derive(Clone, Debug, Default)]
pub struct Origins {
    configured: Option<(Origin, Origin)>,
    admin: Option<Origin>,
}

impl Origins {
    /// A deployment that answers on loopback alone: development, the test
    /// suite, and `librepaper admin serve` with no `origins.app`.
    pub fn loopback_only() -> Origins {
        Origins {
            configured: None,
            admin: None,
        }
    }

    /// Settles the pair an operator asked for. The document origin defaults to
    /// the reader's host behind `docs.`, which is the one DNS record and one
    /// certificate the manual asks for; an operator who wants an unrelated
    /// name says so and it is honored.
    pub fn configure(reader: &str, docs: Option<&str>) -> Result<Origins, String> {
        let reader = Origin::parse(reader).map_err(|error| format!("origins.app: {error}"))?;
        let docs = match docs {
            Some(value) => {
                Origin::parse(value).map_err(|error| format!("origins.docs: {error}"))?
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
            admin: None,
        })
    }

    /// Adds the operator's graphs page, which gets a host of its own. It sits
    /// behind a password the browser keeps per host, so it must not share a
    /// host with the reader's session or with documents, and it needs a
    /// reader origin to be a neighbour of.
    pub fn with_admin(self, admin: Option<&str>) -> Result<Origins, String> {
        let Some(admin) = admin else {
            return Ok(self);
        };
        let admin = Origin::parse(admin).map_err(|error| format!("origins.admin: {error}"))?;
        let Some((reader, docs)) = &self.configured else {
            return Err("origins.admin requires origins.app".into());
        };
        if admin.host() == reader.host() || admin.host() == docs.host() {
            return Err(format!(
                "origins.admin must be a different host from the reader and document origins, but it is {host}.\n\
                 The page is behind a password and must not share a host with the reader's session\n\
                 or with documents. Serve it from its own name, for example admin.{reader_host}: create a\n\
                 DNS record for it pointing at this deployment.",
                host = admin.host(),
                reader_host = reader.host()
            ));
        }
        Ok(Origins {
            admin: Some(admin),
            ..self
        })
    }

    pub fn reader(&self) -> Option<&Origin> {
        self.configured.as_ref().map(|(reader, _)| reader)
    }

    pub fn docs(&self) -> Option<&Origin> {
        self.configured.as_ref().map(|(_, docs)| docs)
    }

    pub fn admin(&self) -> Option<&Origin> {
        self.admin.as_ref()
    }

    /// Which origin a request addressed to `host` is on, or nothing if this
    /// deployment does not answer on that name.
    ///
    /// Loopback is always accepted, whatever the configuration says. The
    /// operator's own status check, a container health check and the test
    /// suite all reach the server that way, and a loopback authority cannot be
    /// used to blur the two origins: a browser sends the authority it was
    /// pointed at, and a document reached over loopback is still served only
    /// under the document side of the pair below.
    pub fn resolve(&self, host: &str) -> Option<Arrival> {
        let authority = normalize(host)?;
        if let Some((reader, docs)) = &self.configured {
            if reader.matches(&authority) {
                return Some(Arrival::settled(
                    Side::Reader,
                    reader.clone(),
                    reader.clone(),
                    docs.clone(),
                ));
            }
            if docs.matches(&authority) {
                return Some(Arrival::settled(
                    Side::Docs,
                    docs.clone(),
                    reader.clone(),
                    docs.clone(),
                ));
            }
            if let Some(admin) = self.admin.as_ref().filter(|admin| admin.matches(&authority)) {
                return Some(Arrival::settled(
                    Side::Admin,
                    admin.clone(),
                    reader.clone(),
                    docs.clone(),
                ));
            }
        }
        self.loopback(&authority)
    }

    /// Whether `host` is one of the configured public names, as asked by a
    /// reverse proxy deciding whether to obtain a certificate for it. Unlike
    /// `resolve`, loopback is not accepted: a certificate is only ever for a
    /// configured public name.
    pub fn ask(&self, host: &str) -> bool {
        let Some(authority) = normalize(host) else {
            return false;
        };
        match &self.configured {
            Some((reader, docs)) => {
                reader.matches(&authority)
                    || docs.matches(&authority)
                    || self
                        .admin
                        .as_ref()
                        .is_some_and(|admin| admin.matches(&authority))
            }
            None => false,
        }
    }

    /// Whether an authority is exactly the configured reader origin. This is
    /// narrower than `resolve`: monitoring agents may need to distinguish the
    /// public application host from the document host and from loopback aliases.
    pub fn is_app_authority(&self, host: &str) -> bool {
        let Some(authority) = normalize(host) else {
            return false;
        };
        self.configured
            .as_ref()
            .is_some_and(|(reader, _)| reader.matches(&authority))
    }

    /// Whether an authority is exactly the configured admin origin. The cost
    /// middleware asks before it has an `Arrival`, to label the request.
    pub fn is_admin_authority(&self, host: &str) -> bool {
        let Some(authority) = normalize(host) else {
            return false;
        };
        self.admin
            .as_ref()
            .is_some_and(|admin| admin.matches(&authority))
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
        let reader = Origin::assemble("http", bare, port);
        let docs = Origin::assemble("http", &format!("{DOCS_PREFIX}{bare}"), port);
        let (side, here) = if name.starts_with(DOCS_PREFIX) {
            (Side::Docs, docs.clone())
        } else {
            (Side::Reader, reader.clone())
        };
        Some(Arrival::settled(side, here, reader, docs))
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

/// What a request tells about where it arrived: which of the deployment's
/// origins it is on, and what the reader and document origins are, settled
/// once so every check below sees the same answer.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub scheme: &'static str,
    side: Side,
    here: Origin,
    reader: Origin,
    docs: Origin,
}

impl Arrival {
    /// `here` is the origin the request arrived on: the reader's, the
    /// documents' or the admin's.
    fn settled(side: Side, here: Origin, reader: Origin, docs: Origin) -> Arrival {
        Arrival {
            scheme: here.scheme,
            side,
            here,
            reader,
            docs,
        }
    }

    pub fn side(&self) -> Side {
        self.side
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
        &self.here.origin
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

    #[test]
    fn a_same_site_document_request_cannot_use_the_readers_cookies() {
        let origins = configured();
        let reader = origins.resolve("paper.example").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("origin", "https://docs.paper.example".parse().unwrap());
        headers.insert("sec-fetch-site", "same-site".parse().unwrap());
        headers.insert(
            "cookie",
            "__Host-librepaper_session=signed-session".parse().unwrap(),
        );
        headers.insert("x-librepaper-client", "web".parse().unwrap());

        // A same-site fetch from docs.paper.example to the reader can carry
        // the reader's SameSite=Lax cookie. Its presence, and the
        // browser-required custom header, do not make that request same-origin
        // with the reader.
        assert!(cross_site_refused(&headers, &reader));
    }

    #[test]
    fn reader_requests_need_the_client_header_but_bearer_clients_keep_working() {
        let reader = configured().resolve("paper.example").unwrap();
        let mut browser = HeaderMap::new();
        browser.insert("origin", "https://paper.example".parse().unwrap());
        browser.insert("sec-fetch-site", "same-origin".parse().unwrap());
        browser.insert(
            "cookie",
            "__Host-librepaper_session=signed-session".parse().unwrap(),
        );
        assert!(cross_site_refused(&browser, &reader));

        browser.insert("x-librepaper-client", "web".parse().unwrap());
        assert!(!cross_site_refused(&browser, &reader));

        let mut cli = HeaderMap::new();
        cli.insert("authorization", "Bearer lp_device-token".parse().unwrap());
        assert!(!cross_site_refused(&cli, &reader));
    }

    #[test]
    fn websocket_origin_must_be_the_reader_origin() {
        let origins = configured();
        let reader = origins.resolve("paper.example").unwrap();
        let mut same_site_document = HeaderMap::new();
        same_site_document.insert("origin", "https://docs.paper.example".parse().unwrap());
        assert!(ws_origin_refused(&same_site_document, &reader));

        let mut same_origin = HeaderMap::new();
        same_origin.insert("origin", "https://paper.example".parse().unwrap());
        assert!(!ws_origin_refused(&same_origin, &reader));

        // Browserless callers do not always send Origin; preserve that
        // compatibility while refusing a browser handshake from docs.
        assert!(!ws_origin_refused(&HeaderMap::new(), &reader));
    }

    #[test]
    fn ask_tells_whether_a_domain_gets_a_certificate() {
        let origins = configured();
        // Both reader and docs origins should be served.
        assert!(origins.ask("paper.example"));
        assert!(origins.ask("docs.paper.example"));
        // Other domains are not served.
        assert!(!origins.ask("other.example"));
        // Loopback names never get certificates.
        assert!(!origins.ask("127.0.0.1"));
        assert!(!origins.ask("localhost"));
        // Empty domain is not served.
        assert!(!origins.ask(""));
        // Loopback deployments serve nothing publicly.
        assert!(!Origins::loopback_only().ask("paper.example"));
    }

    fn with_admin() -> Origins {
        configured()
            .with_admin(Some("https://admin.paper.example"))
            .unwrap()
    }

    #[test]
    fn no_admin_origin_leaves_the_deployment_as_it_was() {
        let origins = configured().with_admin(None).unwrap();
        assert!(origins.admin().is_none());
        assert!(origins.resolve("admin.paper.example").is_none());
        assert_eq!(
            with_admin().admin().unwrap().as_str(),
            "https://admin.paper.example"
        );
    }

    #[test]
    fn an_admin_origin_is_refused_without_a_reader_origin() {
        let error = Origins::loopback_only()
            .with_admin(Some("http://admin.localhost:8080"))
            .expect_err("a loopback-only deployment has no neighbour for the admin page");
        assert_eq!(error, "origins.admin requires origins.app");
    }

    #[test]
    fn an_admin_origin_must_be_an_origin_with_a_host_of_its_own() {
        let error = configured()
            .with_admin(Some("admin.paper.example"))
            .expect_err("a bare host is not an origin");
        assert!(error.starts_with("origins.admin: "), "{error}");

        let error = configured()
            .with_admin(Some("https://paper.example"))
            .expect_err("the reader's host is taken");
        assert!(error.contains("origins.admin"), "{error}");
        assert!(error.contains("different host"), "{error}");

        let error = configured()
            .with_admin(Some("https://docs.paper.example"))
            .expect_err("the default document host is taken");
        assert!(error.contains("different host"), "{error}");
        assert!(error.contains("admin.paper.example"), "{error}");

        let custom_docs =
            Origins::configure("https://paper.example", Some("https://sandbox.example")).unwrap();
        let error = custom_docs
            .with_admin(Some("https://sandbox.example"))
            .expect_err("a configured document host is taken");
        assert!(error.contains("different host"), "{error}");
    }

    #[test]
    fn the_admin_host_resolves_to_a_side_of_its_own() {
        let origins = with_admin();
        for host in [
            "admin.paper.example",
            "ADMIN.PAPER.EXAMPLE",
            "admin.paper.example:443",
        ] {
            let arrival = origins.resolve(host).unwrap();
            assert_eq!(arrival.side(), Side::Admin, "{host:?}");
            assert!(!arrival.is_docs_host(), "{host:?}");
            assert!(arrival.is_https());
            assert_eq!(arrival.reader_origin(), "https://paper.example");
            assert_eq!(arrival.docs_origin(), "https://docs.paper.example");
            assert_eq!(
                arrival.callback_url(),
                "https://admin.paper.example/auth/callback"
            );
        }
        assert_eq!(
            origins.resolve("paper.example").unwrap().side(),
            Side::Reader
        );
        assert_eq!(
            origins.resolve("docs.paper.example").unwrap().side(),
            Side::Docs
        );
        for host in ["admin.paper.example:8443", "admin.paper.example.evil.example"] {
            assert!(origins.resolve(host).is_none(), "{host:?}");
        }
    }

    #[test]
    fn the_admin_name_gets_a_certificate_and_is_its_own_authority() {
        let origins = with_admin();
        assert!(origins.ask("admin.paper.example"));
        assert!(origins.ask("paper.example"));
        assert!(!origins.ask("other.example"));
        assert!(!configured().ask("admin.paper.example"));

        assert!(origins.is_admin_authority("admin.paper.example"));
        assert!(origins.is_admin_authority("admin.paper.example:443"));
        for host in [
            "paper.example",
            "docs.paper.example",
            "localhost:8080",
            "evil.example",
            "",
        ] {
            assert!(!origins.is_admin_authority(host), "{host:?}");
        }
        assert!(!configured().is_admin_authority("admin.paper.example"));
        // The admin name is not the app name either.
        assert!(!origins.is_app_authority("admin.paper.example"));
    }

    #[test]
    fn a_loopback_admin_origin_is_answered_next_to_its_loopback_reader() {
        let origins = Origins::configure("http://localhost:8080", None)
            .unwrap()
            .with_admin(Some("http://admin.localhost:8080"))
            .unwrap();
        let arrival = origins.resolve("admin.localhost:8080").unwrap();
        assert_eq!(arrival.side(), Side::Admin);
        assert!(!arrival.is_https());
        assert_eq!(
            origins.resolve("localhost:8080").unwrap().side(),
            Side::Reader
        );
    }

    #[test]
    fn app_authority_accepts_only_the_configured_reader_authority() {
        let origins = configured();
        assert!(origins.is_app_authority("paper.example"));
        assert!(origins.is_app_authority("paper.example:443"));
        for host in [
            "docs.paper.example",
            "docs.sandbox.example",
            "sandbox.example",
            "evil.example",
            "localhost:8080",
            "127.0.0.1:8080",
            "",
        ] {
            assert!(
                !origins.is_app_authority(host),
                "{host:?} is not the app host"
            );
        }
        assert!(!Origins::loopback_only().is_app_authority("localhost:8080"));

        let custom_docs =
            Origins::configure("https://paper.example", Some("https://sandbox.example")).unwrap();
        assert!(!custom_docs.is_app_authority("sandbox.example"));
    }
}
