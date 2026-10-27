use librepaper_base::ids::Slug;
use url::Url;

use crate::Error;

/// The web project a directory is linked to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The origin of the deployment, such as `https://app.librepaper.org`.
    pub site: Url,
    pub slug: String,
}

impl Link {
    /// Reads `https://<site>/docs/<slug>`, with anything after the slug (a
    /// share key, a query, a fragment) ignored.
    pub fn parse(project_url: &str) -> Result<Link, Error> {
        let bad = || {
            Error::Link(format!(
                "{project_url:?} is not a project URL such as https://app.librepaper.org/docs/<slug>"
            ))
        };
        let url = Url::parse(project_url.trim()).map_err(|_| bad())?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(bad());
        }
        let mut segments = url.path_segments().ok_or_else(bad)?;
        if segments.next() != Some("docs") {
            return Err(bad());
        }
        let slug = segments
            .next()
            .filter(|slug| slug.parse::<Slug>().is_ok())
            .ok_or_else(bad)?;
        Ok(Link::at(&url, slug.to_string()))
    }

    /// A link on the deployment `site` belongs to, which may be any URL on it.
    pub(crate) fn at(site: &Url, slug: String) -> Link {
        let origin = site.origin().ascii_serialization();
        Link {
            site: Url::parse(&origin).unwrap_or_else(|_| site.clone()),
            slug,
        }
    }

    /// `https://<site>`, with no trailing slash.
    pub fn origin(&self) -> String {
        self.site.origin().ascii_serialization()
    }

    /// The project page, as `parse` reads it.
    pub fn project_url(&self) -> String {
        format!("{}/docs/{}", self.origin(), self.slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_page_parses_with_or_without_a_key() {
        for text in [
            "https://app.librepaper.org/docs/paper-abc",
            "https://app.librepaper.org/docs/paper-abc/",
            "https://app.librepaper.org/docs/paper-abc/KEY123",
            "https://app.librepaper.org/docs/paper-abc?x=1#k=abc",
            "  https://app.librepaper.org/docs/paper-abc  ",
        ] {
            let link = Link::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(link.slug, "paper-abc", "{text}");
            assert_eq!(link.origin(), "https://app.librepaper.org", "{text}");
        }
        let link = Link::parse("http://127.0.0.1:8179/docs/local-one").unwrap();
        assert_eq!(link.origin(), "http://127.0.0.1:8179");
        assert_eq!(link.project_url(), "http://127.0.0.1:8179/docs/local-one");
    }

    #[test]
    fn other_urls_are_refused() {
        for text in [
            "",
            "paper-abc",
            "https://app.librepaper.org/",
            "https://app.librepaper.org/docs",
            "https://app.librepaper.org/docs/",
            "https://app.librepaper.org/papers/abc",
            "https://app.librepaper.org/docs/a%2Fb",
            "ftp://app.librepaper.org/docs/abc",
            "file:///docs/abc",
        ] {
            assert!(Link::parse(text).is_err(), "{text:?} should be refused");
        }
    }
}
