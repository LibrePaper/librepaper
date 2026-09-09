//! The commands that act on a document somebody already published: listing,
//! naming one by a prefix of its id, opening it, sharing it, handing it over,
//! and destroying it.

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

pub async fn comment_document(
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

/// `librepaper edit` opens a document in the reader, with its source beside
/// it. The editor is part of the reader rather than a program of its own, so
/// this is what it should be: a way to get to the right page from a short id.
pub async fn edit_document(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    comment_document(identifier, server, token, key).await;
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

// Rights live on the document rather than in the server's flags: a document
// names its coauthors and its reviewers, and the flags are the ceiling it may
// not open past. These commands are that, on the command line.

/// The word a `--link` flag may spell a role with. The wire already accepts
/// both spellings (see `parse_role_word` on the server), so this exists only
/// to die locally with a clear message rather than spending a round trip to
/// learn that "editorr" is not a role. It returns the canonical, long-form
/// word, which is what a person reading the response back would expect to
/// see echoed.
pub(crate) fn parse_link_role(word: &str) -> Result<&'static str, String> {
    match word {
        "read" | "reader" => Ok("reader"),
        "comment" | "commenter" => Ok("commenter"),
        "edit" | "editor" => Ok("editor"),
        _ => Err(format!(
            "{word:?} is not a role: --link takes read, comment, or edit"
        )),
    }
}

/// `librepaper share c9k` with nothing else prints what the document says;
/// with a flag, changes it and prints the result.
#[allow(clippy::too_many_arguments)]
pub async fn share_document(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    link: String,
    until: String,
    label: Option<String>,
    budget: Option<i64>,
    revoke: String,
) {
    let server = server_or_die(server);
    let slug = resolve_identifier(identifier, &server, "", token.as_deref()).await;
    let target = format!("{server}/api/documents/{slug}/share");

    let mut change = json!({});
    let mut minted_role: Option<&'static str> = None;
    if !link.is_empty() {
        let role = parse_link_role(&link).unwrap_or_else(|err| die(err));
        minted_role = Some(role);
        change["link"] = json!({"role": role, "until": until, "budget": budget});
        if let Some(label) = label {
            change["link"]["label"] = json!(label);
        }
    } else if !until.is_empty() || label.is_some() || budget.is_some() {
        die("--until, --label and --budget describe a link; pass --link read, --link comment, or --link edit");
    }
    if !revoke.is_empty() {
        change["revoke"] = json!(revoke);
    }

    // Nothing to change is a request to see what is there, which needs no
    // write and no confirmation.
    let asking = change.as_object().is_some_and(|fields| fields.is_empty());
    let (status, payload) = if asking {
        get_with_token(
            &target,
            &require_token_for(&server, token.as_deref()),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|err| die(err))
    } else {
        post_json(
            &target,
            &change,
            &require_token_for(&server, token.as_deref()),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|err| die(err))
    };
    if status != 200 {
        die(format!("share failed ({status}): {}", detail_of(&payload)));
    }

    // A mint prints just the link it made and its expiry: the document keeps
    // the key now (see `LinkGrant::key`), so there is nothing left to warn
    // about, and a full listing would only bury the one line somebody ran
    // this command to see.
    if let Some(role) = minted_role {
        let empty = Value::Null;
        let link = payload
            .get("links")
            .and_then(|links| links.get(role))
            .unwrap_or(&empty);
        println!("{}", format_role_row(role, link, &server));
        return;
    }
    print_sharing(&payload, &server, &slug);
}

/// One row of `librepaper share`'s listing: the role, the link if it has one,
/// and what state that link is in. `server` and the row's own `url` (a path
/// on that origin) are joined here because that is the whole point of the
/// row -- a link nobody can paste anywhere is not much of a share.
pub(crate) fn format_role_row(role: &str, link: &Value, server: &str) -> String {
    if link.is_null() {
        return format!("  {role:<8} off");
    }
    let key = text(link, "key");
    if key.is_empty() {
        // A link written before the key was kept: the document still knows
        // it existed, but cannot show it, so the only way forward is a new
        // one.
        return format!("  {role:<8} legacy link, reset to get a new one");
    }
    let url = text(link, "url");
    let expired = link.get("expired") == Some(&Value::Bool(true));
    let mut until = text(link, "until");
    until.truncate(10);
    let state = if expired {
        "expired".to_string()
    } else if until.is_empty() {
        "no expiry".to_string()
    } else {
        format!("expires {until}")
    };
    let label = text(link, "label");
    let label = if label.is_empty() {
        String::new()
    } else {
        format!(" [{label}]")
    };
    let budget = link
        .get("budget")
        .and_then(Value::as_i64)
        .map(|budget| format!("   {budget} comments/hour"))
        .unwrap_or_default();
    format!("  {role:<8}{label} {server}{url}   {state}{budget}")
}

/// The full listing `librepaper share` with no flags prints, one line per
/// entry: the slug, the owner's own link, then one row per role in the fixed
/// order a reader, a commenter, and an editor matter to somebody deciding what
/// to change, then whatever legacy people are still named on the document.
/// Built as a plain `Vec<String>` rather than printed straight away, so the
/// order and the content of the report can be checked without capturing
/// stdout.
pub(crate) fn sharing_report_lines(payload: &Value, server: &str, slug: &str) -> Vec<String> {
    let mut lines = vec![
        slug.to_string(),
        // The owner's way in is their sign-in, and the row says so where the
        // others say when they expire: there is nothing to mint or revoke.
        format!("  {:<8} {server}/docs/{slug}   yours, signed in", "owner"),
    ];
    let links = payload.get("links").cloned().unwrap_or(Value::Null);
    let empty = Value::Null;
    for role in ["reader", "commenter", "editor"] {
        let link = links.get(role).unwrap_or(&empty);
        lines.push(format_role_row(role, link, server));
    }
    if let Some(legacy) = payload.get("legacy") {
        lines.push("  people (legacy)".to_string());
        for (field, role) in [("editors", "editor"), ("commenters", "commenter")] {
            for grant in legacy
                .get(field)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let mut since = text(&grant, "since");
                since.truncate(10);
                lines.push(format!("    {role:<9} @{}  {since}", text(&grant, "login")));
            }
        }
    }
    lines
}

pub(super) fn print_sharing(payload: &Value, server: &str, slug: &str) {
    for line in sharing_report_lines(payload, server, slug) {
        println!("{line}");
    }
}

/// Hands a document to somebody else: its history, its comments and its quota
/// go with it. Confirmed like `destroy`, because it is the one other change
/// that leaves the caller with nothing.
pub async fn transfer_document(
    identifier: &str,
    to: &str,
    server: Option<String>,
    token: Option<String>,
    yes: bool,
) {
    let server = server_or_die(server);
    let slug = resolve_identifier(identifier, &server, "", token.as_deref()).await;
    println!("About to transfer on {server}:");
    println!("  {slug}");
    println!("  to @{to}, with its history, its comments and its storage quota");
    println!("\nYou stop being its owner. Only @{to} can share or delete it after this.");
    if !yes {
        if !is_terminal_stdin() {
            die("refusing to transfer without a terminal to confirm at; pass --yes if you are certain");
        }
        eprint!("\nType '{slug}' to confirm: ");
        if read_line() != slug {
            println!("aborted, nothing was transferred");
            return;
        }
    }
    let (status, payload) = post_json(
        &format!("{server}/api/documents/{slug}/transfer"),
        &json!({"to": to}),
        &require_token_for(&server, token.as_deref()),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "transfer failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    println!("{slug} now belongs to @{}", text(&payload, "owner"));
}

pub async fn destroy_document(
    identifier: &str,
    server: Option<String>,
    token: Option<String>,
    yes: bool,
) {
    let server = server_or_die(server);
    // The same identifier `comment` and `export` take. The confirmation
    // below still asks for the whole slug: this is the one irreversible
    // command, and a three-character answer is too easy to give.
    let slug = resolve_identifier(identifier, &server, "", token.as_deref()).await;
    // Authenticated, not `get_json`: an owner's own document can be private,
    // and an unauthenticated read of it gets the same 404 a stranger would --
    // which used to stop `destroy` here before it ever reached the delete
    // route it already carries a token to. A document with no owner still
    // answers to an empty token the same as before.
    let (status, document) = get_with_token(
        &format!("{server}/api/documents/{slug}"),
        &stored_token_for(&server, token.as_deref()),
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("no document with the slug {slug:?} at {server}"));
    }
    let count = document
        .get("comment_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    println!("About to permanently delete from {server}:");
    println!("  {slug}  {}", text(&document, "title"));
    println!("  {count} comment(s), and every reply to them");
    println!("\nThe document, its history and its comments all go. The link stops");
    println!("working. Nothing else on this deployment is touched.");

    if !yes {
        if !is_terminal_stdin() {
            die("refusing to delete without a terminal to confirm at; pass --yes if you are certain");
        }
        eprint!("\nType '{slug}' to confirm: ");
        if read_line() != slug {
            println!("aborted, nothing was deleted");
            return;
        }
    }

    let (status, payload) = post_json(
        &format!("{server}/api/documents/{slug}/delete"),
        &json!({}),
        &require_token_for(&server, token.as_deref()),
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("delete failed ({status}): {}", detail_of(&payload)));
    }
    println!("deleted {slug}");
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
