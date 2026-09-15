//! Read-only access to Zotero desktop through its supported local Web API.
//!
//! The production endpoint is deliberately fixed to loopback. Tests may use
//! `Client::at` to provide a fake loopback server, but callers cannot turn a
//! Zotero request into an arbitrary network request.

use std::collections::{BTreeMap, BTreeSet};

use reqwest::{redirect::Policy, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

fn null_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

const LOCAL_API: &str = "http://127.0.0.1:23119/api/";
const API_VERSION: &str = "3";
const PAGE_SIZE: usize = 100;

#[derive(Debug)]
pub enum Error {
    Unreachable,
    Disabled,
    Incompatible,
    Request(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => write!(f, "Zotero is not running; start Zotero and try again"),
            Self::Disabled => write!(f, "Zotero local access is disabled; enable Settings -> Advanced -> Allow other applications on this computer to communicate with Zotero"),
            Self::Incompatible => write!(f, "Zotero answered but does not support the local API v3"),
            Self::Request(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Item {
    pub key: String,
    pub data: ItemData,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemData {
    #[serde(default, deserialize_with = "null_string")]
    pub item_type: String,
    #[serde(default, deserialize_with = "null_string")]
    pub title: String,
    #[serde(default)]
    pub creators: Vec<Creator>,
    #[serde(default, deserialize_with = "null_string")]
    pub date: String,
    #[serde(default, deserialize_with = "null_string")]
    pub publication_title: String,
    #[serde(default, deserialize_with = "null_string")]
    pub proceedings_title: String,
    #[serde(default, deserialize_with = "null_string")]
    pub book_title: String,
    #[serde(default, deserialize_with = "null_string")]
    pub volume: String,
    #[serde(default, deserialize_with = "null_string")]
    pub issue: String,
    #[serde(default, deserialize_with = "null_string")]
    pub pages: String,
    #[serde(default, deserialize_with = "null_string")]
    pub publisher: String,
    #[serde(default, deserialize_with = "null_string")]
    pub place: String,
    #[serde(rename = "DOI", default, deserialize_with = "null_string")]
    pub doi: String,
    #[serde(rename = "ISBN", default, deserialize_with = "null_string")]
    pub isbn: String,
    #[serde(rename = "ISSN", default, deserialize_with = "null_string")]
    pub issn: String,
    #[serde(default, deserialize_with = "null_string")]
    pub url: String,
    #[serde(default, deserialize_with = "null_string")]
    pub abstract_note: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Creator {
    #[serde(default, deserialize_with = "null_string")]
    pub creator_type: String,
    #[serde(default, deserialize_with = "null_string")]
    pub first_name: String,
    #[serde(default, deserialize_with = "null_string")]
    pub last_name: String,
    #[serde(default, deserialize_with = "null_string")]
    pub name: String,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct SearchResult {
    pub zotero_item: String,
    pub citation_key: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u16>,
    pub item_type: String,
}

impl Client {
    pub fn local() -> Self {
        Self::new(Url::parse(LOCAL_API).expect("constant Zotero local API URL"))
    }

    fn new(base: Url) -> Self {
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .expect("build Zotero HTTP client");
        Self { http, base }
    }

    pub async fn probe(&self) -> Result<(), Error> {
        let response = self.get("users/0/items", &[("limit", "1")]).await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(classify(response.status()))
        }
    }

    pub async fn item(&self, key: &str) -> Result<Item, Error> {
        if key.is_empty() || key.len() > 32 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(Error::Request("invalid Zotero item key".into()));
        }
        let response = self.get(&format!("users/0/items/{key}"), &[]).await?;
        if !response.status().is_success() {
            return Err(classify(response.status()));
        }
        response
            .json()
            .await
            .map_err(|error| Error::Request(format!("Zotero returned invalid item data: {error}")))
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, Error> {
        let limit = limit.clamp(1, PAGE_SIZE).to_string();
        let response = self
            .get(
                "users/0/items",
                &[
                    ("itemType", "-attachment"),
                    ("q", query),
                    ("qmode", "everything"),
                    ("limit", &limit),
                ],
            )
            .await?;
        if !response.status().is_success() {
            return Err(classify(response.status()));
        }
        let items: Vec<Item> = response.json().await.map_err(|error| {
            Error::Request(format!("Zotero returned invalid item data: {error}"))
        })?;
        Ok(items
            .into_iter()
            .map(|item| SearchResult {
                zotero_item: item.key,
                citation_key: base_key(&item.data),
                title: item.data.title,
                authors: author_names(&item.data.creators),
                year: issued_year(&item.data.date),
                item_type: item.data.item_type,
            })
            .collect())
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<reqwest::Response, Error> {
        self.http
            .get(
                self.base
                    .join(path)
                    .map_err(|error| Error::Request(error.to_string()))?,
            )
            .header("Zotero-API-Version", API_VERSION)
            .query(query)
            .send()
            .await
            .map_err(|_| Error::Unreachable)
    }
}

fn classify(status: StatusCode) -> Error {
    match status {
        StatusCode::FORBIDDEN => Error::Disabled,
        StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST | StatusCode::UPGRADE_REQUIRED => {
            Error::Incompatible
        }
        _ => Error::Request(format!("Zotero local API returned HTTP {status}")),
    }
}

fn creators(data: &ItemData) -> impl Iterator<Item = &Creator> {
    data.creators.iter().filter(|creator| {
        matches!(
            creator.creator_type.as_str(),
            "author"
                | "bookAuthor"
                | "inventor"
                | "programmer"
                | "artist"
                | "performer"
                | "composer"
                | "director"
                | "podcaster"
                | "presenter"
        )
    })
}

fn family(creator: &Creator) -> &str {
    if creator.last_name.is_empty() {
        &creator.name
    } else {
        &creator.last_name
    }
}

fn author_names(values: &[Creator]) -> Vec<String> {
    values
        .iter()
        .filter(|creator| creator.creator_type == "author")
        .map(|creator| {
            if !creator.name.is_empty() {
                creator.name.clone()
            } else if creator.first_name.is_empty() {
                creator.last_name.clone()
            } else {
                format!("{}, {}", creator.last_name, creator.first_name)
            }
        })
        .collect()
}

fn fragment(value: &str) -> String {
    let ascii: String = value
        .nfkd()
        .filter(|c| !is_combining_mark(*c) && c.is_ascii_alphabetic())
        .take(3)
        .collect();
    let mut chars = ascii.to_ascii_lowercase().chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

fn issued_year(date: &str) -> Option<u16> {
    let bytes = date.as_bytes();
    bytes.windows(4).find_map(|window| {
        let value = std::str::from_utf8(window).ok()?;
        (value.bytes().all(|b| b.is_ascii_digit()))
            .then(|| value.parse().ok())
            .flatten()
    })
}

pub fn base_key(data: &ItemData) -> String {
    let author = creators(data)
        .take(5)
        .map(family)
        .map(fragment)
        .filter(|part| !part.is_empty())
        .collect::<String>();
    let stem = if !author.is_empty() {
        author
    } else {
        const STOP: &[&str] = &[
            "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
            "with",
        ];
        data.title
            .split(|c: char| !c.is_alphabetic())
            .filter(|word| !word.is_empty() && !STOP.contains(&word.to_lowercase().as_str()))
            .map(fragment)
            .filter(|part| !part.is_empty())
            .take(3)
            .collect()
    };
    let year = issued_year(&data.date)
        .map(|year| year.to_string())
        .unwrap_or_else(|| "ND".to_owned());
    if !stem.is_empty() {
        return format!("{stem}{year}");
    }
    let mut digest = Sha256::new();
    for value in creators(data)
        .map(family)
        .chain([data.date.as_str(), data.title.as_str()])
    {
        digest.update(value.len().to_be_bytes());
        digest.update(value.as_bytes());
    }
    format!("Ref{}", &hex::encode(digest.finalize())[..8])
}

fn suffix(mut index: usize) -> String {
    let mut value = String::new();
    while index > 0 {
        index -= 1;
        value.push((b'a' + (index % 26) as u8) as char);
        index /= 26;
    }
    value.chars().rev().collect()
}

pub fn bibliography(items: &[Item], existing: &BTreeMap<String, String>) -> String {
    let mut ordered = items.to_vec();
    ordered.sort_by(|a, b| a.key.cmp(&b.key));
    let mut used: BTreeSet<String> = existing.values().cloned().collect();
    let mut entries = Vec::new();
    for item in ordered {
        let base = base_key(&item.data);
        let key = existing.get(&item.key).cloned().unwrap_or_else(|| {
            if used.insert(base.clone()) {
                return base;
            }
            for n in 1.. {
                let candidate = format!("{base}{}", suffix(n));
                if used.insert(candidate.clone()) {
                    return candidate;
                }
            }
            unreachable!()
        });
        entries.push((key, bib_entry(&item)));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
        .into_iter()
        .map(|(key, body)| format!("@{}{{{},\n{}}}\n", bib_type(&body.0), key, body.1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn bib_type(item_type: &str) -> &'static str {
    match item_type {
        "journalArticle" => "article",
        "book" => "book",
        "bookSection" => "incollection",
        "conferencePaper" => "inproceedings",
        "thesis" => "phdthesis",
        "report" => "techreport",
        "webpage" | "blogPost" | "forumPost" => "online",
        _ => "misc",
    }
}

fn bib_entry(item: &Item) -> (String, String) {
    let data = &item.data;
    let authors = creators(data)
        .map(|creator| {
            if !creator.name.is_empty() {
                format!("{{{}}}", creator.name)
            } else {
                format!("{}, {}", creator.last_name, creator.first_name)
            }
        })
        .collect::<Vec<_>>()
        .join(" and ");
    let mut fields = Vec::new();
    let mut add = |name: &str, value: &str| {
        if !value.is_empty() {
            fields.push(format!("  {name} = {{{}}}", value.replace(['{', '}'], "")));
        }
    };
    add("author", &authors);
    add("title", &data.title);
    add(
        "year",
        &issued_year(&data.date)
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    add("journal", &data.publication_title);
    add(
        "booktitle",
        if data.proceedings_title.is_empty() {
            &data.book_title
        } else {
            &data.proceedings_title
        },
    );
    add("volume", &data.volume);
    add("number", &data.issue);
    add("pages", &data.pages);
    add("publisher", &data.publisher);
    add("address", &data.place);
    add("doi", &data.doi);
    add("isbn", &data.isbn);
    add("issn", &data.issn);
    add("url", &data.url);
    add("abstract", &data.abstract_note);
    add("x-librepaper-zotero-library", "0");
    add("x-librepaper-zotero-item", &item.key);
    (data.item_type.clone(), fields.join(",\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str, date: &str, names: &[&str]) -> ItemData {
        ItemData {
            title: title.into(),
            date: date.into(),
            creators: names
                .iter()
                .map(|name| Creator {
                    creator_type: "author".into(),
                    last_name: (*name).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn specified_keys() {
        assert_eq!(base_key(&item("", "2023", &["Smith"])), "Smi2023");
        assert_eq!(
            base_key(&item("", "2023", &["Smith", "Jones"])),
            "SmiJon2023"
        );
        assert_eq!(
            base_key(&item(
                "",
                "2023",
                &["Arel-Bundock", "Blair", "Briggs", "MacDonald"]
            )),
            "AreBlaBriMac2023"
        );
        assert_eq!(
            base_key(&item("Effects of Trees on Policy", "2023", &[])),
            "EffTrePol2023"
        );
        assert_eq!(base_key(&item("", "", &["Smíth"])), "SmiND");
    }

    #[test]
    fn spreadsheet_suffixes() {
        assert_eq!(suffix(1), "a");
        assert_eq!(suffix(26), "z");
        assert_eq!(suffix(27), "aa");
    }

    #[test]
    fn nullable_zotero_fields_are_empty_strings() {
        let item: Item = serde_json::from_str(r#"{"key":"ABC123","data":{"itemType":"webpage","title":"Page","date":null,"DOI":null,"creators":[{"creatorType":"author","name":null}]}}"#).unwrap();
        assert_eq!(item.data.date, "");
        assert_eq!(item.data.doi, "");
        assert_eq!(item.data.creators[0].name, "");
        assert_eq!(base_key(&item.data), "PagND");
    }
}
