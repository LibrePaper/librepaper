//! The commands that act on a document somebody already published: listing,
//! naming one by a prefix of its id, opening it, sharing it, handing it over,
//! and destroying it.

use super::*;

pub async fn list_documents(server_flag: String) {
    let server = server_from(&server_flag);
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token_for(&server),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "listing failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let documents = payload
        .get("documents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
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
    let items: Vec<(String, String)> = documents
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

    let mut width = SHORT_ID_MINIMUM;
    for (_, key) in &items {
        let mut needed = key.len();
        for length in 1..=key.len() {
            let prefix = &key[..length];
            if items
                .iter()
                .filter(|(_, other)| other.starts_with(prefix))
                .count()
                == 1
            {
                needed = length;
                break;
            }
        }
        width = width.max(needed);
    }

    items
        .into_iter()
        .map(|(slug, key)| {
            // A key shorter than the common width is used whole; it is already
            // as distinct as it will ever be.
            let id = if width < key.len() {
                key[..width].to_string()
            } else {
                key
            };
            (slug, id)
        })
        .collect()
}

/// Turns what the user typed -- a full slug, or one of the short handles
/// `list` prints -- into the slug the API knows.
pub async fn resolve_identifier(identifier: &str, server: &str, key: &str) -> String {
    // A full slug needs no listing: this is the path a command run from a
    // link someone sent takes, with the link's key as its credential, and
    // the path an owner's own slug takes with their sign-in.
    if let Ok((200, _)) = get_as(
        &format!("{server}/api/documents/{identifier}"),
        &Credentials::new(&stored_token_for(server), key),
        Duration::from_secs(30),
    )
    .await
    {
        return identifier.to_string();
    }
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token_for(server),
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "listing failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let documents = payload
        .get("documents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let ids = short_ids(&documents, &Configuration::default());
    let mut found = String::new();
    for document in &documents {
        let slug = text(document, "slug");
        if identifier == slug || ids.get(&slug).is_some_and(|id| id == identifier) {
            if !found.is_empty() {
                die(format!("{identifier:?} matches more than one document"));
            }
            found = slug;
        }
    }
    if found.is_empty() {
        die(format!("no visible document matches {identifier:?}"));
    }
    found
}

pub async fn comment_document(identifier: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
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

/// `komodoc edit` opens a document in the reader, with its source beside it.
/// The editor is part of the reader rather than a program of its own, so this
/// is what it should be: a way to get to the right page from a short id.
pub async fn edit_document(identifier: &str, server_flag: String, key: String) {
    comment_document(identifier, server_flag, key).await;
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

/// `komodoc share c9k` with nothing else prints what the document says; with a
/// flag, changes it and prints the result.
pub async fn share_document(
    identifier: &str,
    server_flag: String,
    link: String,
    until: String,
    label: Option<String>,
    budget: Option<i64>,
    revoke: String,
) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
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
            &require_token_for(&server),
            Duration::from_secs(60),
        )
        .await
        .unwrap_or_else(|err| die(err))
    } else {
        post_json(
            &target,
            &change,
            &require_token_for(&server),
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

/// One row of `komodoc share`'s listing: the role, the link if it has one,
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

/// The full listing `komodoc share` with no flags prints, one line per entry:
/// the slug, the owner's own link, then one row per role in the fixed order a
/// reader, a commenter, and an editor matter to somebody deciding what to
/// change, then whatever legacy people are still named on the document. Built as a plain
/// `Vec<String>` rather than printed straight away, so the order and the
/// content of the report can be checked without capturing stdout.
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
pub async fn transfer_document(identifier: &str, to: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
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
        &require_token_for(&server),
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

pub async fn destroy_document(identifier: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    // The same identifier `comment` and `export` take. The confirmation
    // below still asks for the whole slug: this is the one irreversible
    // command, and a three-character answer is too easy to give.
    let slug = resolve_identifier(identifier, &server, "").await;
    // Authenticated, not `get_json`: an owner's own document can be private,
    // and an unauthenticated read of it gets the same 404 a stranger would --
    // which used to stop `destroy` here before it ever reached the delete
    // route it already carries a token to. A document with no owner still
    // answers to an empty token the same as before.
    let (status, document) = get_with_token(
        &format!("{server}/api/documents/{slug}"),
        &stored_token_for(&server),
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
        &require_token_for(&server),
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("delete failed ({status}): {}", detail_of(&payload)));
    }
    println!("deleted {slug}");
}
