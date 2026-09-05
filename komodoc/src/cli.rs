//! The command line: publishing, listing, opening, signing in. Every command
//! here talks to a deployment over HTTP, the way a browser does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Configuration;
use crate::http::{detail_of, get_json, get_with_token, post_json, text};
use crate::render::{
    counted, is_html, is_markdown, is_typst, render_typst_document, report, title_from_html,
    title_from_markdown, title_from_typst,
};
use crate::util::{die, is_terminal_stdin, is_terminal_stdout, read_line};

pub fn server_from(flag: &str) -> String {
    let mut server = flag.to_string();
    if server.is_empty() {
        server = std::env::var("KOMODOC_SERVER").unwrap_or_default();
    }
    if server.is_empty() {
        die("set --server or $KOMODOC_SERVER");
    }
    server.trim_end_matches('/').to_string()
}

/* --------------------------------------------------------------- login */

// The CLI signs in through the deployment, not through a provider: it asks the
// server for a code, you open the URL it prints and approve there with
// whichever provider that deployment offers, and the token lands here. No
// callback URL and no local web server, so it works over SSH and on a machine
// with no browser of its own -- and adding a provider to a deployment adds it
// to `login` with no new flag and no new release of this binary.

/// Where the token is cached, following XDG.
pub fn token_path() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(base) if !base.is_empty() => PathBuf::from(base),
        _ => {
            let home = std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| die("no home directory to store the token in"));
            Path::new(&home).join(".config")
        }
    };
    base.join("komodoc").join("token")
}

/// The token to send, from the environment or the cache written by
/// `komodoc login`. A `KOMODOC_TOKEN` holding a GitHub token still works: the
/// server tells the two apart by the `kmd_` prefix and verifies each its own
/// way.
pub fn stored_token() -> String {
    if let Ok(token) = std::env::var("KOMODOC_TOKEN") {
        if !token.trim().is_empty() {
            return token.trim().to_string();
        }
    }
    std::fs::read_to_string(token_path())
        .map(|raw| raw.trim().to_string())
        .unwrap_or_default()
}

/// What every command that writes needs.
pub fn require_token() -> String {
    let token = stored_token();
    if token.is_empty() {
        die("not signed in. Run:\n    komodoc login");
    }
    token
}

pub async fn login(server_flag: String) {
    let server = server_from(&server_flag);
    let code = request_device_code(&server)
        .await
        .unwrap_or_else(|err| die(format!("could not start the sign-in: {err}")));
    eprintln!(
        "\n  Open {}\n  and enter the code:  {}\n",
        code.verification_url, code.user_code
    );
    eprint!("  waiting for you to approve it");

    let token = poll_for_token(&server, &code).await;
    eprintln!();
    let token = token.unwrap_or_else(|err| die(err));

    // Who the token says you are. It is this deployment's own token, so the
    // deployment is the only thing that can answer, and it costs one call.
    let who =
        match get_with_token(&format!("{server}/api/me"), &token, Duration::from_secs(30)).await {
            Ok((200, payload)) => text(&payload, "name"),
            _ => String::new(),
        };

    let path = token_path();
    write_token(&path, &token).unwrap_or_else(|err| die(err));
    println!("signed in as {who}");
    eprintln!("  token stored in {}", path.display());
}

/// Writes the token where the next command will look for it, readable by
/// nobody else: it is a bearer, so the file permissions are the whole of its
/// protection at rest.
pub fn write_token(path: &Path, token: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
    }
    std::fs::write(path, format!("{token}\n"))
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn logout() {
    let path = token_path();
    match std::fs::remove_file(&path) {
        Ok(()) => println!("signed out"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => println!("not signed in"),
        Err(err) => die(format!("could not remove {}: {err}", path.display())),
    }
}

pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn request_device_code(server: &str) -> Result<DeviceCode, String> {
    // No bearer: the terminal has no token yet, which is the whole reason it
    // is asking.
    let (status, payload) = post_json(
        &format!("{server}/api/auth/device"),
        &json!({}),
        "",
        Duration::from_secs(30),
    )
    .await?;
    let device_code = text(&payload, "device_code");
    if device_code.is_empty() {
        return Err(format!(
            "{server} returned {status}: {}",
            detail_of(&payload)
        ));
    }
    Ok(DeviceCode {
        device_code,
        user_code: text(&payload, "user_code"),
        verification_url: text(&payload, "verification_url"),
        expires_in: payload
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(600),
        interval: payload
            .get("interval")
            .and_then(Value::as_u64)
            .filter(|i| *i > 0)
            .unwrap_or(5),
    })
}

/// Waits for the code to be approved, at the interval the server asks for and
/// no faster. The deadline is the server's own expiry, so a code the server
/// has already forgotten is not polled for after it says so.
pub async fn poll_for_token(server: &str, code: &DeviceCode) -> Result<String, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in.max(60));
    let interval = Duration::from_secs(code.interval);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        eprint!(".");
        let Ok((_, reply)) = post_json(
            &format!("{server}/api/auth/device/token"),
            &json!({"device_code": code.device_code}),
            "",
            Duration::from_secs(30),
        )
        .await
        else {
            continue;
        };
        let token = text(&reply, "token");
        if !token.is_empty() {
            return Ok(token);
        }
        match text(&reply, "error").as_str() {
            "authorization_pending" | "" => {}
            "expired_token" => return Err("the code expired before it was approved".into()),
            other => return Err(format!("the server said: {other}")),
        }
    }
    Err("the code expired before it was approved".into())
}

/* ------------------------------------------------------------- publish */

pub async fn publish(file: &str, mut title: String, slug: String, server_flag: String) {
    let path = Path::new(file);
    let Ok(info) = std::fs::metadata(path) else {
        die(format!("file not found: {file}"))
    };
    if info.is_dir() {
        die(format!("file not found: {file}"));
    }
    let base_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    // What is stored is always HTML: the reader frames the document and
    // anchors comments into its text nodes. Markdown and typst are rendered
    // here, before they are uploaded.
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if extension != "html" && extension != "htm" && !is_markdown(file) && !is_typst(file) {
        die(format!(
            "{base_name} is not a document Komodoc can serve.\n\n  \
             It takes HTML, or markdown or typst, which it renders for you.\n  \
             From Quarto:\n    quarto render paper.qmd --to html -M embed-resources:true"
        ));
    }
    let raw =
        std::fs::read(path).unwrap_or_else(|err| die(format!("could not read {file}: {err}")));
    let config = Configuration::default();
    if raw.len() > config.max_html {
        die(format!(
            "document exceeds the {} MB limit",
            config.max_html / (1024 * 1024)
        ));
    }
    let Ok(html) = String::from_utf8(raw.clone()) else {
        die(format!("{base_name} is not valid UTF-8 text"))
    };
    // Kept beside the rendered document so it can be reopened in the editor.
    // Every format komodoc renders has one, HTML included: its renderer is the
    // identity, so an HTML document's source is the HTML it was published as.
    let (mut source, mut source_format) = (String::new(), String::new());

    if is_typst(file) {
        if title.is_empty() {
            // The first heading names the document, before the filename does.
            title = title_from_typst(&html);
        }
        let compiled = render_typst_document(path, &html, &title_or(&title, file));
        // Every diagnostic, where it is, and nothing uploaded if any of them
        // is an error: `publish` exists to make a document readable, and a
        // document that does not compile is not one.
        report(&compiled.diagnostics, &base_name);
        let errors = compiled.errors().count();
        let warnings = compiled.warnings().count();
        if compiled.page.is_none() {
            die(format!(
                "{base_name} did not compile ({})",
                counted(errors.max(1), "error")
            ))
        }
        eprintln!(
            "rendered {base_name} ({} KiB of typst{})",
            raw.len() / 1024,
            if warnings == 0 {
                String::new()
            } else {
                format!(", {}", counted(warnings, "warning"))
            }
        );
        source = html;
        source_format = "typst".to_string();
    } else if is_html(file) {
        if title.is_empty() {
            // What the document calls itself, which is what the landing page
            // has always shown and what the command line used to ignore.
            title = title_from_html(&html);
        }
        source = html.clone();
        source_format = "html".to_string();
    } else if is_markdown(file) {
        if title.is_empty() {
            title = title_from_markdown(&html);
        }
        eprintln!("read {base_name} ({} KiB of markdown)", raw.len() / 1024);
        source = html;
        source_format = "markdown".to_string();
    } else if !html.contains('<') {
        die(format!("{base_name} contains no HTML tags"));
    }

    let server = server_from(&server_flag);
    if title.is_empty() && !slug.is_empty() {
        // Publishing a revision: keep the title the document already has
        // rather than silently renaming it after the file on disk.
        if let Ok((200, existing)) = get_json(
            &format!("{server}/api/documents/{slug}"),
            Duration::from_secs(30),
        )
        .await
        {
            title = text(&existing, "title");
        }
    }
    if title.is_empty() {
        title = title_or("", file);
    }

    // stored_token, not require_token: a deployment whose publishers are
    // "anyone" takes documents with no sign-in, and one that does need an
    // account answers with its own message.
    let (status, document) = post_json(
        &format!("{server}/api/documents"),
        // The source, and nothing rendered from it: the server stores the
        // document and every browser that shows it renders it. The compile
        // above still happens, because `publish` exists to make a document
        // readable and a document that does not compile is not one -- but what
        // it produces is a check, not a payload.
        &json!({"title": title, "slug": slug, "source": source, "source_format": source_format}),
        &stored_token(),
        Duration::from_secs(300),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 201 {
        die(format!(
            "upload failed ({status}): {}",
            detail_of(&document)
        ));
    }

    let link = format!("{server}{}", text(&document, "url"));
    println!("{link}");
    if is_terminal_stdout() {
        eprintln!(
            "\nShare this link; anyone with it can comment, no account needed.\n\
             To publish a revision to the same link:\n  komodoc publish {file} --slug {}",
            text(&document, "slug")
        );
    }
}

/// Falls back to the filename, the way an untitled document is named.
pub fn title_or(title: &str, file: &str) -> String {
    if !title.trim().is_empty() {
        return title.to_string();
    }
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    stem.replace(['_', '-'], " ").trim().to_string()
}

/* ------------------------------------------------------------ listing */

pub async fn list_documents(server_flag: String) {
    let server = server_from(&server_flag);
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token(),
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
const SHORT_ID_MINIMUM: usize = 3;

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
pub async fn resolve_identifier(identifier: &str, server: &str) -> String {
    // A full slug needs no listing, and so no token: this is the path an
    // export from a link someone sent takes.
    if let Ok((200, _)) = get_json(
        &format!("{server}/api/documents/{identifier}"),
        Duration::from_secs(30),
    )
    .await
    {
        return identifier.to_string();
    }
    let (status, payload) = post_json(
        &format!("{server}/api/list"),
        &json!({}),
        &require_token(),
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

pub async fn comment_document(identifier: &str, server_flag: String) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server).await;
    open_url(&format!("{server}/docs/{slug}"));
}

/// `komodoc edit` opens a document in the reader, with its source beside it.
/// The editor is part of the reader rather than a program of its own, so this
/// is what it should be: a way to get to the right page from a short id.
pub async fn edit_document(identifier: &str, server_flag: String) {
    comment_document(identifier, server_flag).await;
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

/* --------------------------------------------------------------- sharing */

// Rights live on the document rather than in the server's flags: a document
// names its coauthors and its reviewers, and the flags are the ceiling it may
// not open past. These commands are that, on the command line.

/// `komodoc share c9k` with nothing else prints what the document says; with a
/// flag, changes it and prints the result.
#[allow(clippy::too_many_arguments)] // one flag per thing a share can change
pub async fn share_document(
    identifier: &str,
    server_flag: String,
    editor: String,
    commenter: String,
    link: String,
    label: String,
    until: String,
    visibility: String,
    revoke: String,
) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server).await;
    let target = format!("{server}/api/documents/{slug}/share");

    let mut change = json!({});
    if !editor.is_empty() {
        change["grant"] = json!({"login": editor, "role": "editor"});
    }
    if !commenter.is_empty() {
        if change.get("grant").is_some() {
            die("name one person at a time: --editor or --commenter");
        }
        change["grant"] = json!({"login": commenter, "role": "commenter"});
    }
    if !link.is_empty() {
        change["link"] = json!({"role": link, "label": label, "until": until});
    } else if !label.is_empty() || !until.is_empty() {
        die("--label and --until describe a link; pass --link commenter or --link editor");
    }
    if !visibility.is_empty() {
        change["visibility"] = json!(visibility);
    }
    if !revoke.is_empty() {
        change["revoke"] = json!(revoke);
    }

    // Nothing to change is a request to see what is there, which needs no
    // write and no confirmation.
    let asking = change.as_object().is_some_and(|fields| fields.is_empty());
    let (status, payload) = if asking {
        get_with_token(&target, &require_token(), Duration::from_secs(60))
            .await
            .unwrap_or_else(|err| die(err))
    } else {
        post_json(&target, &change, &require_token(), Duration::from_secs(60))
            .await
            .unwrap_or_else(|err| die(err))
    };
    if status != 200 {
        die(format!("share failed ({status}): {}", detail_of(&payload)));
    }

    // A new link's key is in this response and nowhere else, so it is printed
    // before anything that might scroll it away, and said to be the only time.
    if let Some(key) = payload.get("key").and_then(Value::as_str) {
        println!("{server}/docs/{slug}#k={key}");
        eprintln!("\n  This link is shown once and cannot be shown again.");
        eprintln!(
            "  Revoke it with:  komodoc share {identifier} --revoke {}\n",
            text(&payload, "key_id")
        );
    }
    print_sharing(&payload, &slug);
}

fn print_sharing(payload: &Value, slug: &str) {
    println!("{slug}  {}", text(payload, "visibility"));
    let owner = payload
        .get("owner")
        .map(|owner| text(owner, "login"))
        .unwrap_or_default();
    println!(
        "  owner       {}",
        if owner.is_empty() {
            "nobody in particular".to_string()
        } else {
            format!("@{owner}")
        }
    );
    for (field, role) in [("editors", "editor"), ("commenters", "commenter")] {
        for grant in payload
            .get(field)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let mut since = text(&grant, "since");
            since.truncate(10);
            println!("  {role:<11} @{}  {since}", text(&grant, "login"));
        }
    }
    for link in payload
        .get("links")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let mut until = text(&link, "until");
        until.truncate(10);
        let state = if link.get("expired") == Some(&Value::Bool(true)) {
            "expired".to_string()
        } else if until.is_empty() {
            "no expiry".to_string()
        } else {
            format!("until {until}")
        };
        let label = text(&link, "label");
        println!(
            "  link        {}  {}  {state}{}",
            text(&link, "id"),
            text(&link, "role"),
            if label.is_empty() {
                String::new()
            } else {
                format!("  {label:?}")
            }
        );
    }
}

/// Hands a document to somebody else: its history, its comments and its quota
/// go with it. Confirmed like `destroy`, because it is the one other change
/// that leaves the caller with nothing.
pub async fn transfer_document(identifier: &str, to: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server).await;
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
        &require_token(),
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

/* ------------------------------------------------------------- destroy */

pub async fn destroy_document(identifier: &str, server_flag: String, yes: bool) {
    let server = server_from(&server_flag);
    // The same identifier `comment` and `export` take. The confirmation
    // below still asks for the whole slug: this is the one irreversible
    // command, and a three-character answer is too easy to give.
    let slug = resolve_identifier(identifier, &server).await;
    let (status, document) = get_json(
        &format!("{server}/api/documents/{slug}"),
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
        &require_token(),
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("delete failed ({status}): {}", detail_of(&payload)));
    }
    println!("deleted {slug}");
}
