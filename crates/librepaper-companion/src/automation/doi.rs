//! DOI lookup for the `reference_from_doi` relay tool. The lookup runs on the
//! user's own machine, never on the LibrePaper server. The conversion is
//! mirrored byte for byte by `web/src/lib/doi.js`; both are tested against
//! `web/tests/fixtures/doi-bibtex.json`.

use std::time::Duration;

use reqwest::{redirect::Policy, StatusCode};
use serde_json::Value;

use crate::local::zotero::fragment;

pub struct Converted {
    /// The provisional key; the caller de-duplicates it against the project.
    pub key: String,
    pub bibtex: String,
}

/// The bare DOI for what a user typed, or `None` when it is not a DOI.
pub fn normalise(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = if lower.starts_with("doi:") {
        trimmed[4..].trim_start().to_owned()
    } else if let Some(length) = [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "http://dx.doi.org/",
    ]
    .iter()
    .find(|prefix| lower.starts_with(**prefix))
    .map(|prefix| prefix.len())
    {
        percent_decode(&trimmed[length..])
    } else {
        trimmed.to_owned()
    };
    valid(&rest).then_some(rest)
}

/// `^10\.\d{4,9}/\S+$`
fn valid(doi: &str) -> bool {
    let Some(rest) = doi.strip_prefix("10.") else {
        return false;
    };
    let Some((digits, suffix)) = rest.split_once('/') else {
        return false;
    };
    (4..=9).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !suffix.is_empty()
        && !suffix.chars().any(char::is_whitespace)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

fn encode_segment(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The CSL-JSON record doi.org holds for a normalised DOI.
pub async fn fetch(doi: &str) -> Result<Value, String> {
    const UNREACHABLE: &str = "could not reach doi.org";
    let path = doi.split('/').map(encode_segment).collect::<Vec<_>>().join("/");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(Policy::limited(5))
        .build()
        .map_err(|_| UNREACHABLE.to_owned())?;
    let response = client
        .get(format!("https://doi.org/{path}"))
        .header("Accept", "application/vnd.citationstyles.csl+json")
        .send()
        .await
        .map_err(|_| UNREACHABLE.to_owned())?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err("no record for this DOI".to_owned());
    }
    if !response.status().is_success() {
        return Err(UNREACHABLE.to_owned());
    }
    response
        .json::<Value>()
        .await
        .map_err(|_| UNREACHABLE.to_owned())
}

/// Text cleanup applied to every value; `None` when nothing is left.
fn clean(value: &Value) -> Option<String> {
    let first = match value {
        Value::Array(items) => items.first()?,
        other => other,
    };
    let text = match first {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let mut stripped = String::with_capacity(text.len());
    let mut in_tag = false;
    let mut pending = String::new();
    for c in text.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                pending.clear();
            } else {
                pending.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else {
            stripped.push(c);
        }
    }
    if in_tag {
        // An unclosed `<` is not a tag match: keep it and what followed.
        stripped.push('<');
        stripped.push_str(&pending);
    }
    let decoded = stripped
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let joined = decoded
        .chars()
        .filter(|c| *c != '{' && *c != '}')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!joined.is_empty()).then_some(joined)
}

fn field(csl: &Value, name: &str) -> Option<String> {
    csl.get(name).and_then(clean)
}

fn year(csl: &Value) -> Option<String> {
    let first = csl.get("issued")?.get("date-parts")?.get(0)?.get(0)?;
    match first {
        Value::Number(number) => Some(number.to_string()),
        Value::String(text) => text.trim().parse::<u32>().ok().map(|n| n.to_string()),
        _ => None,
    }
}

fn pages(page: &str) -> String {
    let page = page.replace('\u{2013}', "--");
    let chars: Vec<char> = page.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-' {
            let mut j = i;
            while j < chars.len() && chars[j] == '-' {
                j += 1;
            }
            if j - i == 1 {
                out.push_str("--");
            } else {
                out.extend(&chars[i..j]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn is_doi_host(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    host == "doi.org" || host == "dx.doi.org"
}

struct Author {
    /// The text written into the `author` field.
    written: String,
    /// The text the key fragment is taken from.
    key_source: String,
}

fn authors(csl: &Value) -> Vec<Author> {
    csl.get("author")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|author| {
            let family = field(author, "family");
            let literal = field(author, "literal");
            if let Some(family) = family {
                let written = match field(author, "given") {
                    Some(given) => format!("{family}, {given}"),
                    None => family.clone(),
                };
                Some(Author {
                    written,
                    key_source: family,
                })
            } else {
                literal.map(|literal| Author {
                    written: format!("{{{literal}}}"),
                    key_source: literal,
                })
            }
        })
        .collect()
}

fn key_for(authors: &[Author], title: Option<&str>, year: Option<&str>) -> String {
    const STOP: &[&str] = &[
        "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
        "with",
    ];
    let mut stem: String = authors
        .iter()
        .take(5)
        .map(|author| fragment(&author.key_source))
        .filter(|part| !part.is_empty())
        .collect();
    if stem.is_empty() {
        stem = title
            .unwrap_or("")
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty() && !STOP.contains(&word.to_lowercase().as_str()))
            .map(fragment)
            .filter(|part| !part.is_empty())
            .take(3)
            .collect();
    }
    let year = year.unwrap_or("ND");
    if stem.is_empty() {
        format!("Ref{year}")
    } else {
        format!("{stem}{year}")
    }
}

/// BibTeX for a CSL-JSON record, keyed provisionally.
pub fn convert(csl: &Value, doi: &str) -> Converted {
    let kind = match csl.get("type").and_then(Value::as_str).unwrap_or("") {
        "article-journal" => "article",
        "book" => "book",
        "chapter" => "incollection",
        "paper-conference" => "inproceedings",
        "thesis" => "phdthesis",
        "report" => "techreport",
        "webpage" => "online",
        _ => "misc",
    };
    let authors = authors(csl);
    let title = field(csl, "title");
    let year = year(csl);
    let container = field(csl, "container-title");
    let url = field(csl, "URL").filter(|url| !is_doi_host(url));
    let mut fields: Vec<(&str, Option<String>)> = vec![
        (
            "author",
            (!authors.is_empty()).then(|| {
                authors
                    .iter()
                    .map(|author| author.written.as_str())
                    .collect::<Vec<_>>()
                    .join(" and ")
            }),
        ),
        ("title", title.clone()),
        ("year", year.clone()),
        ("journal", container.clone().filter(|_| kind == "article")),
        (
            "booktitle",
            container
                .clone()
                .filter(|_| kind == "incollection" || kind == "inproceedings"),
        ),
        ("volume", field(csl, "volume")),
        ("number", field(csl, "issue")),
        ("pages", field(csl, "page").map(|page| pages(&page))),
        ("publisher", field(csl, "publisher")),
        ("address", field(csl, "publisher-place")),
        ("doi", field(csl, "DOI").or_else(|| Some(doi.to_owned()))),
        ("isbn", field(csl, "ISBN")),
        ("issn", field(csl, "ISSN")),
        ("url", url),
    ];
    fields.retain(|(_, value)| value.is_some());
    let key = key_for(&authors, title.as_deref(), year.as_deref());
    let body = fields
        .iter()
        .map(|(name, value)| format!("  {name} = {{{}}}", value.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join(",\n");
    Converted {
        bibtex: format!("@{kind}{{{key},\n{body}\n}}\n"),
        key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: &str = include_str!("../../../../web/tests/fixtures/doi-bibtex.json");

    #[test]
    fn convert_matches_the_shared_fixture() {
        let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
        let cases = fixtures["convert"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let converted = convert(&case["csl"], case["doi"].as_str().unwrap());
            assert_eq!(converted.key, case["key"].as_str().unwrap(), "{name}: key");
            assert_eq!(
                converted.bibtex,
                case["bibtex"].as_str().unwrap(),
                "{name}: bibtex"
            );
        }
    }

    #[test]
    fn normalise_matches_the_shared_fixture() {
        let fixtures: Value = serde_json::from_str(FIXTURES).unwrap();
        let cases = fixtures["normalise"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let input = case["input"].as_str().unwrap();
            assert_eq!(
                normalise(input).as_deref(),
                case["doi"].as_str(),
                "input {input:?}"
            );
        }
    }
}
