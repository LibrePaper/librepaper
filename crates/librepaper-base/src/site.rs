//! The address of a deployment. A deployment is an origin: scheme, host and
//! port, with no path. `SiteUrl` is the one parse and the one canonical form
//! for such an address, so two spellings of the same server key the same
//! stored credentials.

use std::fmt;
use std::str::FromStr;

use url::Url;

/// A deployment's origin in canonical form: the scheme, the lowercase host,
/// and the port only when it is not the scheme's default.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SiteUrl {
    text: String,
}

impl SiteUrl {
    /// The canonical origin text, with no trailing slash.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// `path` appended to the origin. `path` should start with `/`.
    pub fn join(&self, path: &str) -> String {
        format!("{}{path}", self.text)
    }

    /// The canonical origin of a URL that has already been parsed: scheme,
    /// lowercase host, and a port that is not the default. Nothing is
    /// validated and the path is ignored.
    pub fn origin_of(url: &Url) -> String {
        let scheme = url.scheme().to_ascii_lowercase();
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        match url.port() {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        }
    }

    /// The origin of any URL, as a `SiteUrl`. The path, query and fragment
    /// are dropped; the scheme and host rules of `FromStr` still apply.
    pub fn from_origin_of(url: &Url) -> Result<SiteUrl, String> {
        Self::checked(url, &Self::origin_of(url))
    }

    fn checked(url: &Url, text: &str) -> Result<SiteUrl, String> {
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err("the server must be an http(s) address with a host".into());
        }
        if url.scheme() == "http" && !is_loopback(url) {
            return Err("plain http is allowed only for localhost; use https".into());
        }
        Ok(SiteUrl {
            text: text.to_string(),
        })
    }
}

/// Whether `url` names this computer: `localhost` or a loopback address. Plain
/// http is acceptable to a server or page only when this holds.
pub fn is_loopback(url: &Url) -> bool {
    url.host().is_some_and(|host| match host {
        url::Host::Domain(domain) => domain == "localhost",
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    })
}

impl FromStr for SiteUrl {
    type Err = String;

    fn from_str(raw: &str) -> Result<SiteUrl, String> {
        let raw = raw.trim();
        let url = Url::parse(raw).map_err(|error| format!("{raw:?} is not a URL: {error}"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("the server address must not contain credentials".into());
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err("the server address must not have a query or fragment".into());
        }
        if url.path() != "/" {
            return Err(
                "the server address must be an origin such as https://app.librepaper.org, with no path"
                    .into(),
            );
        }
        Self::checked(&url, &Self::origin_of(&url))
    }
}

impl fmt::Display for SiteUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(raw: &str) -> String {
        raw.parse::<SiteUrl>().unwrap().to_string()
    }

    #[test]
    fn accepted_spellings_have_one_canonical_form() {
        for (raw, expected) in [
            ("https://app.librepaper.org", "https://app.librepaper.org"),
            ("https://app.librepaper.org/", "https://app.librepaper.org"),
            ("  HTTPS://LibrePaper.Example:5173/ ", "https://librepaper.example:5173"),
            ("https://x.example:443", "https://x.example"),
            ("http://localhost:3000/", "http://localhost:3000"),
            ("http://localhost:80", "http://localhost"),
            ("http://127.0.0.1:8179", "http://127.0.0.1:8179"),
            ("http://[::1]:3000/", "http://[::1]:3000"),
        ] {
            assert_eq!(canonical(raw), expected, "{raw}");
        }
    }

    #[test]
    fn refused_spellings() {
        for raw in [
            "",
            "app.librepaper.org",
            "ftp://app.librepaper.org",
            "http://app.example",
            "https://user@app.example",
            "https://user:pw@app.example",
            "https://app.example/#frag",
            "https://app.example/?x=1",
            "https://app.example?x=1",
            "https://app.example/sub",
            "https://app.example/sub/",
        ] {
            assert!(raw.parse::<SiteUrl>().is_err(), "accepted {raw:?}");
        }
    }

    #[test]
    fn canonical_form_matches_the_old_companion_origin_keys() {
        // What the companion's `normalize_origin` returned for these inputs
        // before `SiteUrl` replaced its hand-built form.
        for (raw, old) in [
            ("HTTPS://LibrePaper.Example:5173/", "https://librepaper.example:5173"),
            ("https://librepaper.example:5173", "https://librepaper.example:5173"),
            ("https://librepaper.example", "https://librepaper.example"),
            ("https://papers.example", "https://papers.example"),
            ("https://papers.example:443/", "https://papers.example"),
            ("http://localhost:3000", "http://localhost:3000"),
            ("http://[::1]:3000/", "http://[::1]:3000"),
        ] {
            assert_eq!(canonical(raw), old, "{raw}");
        }
    }

    #[test]
    fn join_appends_a_path() {
        let site: SiteUrl = "https://app.example/".parse().unwrap();
        assert_eq!(site.join("/api/config"), "https://app.example/api/config");
    }
}
