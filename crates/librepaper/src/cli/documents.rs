//! The commands that identify and open a document somebody already published.

use super::*;

pub async fn list_documents(server: Option<String>, token: Option<String>) {
    let server = server_or_die(server);
    let documents = visible_documents(&server, &require_token_for(&server, token.as_deref()))
        .await
        .unwrap_or_else(|err| die(err.to_string()));
    if documents.is_empty() {
        println!("no documents yet");
        return;
    }
    let ids = short_ids(&documents, &Configuration::default());
    let width = ids.values().map(String::len).max().unwrap_or(0);
    for document in &documents {
        let mut updated = text(document, "updated_at");
        updated.truncate(10);
        let slug = text(document, "slug");
        // A document shared with you by name is in your list, and says what
        // you hold on it. Your own say nothing: everything here without a mark
        // is yours, which is what this listing has always meant.
        let role = text(document, "role");
        let held = if role == "owner" || role.is_empty() {
            String::new()
        } else {
            format!("  ({role})")
        };
        println!(
            "{:<width$}  {}  {}{held}",
            ids.get(&slug).cloned().unwrap_or_default(),
            updated,
            text(document, "title")
        );
    }
}

/// The shortest handle `list` will print. One character is unique today and
/// ambiguous after the next publish, and it reads as a typo rather than a
/// name; three is short enough to type and stable enough to keep in a note.
pub(super) const SHORT_ID_MINIMUM: usize = 3;

/// Gives each listed document a short handle: a prefix of its generated random
/// suffix (or explicit slug). Every handle is cut to the same width -- ragged
/// ids are hard to read down a column and hard to remember -- which is the
/// longest prefix any one document needs to be unambiguous, and never fewer
/// than three characters.
pub fn short_ids(
    documents: &[Value],
    config: &Configuration,
) -> std::collections::HashMap<String, String> {
    let mut items: Vec<(String, String)> = documents
        .iter()
        .filter_map(|document| {
            let slug = document.get("slug")?.as_str()?.to_string();
            let last = slug.rsplit('-').next().unwrap_or(&slug).to_string();
            let key = if last.len() == config.suffix_length
                && last.chars().all(|c| config.suffix_alphabet.contains(c))
            {
                last
            } else {
                slug.clone()
            };
            Some((slug, key))
        })
        .collect();

    let counts = items.iter().fold(
        std::collections::HashMap::<String, usize>::new(),
        |mut counts, (_, key)| {
            *counts.entry(key.clone()).or_default() += 1;
            counts
        },
    );
    for (slug, key) in &mut items {
        if counts[key] > 1 {
            *key = slug.clone();
        }
    }

    let mut width = SHORT_ID_MINIMUM;
    for (_, key) in &items {
        let mut needed = key.len();
        for length in 1..=key.chars().count() {
            let prefix: String = key.chars().take(length).collect();
            if items
                .iter()
                .filter(|(_, other)| other.starts_with(&prefix))
                .count()
                == 1
            {
                needed = length;
                break;
            }
        }
        width = width.max(needed);
    }

    let mut handles: std::collections::HashMap<String, String> = items
        .into_iter()
        .map(|(slug, key)| {
            // A key shorter than the common width is used whole; it is already
            // as distinct as it will ever be.
            let id = if width < key.len() {
                key.chars().take(width).collect()
            } else {
                key
            };
            (slug, id)
        })
        .collect();
    loop {
        let ambiguous: Vec<String> = handles
            .iter()
            .filter(|&(slug, handle)| {
                handles.iter().any(|(other_slug, other_handle)| {
                    other_slug != slug && (other_handle == handle || other_slug == handle)
                })
            })
            .map(|(slug, _)| slug.clone())
            .collect();
        if ambiguous.is_empty() {
            break;
        }
        for slug in ambiguous {
            handles.insert(slug.clone(), slug);
        }
    }
    handles
}

/// Materialize every listing page before assigning handles. Cursor repetition
/// is an invalid server response, not a reason to loop forever.
#[derive(Debug)]
struct ListingError {
    status: Option<u16>,
    message: String,
}

impl std::fmt::Display for ListingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<String> for ListingError {
    fn from(message: String) -> Self {
        Self {
            status: None,
            message,
        }
    }
}

impl From<&str> for ListingError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

async fn visible_documents(server: &str, token: &str) -> Result<Vec<Value>, ListingError> {
    let mut target =
        url::Url::parse(&format!("{server}/api/list")).map_err(|err| err.to_string())?;
    let mut documents = Vec::new();
    let mut seen_slugs = std::collections::HashSet::new();
    let mut cursors = std::collections::HashSet::new();
    loop {
        let (status, payload) =
            post_json(target.as_str(), &json!({}), token, Duration::from_secs(60)).await?;
        if status != 200 {
            return Err(ListingError {
                status: Some(status),
                message: format!("listing failed ({status}): {}", detail_of(&payload)),
            });
        }
        let page = payload
            .get("documents")
            .and_then(Value::as_array)
            .ok_or("invalid document listing: missing documents")?;
        for document in page {
            let slug = text(document, "slug");
            if !slug.is_empty() && seen_slugs.insert(slug) {
                documents.push(document.clone());
            }
        }
        let Some(cursor) = payload.get("next_cursor").filter(|v| !v.is_null()) else {
            break;
        };
        let updated = cursor
            .get("after_updated")
            .and_then(Value::as_str)
            .ok_or("invalid listing cursor")?;
        let slug = cursor
            .get("after_slug")
            .and_then(Value::as_str)
            .ok_or("invalid listing cursor")?;
        if !cursors.insert((updated.to_string(), slug.to_string())) {
            return Err("server repeated a document listing cursor".into());
        }
        target
            .query_pairs_mut()
            .clear()
            .append_pair("after_updated", updated)
            .append_pair("after_slug", slug);
    }
    Ok(documents)
}

/// Resolve both interpretations before accepting a handle. A readable full
/// slug outside the listing must not silently shadow a listed handle.
fn match_identifier(identifier: &str, documents: &[Value], direct: bool) -> Result<String, String> {
    let ids = short_ids(documents, &Configuration::default());
    let mut matches = std::collections::HashSet::new();
    if direct {
        matches.insert(identifier.to_string());
    }
    for (slug, handle) in ids {
        if identifier == slug || identifier == handle {
            matches.insert(slug);
        }
    }
    if matches.len() > 1 {
        return Err(format!(
            "{identifier:?} matches more than one document; use the complete slug or a share link"
        ));
    }
    matches
        .into_iter()
        .next()
        .ok_or_else(|| format!("no visible document matches {identifier:?}"))
}

pub async fn resolve_identifier(
    identifier: &str,
    server: &str,
    key: &str,
    token: Option<&str>,
) -> String {
    resolve_identifier_with(identifier, server, key, &stored_token_for(server, token))
        .await
        .unwrap_or_else(|err| die(err))
}

async fn resolve_identifier_with(
    identifier: &str,
    server: &str,
    key: &str,
    token: &str,
) -> Result<String, String> {
    let (status, payload) = get_as(
        &format!("{server}/api/documents/{identifier}"),
        &Credentials::new(token, key),
        Duration::from_secs(30),
    )
    .await?;
    if status != 200 && status != 404 {
        return Err(format!(
            "cannot read {identifier:?} ({status}): {}",
            detail_of(&payload)
        ));
    }
    // An explicit link authenticates this document directly. Without an
    // account no listing exists to interpret as an alternative handle.
    if status == 200 && (!key.is_empty() || token.is_empty()) {
        return Ok(identifier.to_string());
    }
    if token.is_empty() {
        return Err(format!("document {identifier:?} was not found or this link cannot access it; short handles require a sign-in"));
    }
    let documents = match visible_documents(server, token).await {
        Ok(documents) => documents,
        // A named reader can access a document while the deployment denies
        // publisher-only listing. No short handles are available to them.
        Err(error) if status == 200 && matches!(error.status, Some(401 | 403)) => {
            return Ok(identifier.to_string());
        }
        Err(error) => return Err(error.to_string()),
    };
    match_identifier(identifier, &documents, status == 200)
}

pub async fn open_document(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    let server = server_or_die(server);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key, token.as_deref()).await;
    // The key goes back where a browser expects it, in the fragment, so the
    // page opened is the link exactly as it was shared.
    if key.is_empty() {
        open_url(&format!("{server}/docs/{slug}"));
    } else {
        open_url(&format!("{server}/docs/{slug}#k={key}"));
    }
}

/// What `--key` takes: the key itself, or the whole link it came in, since a
/// link is what a person actually has in their clipboard. A URL with no key
/// in its fragment is an empty key, which is what a plain document URL
/// carries.
pub fn link_key(flag: &str) -> String {
    let flag = flag.trim();
    let Some((_, fragment)) = flag.split_once('#') else {
        return if flag.contains("://") {
            String::new()
        } else {
            flag.to_string()
        };
    };
    fragment
        .split('&')
        .find_map(|part| part.strip_prefix("k="))
        .map(|key| {
            url::form_urlencoded::parse(format!("k={key}").as_bytes())
                .next()
                .map(|(_, value)| value.to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

pub fn open_url(target: &str) {
    let (command, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![target])
    } else if cfg!(target_os = "windows") {
        ("rundll32", vec!["url.dll,FileProtocolHandler", target])
    } else {
        ("xdg-open", vec![target])
    };
    if let Err(err) = std::process::Command::new(command).args(args).spawn() {
        die(format!("could not open {target}: {err}"));
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    #[test]
    fn complete_slugs_and_duplicate_suffixes_cannot_shadow_handles() {
        let documents = vec![
            json!({"slug":"abcdefghij"}),
            json!({"slug":"paper-abcdefghij"}),
        ];
        let handles = short_ids(&documents, &Configuration::default());
        assert_ne!(handles["abcdefghij"], handles["paper-abcdefghij"]);
        for (slug, handle) in &handles {
            assert_eq!(
                match_identifier(handle, &documents, handle == slug).unwrap(),
                *slug
            );
        }
        let documents = vec![json!({"slug":"paper-abcdefghij"})];
        assert!(match_identifier("abc", &documents, true)
            .unwrap_err()
            .contains("more than one"));
    }

    #[tokio::test]
    async fn listing_follows_cursors_before_resolving_handles() {
        use axum::{extract::Query, routing::post, Json};
        let app = axum::Router::new().route("/api/list", post(|Query(query): Query<std::collections::HashMap<String, String>>| async move {
            if query.get("after_slug").is_some_and(|slug| slug == "first-abcdefghij") {
                assert_eq!(query["after_updated"], "2026-01-01T00:00:00+00:00");
                Json(json!({"documents":[{"slug":"second-zyxwvutsrq"}]}))
            } else {
                Json(json!({"documents":[{"slug":"first-abcdefghij"}], "next_cursor":{"after_slug":"first-abcdefghij", "after_updated":"2026-01-01T00:00:00+00:00"}}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let documents = visible_documents(&server, "fixture").await.unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(
            match_identifier("zyx", &documents, false).unwrap(),
            "second-zyxwvutsrq"
        );
        task.abort();
    }

    #[tokio::test]
    async fn named_reader_can_resolve_a_full_slug_without_listing_permission() {
        use axum::{
            http::StatusCode,
            routing::{get, post},
            Json,
        };
        let app = axum::Router::new()
            .route(
                "/api/documents/paper",
                get(|| async { Json(json!({"slug":"paper"})) }),
            )
            .route("/api/list", post(|| async { StatusCode::FORBIDDEN }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(
            resolve_identifier_with("paper", &server, "", "reader")
                .await
                .unwrap(),
            "paper"
        );
        task.abort();
    }

    #[tokio::test]
    async fn failed_document_read_is_not_reported_as_missing_login() {
        use axum::{http::StatusCode, routing::get};
        let app = axum::Router::new().route(
            "/api/documents/paper",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = resolve_identifier_with("paper", &server, "key", "")
            .await
            .unwrap_err();
        assert!(error.contains("503"), "{error}");
        task.abort();
    }
}
