//! Suggestions from a terminal: proposing a replacement for a passage, and
//! accepting or rejecting one.

use super::*;

/// A captured document revision is the canonical lowercase SHA-256 tree
/// digest emitted by the snapshot endpoint. Rejecting malformed values at the
/// CLI boundary keeps an assistant from accidentally posting an empty or
/// ambiguous revision.
pub(crate) fn revision_value(value: &str) -> Result<String, String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(value.to_string())
    } else {
        Err("revision must be a 64-character lowercase SHA-256 digest".into())
    }
}

pub(crate) async fn post_assistant_json(
    target: &str,
    payload: &Value,
    credentials: &Credentials,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let body = serde_json::to_vec(payload)
        .map_err(|err| format!("could not encode the request: {err}"))?;
    let owned = credentials.headers();
    let mut request = crate::http::client()
        .post(target)
        .timeout(timeout)
        .header("content-type", "application/json")
        .header("x-librepaper-automation", "1");
    for (name, value) in &owned {
        request = request.header(*name, value);
    }
    let response = request
        .body(body)
        .send()
        .await
        .map_err(|err| format!("POST {target}: {err}"))?;
    let status = response.status().as_u16();
    let raw = response
        .bytes()
        .await
        .map_err(|err| format!("POST {target}: {err}"))?;
    Ok((
        status,
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| json!({"error": String::from_utf8_lossy(&raw)})),
    ))
}

/// The refusal text a suggestion route answers with, in `message` rather than
/// `error`: everything else the CLI reads uses `detail_of`, which falls back
/// to the whole payload when `message` is what is actually there.
pub(super) fn suggestion_message(payload: &Value) -> String {
    match payload.get("message").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => detail_of(payload),
    }
}

/// A source anchor into a passage found by `locate_passage`: what `suggest`
/// posts as both the top-level anchor fields (for the rendered quotation, on
/// a single-file document where the two coincide) and the `source` object.
#[derive(Debug)]
pub(crate) struct Anchor {
    pub(crate) exact: String,
    pub(crate) prefix: String,
    pub(crate) suffix: String,
    pub(crate) position: i64,
}

fn target_for(
    identifier: &str,
    server: Option<String>,
    key_flag: &str,
) -> (String, String, String) {
    let supplied_key = link_key(key_flag);
    if identifier.starts_with("http://") || identifier.starts_with("https://") {
        if let Ok(link) = crate::cli::peer::DocumentLink::parse(identifier, "") {
            let key = if supplied_key.is_empty() {
                link_key(identifier)
            } else {
                supplied_key
            };
            return (link.server().to_string(), key, link.slug().to_string());
        }
    }
    (server_or_die(server), supplied_key, identifier.to_string())
}

/// Finds `find` in `source`, refusing when it occurs zero or more than once so
/// the anchor is never ambiguous, and builds the anchor around it: up to 32
/// characters of prefix and suffix and the UTF-16 position `wasm_helpers::text`
/// and the browser both anchor by. Pure and synchronous so it is testable
/// without a server.
pub(crate) fn locate_passage(source: &str, path: &str, find: &str) -> Result<Anchor, String> {
    let matches: Vec<usize> = source.match_indices(find).map(|(at, _)| at).collect();
    let at = match matches.as_slice() {
        [at] => *at,
        [] => return Err(format!("{find:?} does not occur in {path}")),
        many => {
            return Err(format!(
                "{find:?} occurs {} times in {path}; give more context",
                many.len()
            ))
        }
    };
    let before = &source[..at];
    let after = &source[at + find.len()..];
    let prefix: String = before
        .chars()
        .rev()
        .take(32)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    let suffix: String = after.chars().take(32).collect();
    let position = before.encode_utf16().count() as i64;
    Ok(Anchor {
        exact: find.to_string(),
        prefix,
        suffix,
        position,
    })
}

/// Proposes a replacement for a passage: finds `find` in the named file (the
/// main file of the live document by default), builds a source anchor
/// around it, and posts a suggestion comment. Prints the new comment's id.
#[allow(clippy::too_many_arguments)]
pub async fn suggest_passage(
    identifier: &str,
    find: &str,
    replace: &str,
    path: String,
    note: String,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    suggest_passage_with_revision(
        identifier,
        find,
        replace,
        path,
        note,
        String::new(),
        server,
        token,
        key,
    )
    .await;
}

/// The revision-aware form used by the assistant context. The old
/// `suggest_passage` signature remains available to scripts and tests.
#[allow(clippy::too_many_arguments)]
pub async fn suggest_passage_with_revision(
    identifier: &str,
    find: &str,
    replace: &str,
    path: String,
    note: String,
    revision: String,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    if !revision.is_empty() {
        revision_value(&revision).unwrap_or_else(|err| die(err));
    }
    let path = if path.is_empty()
        && (identifier.starts_with("http://") || identifier.starts_with("https://"))
    {
        crate::cli::peer::DocumentLink::parse(identifier, "")
            .map(|link| link.path().to_string())
            .unwrap_or_default()
    } else {
        path
    };
    let (server, key, slug) = target_for(identifier, server, &key);
    let slug = resolve_identifier(&slug, &server, &key, token.as_deref()).await;
    let credentials = Credentials::new(&stored_token_for(&server, token.as_deref()), &key);

    let (status, checkpoint) = get_as(
        &format!("{server}/api/documents/{slug}/snapshot"),
        &credentials,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "snapshot failed ({status}): {}",
            detail_of(&checkpoint)
        ));
    }
    let main = text(&checkpoint, "main");
    let target_path = if path.is_empty() { main } else { path };
    let texts = checkpoint.get("texts").and_then(Value::as_object);
    let Some(source) = texts
        .and_then(|texts| texts.get(&target_path))
        .and_then(Value::as_str)
    else {
        die(format!("{target_path:?} is not a file in {slug}"));
    };
    let anchor = match locate_passage(source, &target_path, find) {
        Ok(anchor) => anchor,
        Err(message) => die(message),
    };

    let mut request = json!({
        "type": "comment",
        "motivation": "editing",
        "exact": anchor.exact,
        "prefix": anchor.prefix,
        "suffix": anchor.suffix,
        "position": anchor.position,
        "source": {
            "path": target_path,
            "exact": anchor.exact,
            "prefix": anchor.prefix,
            "suffix": anchor.suffix,
            "position": anchor.position,
        },
        "proposed": replace,
        "body": note,
    });
    if !revision.is_empty() {
        request["revision"] = json!(revision);
    }
    let (status, payload) = post_assistant_json(
        &format!("{server}/api/documents/{slug}/comments"),
        &request,
        &credentials,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "suggest failed ({status}): {}",
            suggestion_message(&payload)
        ));
    }
    println!("{}", text(&payload["comment"], "id"));
}

/// Post one source anchor copied from an assistant context. The source value
/// is retained as a JSON value so fields and their spelling survive the CLI
/// boundary unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn suggest_anchor(
    identifier: &str,
    anchor_json: &str,
    replace: &str,
    note: String,
    revision: String,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    revision_value(&revision).unwrap_or_else(|err| die(err));
    let anchor: Value = serde_json::from_str(anchor_json)
        .unwrap_or_else(|err| die(format!("invalid --anchor JSON: {err}")));
    if !anchor.is_object() {
        die("--anchor must be a JSON object");
    }
    for field in ["path", "exact", "prefix", "suffix", "position"] {
        if anchor.get(field).is_none() {
            die(format!("--anchor is missing {field}"));
        }
    }
    let (server, key, slug) = target_for(identifier, server, &key);
    let slug = resolve_identifier(&slug, &server, &key, token.as_deref()).await;
    let mut request = json!({
        "type": "comment",
        "motivation": "editing",
        "exact": anchor["exact"],
        "prefix": anchor["prefix"],
        "suffix": anchor["suffix"],
        "position": anchor["position"],
        "source": anchor,
        "proposed": replace,
        "body": note,
    });
    if !revision.is_empty() {
        request["revision"] = json!(revision);
    }
    let credentials = Credentials::new(&stored_token_for(&server, token.as_deref()), &key);
    let (status, payload) = post_assistant_json(
        &format!("{server}/api/documents/{slug}/comments"),
        &request,
        &credentials,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "suggest failed ({status}): {}",
            suggestion_message(&payload)
        ));
    }
    println!("{}", text(&payload["comment"], "id"));
}

/// Post a batch of anchored proposals. The input accepts either an array of
/// items or `{\"revision\": ..., \"items\": [...]}`. A separately supplied
/// `--revision` must agree with any revision recorded in the file.
pub async fn suggest_batch(
    identifier: &str,
    file: &str,
    revision: String,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    let raw = std::fs::read_to_string(file)
        .unwrap_or_else(|err| die(format!("could not read batch {file}: {err}")));
    let input: Value =
        serde_json::from_str(&raw).unwrap_or_else(|err| die(format!("invalid batch JSON: {err}")));
    let (file_revision, items) = match input {
        Value::Array(items) => (String::new(), items),
        Value::Object(mut object) => {
            let revision = object
                .remove("revision")
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            let items = object.remove("items").unwrap_or(Value::Null);
            let Some(items) = items.as_array() else {
                die("batch JSON must contain an items array");
            };
            (revision, items.clone())
        }
        _ => die("batch JSON must be an array or an object with items"),
    };
    if items.is_empty() {
        die("batch must contain at least one item");
    }
    if items.len() > 100 {
        die("batch cannot contain more than 100 items");
    }
    if !revision.is_empty() && !file_revision.is_empty() && revision != file_revision {
        die("--revision disagrees with the revision in the batch file");
    }
    let revision = if revision.is_empty() {
        file_revision
    } else {
        revision
    };
    if revision.is_empty() {
        die("a revision is required for a batch (use --revision or include revision in the file)");
    }
    revision_value(&revision).unwrap_or_else(|err| die(err));
    let mut checked = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let Some(object) = item.as_object() else {
            die(format!("batch item {index} must be an object"));
        };
        let Some(anchor) = object.get("anchor") else {
            die(format!("batch item {index} is missing anchor"));
        };
        if !anchor.is_object() {
            die(format!("batch item {index} anchor must be an object"));
        }
        if object.get("proposed").and_then(Value::as_str).is_none() {
            die(format!("batch item {index} is missing proposed text"));
        }
        let mut item = json!({
            "anchor": anchor,
            "proposed": object["proposed"],
        });
        if let Some(body) = object.get("body") {
            item["body"] = body.clone();
        }
        checked.push(item);
    }
    let (server, key, slug) = target_for(identifier, server, &key);
    let slug = resolve_identifier(&slug, &server, &key, token.as_deref()).await;
    let credentials = Credentials::new(&stored_token_for(&server, token.as_deref()), &key);
    let (status, payload) = post_assistant_json(
        &format!("{server}/api/documents/{slug}/suggestions"),
        &json!({"revision": revision, "items": checked}),
        &credentials,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "suggest batch failed ({status}): {}",
            suggestion_message(&payload)
        ));
    }
    println!(
        "{}",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into())
    );
}

/// Why deciding a suggestion did not go through: refused outright, in which
/// case `accept` and `reject` behave like every other command and die with
/// the server's message, or stale, which `accept` alone tells apart with its
/// own exit code so a script can open the merge editor instead of retrying.
#[derive(Debug)]
pub(crate) enum Decision {
    Refused(String),
    Stale(String),
}

/// The exit code `accept` leaves an agent with: 3 for stale, so a script can
/// tell it apart from every other refusal (1, the same as any other `die`)
/// and open the merge editor instead of retrying blindly.
pub(crate) fn exit_code_for(decision: &Decision) -> i32 {
    match decision {
        Decision::Refused(_) => 1,
        Decision::Stale(_) => 3,
    }
}

/// Posts an `accept` or `reject` and sorts the reply into a success payload
/// or one of the two ways it can fail. Shared by `accept_suggestion` and
/// `reject_suggestion` so both speak the identical request the spec gives.
pub(crate) async fn decide_suggestion(
    server: &str,
    slug: &str,
    credentials: &Credentials,
    comment_id: &str,
    kind: &str,
) -> Result<Value, Decision> {
    let (status, payload) = post_json_as(
        &format!("{server}/api/documents/{slug}/comments"),
        &json!({"type": kind, "comment_id": comment_id, "request_id": new_id()}),
        credentials,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status == 200 {
        return Ok(payload);
    }
    if payload.get("stale") == Some(&Value::Bool(true)) {
        return Err(Decision::Stale(suggestion_message(&payload)));
    }
    Err(Decision::Refused(suggestion_message(&payload)))
}

/// Applies a suggestion to the live document. Prints the outcome and, on
/// success, the checkpoint the acceptance was recorded in. A stale
/// suggestion -- the passage moved out from under it, even against a merge --
/// exits 3 rather than 1, so a script can tell it apart from an ordinary
/// refusal and open the merge editor instead of retrying blindly.
pub async fn accept_suggestion(
    identifier: &str,
    comment_id: &str,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    let server = server_or_die(server);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key, token.as_deref()).await;
    let credentials = Credentials::new(&stored_token_for(&server, token.as_deref()), &key);
    match decide_suggestion(&server, &slug, &credentials, comment_id, "accept").await {
        Ok(payload) => {
            let sha = text(&payload, "resolved_in");
            let short = sha.chars().take(7).collect::<String>();
            if payload.get("noop") == Some(&Value::Bool(true)) {
                println!("already accepted, resolved in {short}");
            } else {
                println!("accepted, resolved in {short}");
            }
        }
        Err(decision) => {
            let code = exit_code_for(&decision);
            let message = match decision {
                Decision::Refused(message) | Decision::Stale(message) => message,
            };
            eprintln!("error: {message}");
            std::process::exit(code);
        }
    }
}

/// Resolves a suggestion without applying it. Prints the outcome.
pub async fn reject_suggestion(
    identifier: &str,
    comment_id: &str,
    server: Option<String>,
    token: Option<String>,
    key: String,
) {
    let server = server_or_die(server);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key, token.as_deref()).await;
    let credentials = Credentials::new(&stored_token_for(&server, token.as_deref()), &key);
    match decide_suggestion(&server, &slug, &credentials, comment_id, "reject").await {
        Ok(_) => println!("rejected"),
        // A reject never goes stale -- it only marks the comment, and never
        // touches the passage -- but the branch is kept exhaustive rather
        // than assumed away, in case the server ever starts sending one.
        Err(Decision::Refused(message)) => die(message),
        Err(Decision::Stale(message)) => die(message),
    }
}
