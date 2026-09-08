//! Suggestions from a terminal: proposing a replacement for a passage, and
//! accepting or rejecting one.

use super::*;

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

/// Finds `find` in `source`, refusing when it occurs zero or more than once
/// so the anchor is never ambiguous, and builds the anchor around it: up to
/// 32 characters of prefix and suffix and the UTF-16 position `komodoc_text`
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
pub async fn suggest_passage(
    identifier: &str,
    find: &str,
    replace: &str,
    path: String,
    note: String,
    server_flag: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);

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

    let (status, payload) = post_json_as(
        &format!("{server}/api/documents/{slug}/comments"),
        &json!({
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
        }),
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
    server_flag: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
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
    server_flag: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
    match decide_suggestion(&server, &slug, &credentials, comment_id, "reject").await {
        Ok(_) => println!("rejected"),
        // A reject never goes stale -- it only marks the comment, and never
        // touches the passage -- but the branch is kept exhaustive rather
        // than assumed away, in case the server ever starts sending one.
        Err(Decision::Refused(message)) => die(message),
        Err(Decision::Stale(message)) => die(message),
    }
}
