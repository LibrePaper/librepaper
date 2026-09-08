//! The timeline from a terminal: listing checkpoints, diffing two, restoring
//! to one, and naming one.

use super::*;

/// What this document used to say, and when.
///
/// One line per checkpoint, oldest first, which is the order the manifest is
/// in and the order a history reads in. The newest is marked, because "where
/// am I" is the first question anybody asks of a list like this, and a label
/// is printed as it was given: it is somebody's own words about a moment.
pub async fn history_document(identifier: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let checkpoints = manifest_for(
        &server,
        &slug,
        &Credentials::new(&stored_token_for(&server), &key),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if checkpoints.is_empty() {
        println!("no checkpoints yet");
        return;
    }
    println!("sha      at                    by                  why      label");
    let last = checkpoints.len() - 1;
    for (n, point) in checkpoints.iter().enumerate() {
        let sha = text(point, "sha");
        // Seven characters, which is what git prints and what the panel and
        // `komodoc label` both accept.
        let short = sha.chars().take(7).collect::<String>();
        // The stored time is ISO 8601 in UTC; a table reads better with the
        // T and the Z taken out and nothing else changed.
        let at = text(point, "at")
            .replace('T', " ")
            .trim_end_matches('Z')
            .to_string();
        let label = text(point, "label");
        // The newest carries a star, because "where am I" is the first
        // question anybody asks of a list like this.
        let mark = match (n == last, label.is_empty()) {
            (false, _) => "",
            (true, true) => "*",
            (true, false) => "  *",
        };
        println!(
            "{short:<7}  {at:<19}  {:<18}  {:<7}  {label}{mark}",
            text(point, "by"),
            text(point, "why"),
        );
    }
}

/// Read failures must remain failures; an outage is not an empty timeline.
pub(super) async fn manifest_for(
    server: &str,
    slug: &str,
    credentials: &Credentials,
) -> Result<Vec<Value>, String> {
    let (status, payload) = get_as(
        &format!("{server}/api/documents/{slug}/history"),
        credentials,
        Duration::from_secs(60),
    )
    .await?;
    if status != 200 {
        return Err(format!(
            "history failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    payload
        .get("checkpoints")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "invalid history response: missing checkpoints".into())
}

pub(super) fn checkpoint_sha(
    checkpoints: &[Value],
    slug: &str,
    requested: &str,
) -> Result<String, String> {
    if requested.is_empty() {
        return Err("a checkpoint SHA is required".into());
    }
    let matching: std::collections::HashSet<String> = checkpoints
        .iter()
        .map(|point| text(point, "sha"))
        .filter(|sha| sha.starts_with(requested))
        .collect();
    match matching.len() {
        0 => Err(format!("no checkpoint of {slug} starts with {requested:?}")),
        1 => Ok(matching
            .into_iter()
            .next()
            .expect("one matching checkpoint")),
        many => Err(format!(
            "{requested:?} names {many} checkpoints of {slug}; give more of the digest"
        )),
    }
}

async fn checkpoint_by_sha(
    server: &str,
    slug: &str,
    sha: &str,
    credentials: &Credentials,
) -> Value {
    let (status, checkpoint) = get_as(
        &format!("{server}/api/documents/{slug}/history/{sha}"),
        credentials,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "checkpoint failed ({status}): {}",
            detail_of(&checkpoint)
        ));
    }
    checkpoint
}

/// Prints a source diff between two checkpoints. It uses the same checkpoint
/// objects as the browser, and a compact unified representation suitable for
/// a terminal or a pipe.
pub async fn diff_document(
    identifier: &str,
    from: &str,
    to: &str,
    server_flag: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
    let manifest = manifest_for(&server, &slug, &credentials)
        .await
        .unwrap_or_else(|err| die(err));
    let from = checkpoint_sha(&manifest, &slug, from).unwrap_or_else(|err| die(err));
    let to = checkpoint_sha(&manifest, &slug, to).unwrap_or_else(|err| die(err));
    let old = checkpoint_by_sha(&server, &slug, &from, &credentials).await;
    let new = checkpoint_by_sha(&server, &slug, &to, &credentials).await;
    let old_files = old.get("texts").and_then(Value::as_object);
    let new_files = new.get("texts").and_then(Value::as_object);
    let old_entries = old.get("files").and_then(Value::as_object);
    let new_entries = new.get("files").and_then(Value::as_object);
    let mut paths: Vec<String> = old_files
        .into_iter()
        .flat_map(|files| files.keys().cloned())
        .chain(
            new_files
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .chain(
            old_entries
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .chain(
            new_entries
                .into_iter()
                .flat_map(|files| files.keys().cloned()),
        )
        .collect();
    paths.sort();
    paths.dedup();
    let old_sha = text(&old, "sha");
    let new_sha = text(&new, "sha");
    let old_main = text(&old, "main");
    let new_main = text(&new, "main");
    if old_main != new_main {
        println!("# main file changed: {old_main} -> {new_main}");
    }
    for path in paths {
        let before = old_files
            .and_then(|files| files.get(&path))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let after = new_files
            .and_then(|files| files.get(&path))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let old_entry = old_entries.and_then(|files| files.get(&path));
        let new_entry = new_entries.and_then(|files| files.get(&path));
        let old_kind = old_entry
            .and_then(|entry| entry.get("kind"))
            .and_then(Value::as_str);
        let new_kind = new_entry
            .and_then(|entry| entry.get("kind"))
            .and_then(Value::as_str);
        let old_present =
            old_entry.is_some() || old_files.is_some_and(|files| files.contains_key(&path));
        let new_present =
            new_entry.is_some() || new_files.is_some_and(|files| files.contains_key(&path));
        if old_kind == Some("asset") || new_kind == Some("asset") {
            if old_kind != new_kind
                || old_entry.and_then(|entry| entry.get("sha"))
                    != new_entry.and_then(|entry| entry.get("sha"))
            {
                println!("Binary files a/{path}@{old_sha} and b/{path}@{new_sha} differ");
            }
            continue;
        }
        if old_kind != new_kind && old_kind.is_some() && new_kind.is_some() {
            println!("# file kind changed: {path} ({old_kind:?} -> {new_kind:?})");
            continue;
        }
        if before == after && old_present == new_present {
            continue;
        }
        print!(
            "{}",
            unified_source_diff(
                &path,
                before,
                after,
                &old_sha,
                &new_sha,
                old_present,
                new_present,
            )
        );
        if before.is_empty() && after.is_empty() && old_present != new_present {
            println!(
                "# empty file {}: {}",
                path,
                if new_present { "added" } else { "deleted" }
            );
        }
    }
}

pub(super) fn unified_source_diff(
    path: &str,
    before: &str,
    after: &str,
    old_sha: &str,
    new_sha: &str,
    old_present: bool,
    new_present: bool,
) -> String {
    let old = diff_lines(before);
    let new = diff_lines(after);
    let ops = line_ops(&old, &new);
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(index, op)| (!matches!(op, LineOp::Equal(_))).then_some(index))
        .collect();
    let old_label = if !old_present {
        "/dev/null".to_string()
    } else {
        format!("a/{path}@{old_sha}")
    };
    let new_label = if !new_present {
        "/dev/null".to_string()
    } else {
        format!("b/{path}@{new_sha}")
    };
    let mut out = format!("--- {old_label}\n+++ {new_label}\n");
    let mut regions: Vec<(usize, usize)> = Vec::new();
    for index in changed {
        let start = index.saturating_sub(3);
        let end = (index + 4).min(ops.len());
        match regions.last_mut() {
            Some((_, previous_end)) if start <= *previous_end => {
                *previous_end = end.max(*previous_end)
            }
            _ => regions.push((start, end)),
        }
    }
    for (start, end) in regions {
        let old_before = ops[..start]
            .iter()
            .filter(|op| !matches!(op, LineOp::Insert(_)))
            .count();
        let new_before = ops[..start]
            .iter()
            .filter(|op| !matches!(op, LineOp::Delete(_)))
            .count();
        let old_count = ops[start..end]
            .iter()
            .filter(|op| !matches!(op, LineOp::Insert(_)))
            .count();
        let new_count = ops[start..end]
            .iter()
            .filter(|op| !matches!(op, LineOp::Delete(_)))
            .count();
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            unified_range(old_before, old_count),
            unified_range(new_before, new_count)
        ));
        for op in &ops[start..end] {
            let (prefix, line) = match op {
                LineOp::Equal(line) => (' ', line),
                LineOp::Delete(line) => ('-', line),
                LineOp::Insert(line) => ('+', line),
            };
            out.push(prefix);
            out.push_str(&line.text);
            out.push('\n');
            if !line.newline {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
    out
}

#[derive(Clone)]
pub(super) struct DiffLine {
    pub(super) text: String,
    pub(super) newline: bool,
}

pub(super) enum LineOp {
    Equal(DiffLine),
    Delete(DiffLine),
    Insert(DiffLine),
}

pub(super) fn diff_lines(source: &str) -> Vec<DiffLine> {
    if source.is_empty() {
        return Vec::new();
    }
    source
        .split_inclusive('\n')
        .map(|line| {
            let newline = line.ends_with('\n');
            DiffLine {
                text: line.trim_end_matches('\n').to_string(),
                newline,
            }
        })
        .collect()
}

pub(super) fn line_ops(old: &[DiffLine], new: &[DiffLine]) -> Vec<LineOp> {
    let mut prefix = 0;
    while prefix < old.len() && prefix < new.len() && same_line(&old[prefix], &new[prefix]) {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < old.len() - prefix
        && suffix < new.len() - prefix
        && same_line(&old[old.len() - 1 - suffix], &new[new.len() - 1 - suffix])
    {
        suffix += 1;
    }
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];
    let mut ops = old[..prefix]
        .iter()
        .cloned()
        .map(LineOp::Equal)
        .collect::<Vec<_>>();
    // Generated sources can have many thousands of lines. A bounded fallback
    // remains a valid unified diff and avoids allocating a quadratic matrix
    // when the changed region itself is too large for an exact LCS.
    if old_mid.len().saturating_mul(new_mid.len()) > 4_000_000 {
        ops.extend(old_mid.iter().cloned().map(LineOp::Delete));
        ops.extend(new_mid.iter().cloned().map(LineOp::Insert));
    } else {
        ops.extend(line_ops_middle(old_mid, new_mid));
    }
    ops.extend(new[new.len() - suffix..].iter().cloned().map(LineOp::Equal));
    ops
}

pub(super) fn same_line(old: &DiffLine, new: &DiffLine) -> bool {
    old.text == new.text && old.newline == new.newline
}

pub(super) fn line_ops_middle(old: &[DiffLine], new: &[DiffLine]) -> Vec<LineOp> {
    let columns = new.len() + 1;
    let mut common = vec![0usize; (old.len() + 1) * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i * columns + j] =
                if old[i].text == new[j].text && old[i].newline == new[j].newline {
                    common[(i + 1) * columns + j + 1] + 1
                } else {
                    common[(i + 1) * columns + j].max(common[i * columns + j + 1])
                };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len()
            && j < new.len()
            && old[i].text == new[j].text
            && old[i].newline == new[j].newline
        {
            ops.push(LineOp::Equal(old[i].clone()));
            i += 1;
            j += 1;
        } else if j == new.len()
            || (i < old.len() && common[(i + 1) * columns + j] >= common[i * columns + j + 1])
        {
            ops.push(LineOp::Delete(old[i].clone()));
            i += 1;
        } else {
            ops.push(LineOp::Insert(new[j].clone()));
            j += 1;
        }
    }
    ops
}

pub(super) fn unified_range(start: usize, count: usize) -> String {
    if count == 0 {
        format!("{start},0")
    } else if count == 1 {
        (start + 1).to_string()
    } else {
        format!("{},{}", start + 1, count)
    }
}

/// Restores a checkpoint through the editor-only API. The key may be either a
/// raw share key or the complete link copied from a browser.
pub async fn restore_document(identifier: &str, sha: &str, server_flag: String, key: String) {
    let server = server_from(&server_flag);
    let key = link_key(&key);
    let slug = resolve_identifier(identifier, &server, &key).await;
    let credentials = Credentials::new(&stored_token_for(&server), &key);
    let (status, document) = get_as(
        &format!("{server}/api/documents/{slug}"),
        &credentials,
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("cannot restore {slug}: {}", detail_of(&document)));
    }
    let role = text(&document, "role");
    if document.get("can_edit") != Some(&Value::Bool(true))
        && !matches!(role.as_str(), "editor" | "owner")
    {
        die(format!(
            "you may read {slug} but not edit it; restore requires an editor"
        ));
    }
    let (status, payload) = post_json_as(
        &format!("{server}/api/documents/{slug}/restore"),
        &json!({"sha": sha}),
        &credentials,
        Duration::from_secs(120),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "restore failed ({status}): {}",
            detail_of(&payload)
        ));
    }
    let restored = text(&payload, "sha");
    println!(
        "restored {slug} to {}",
        restored.chars().take(7).collect::<String>()
    );
}

/// Names a checkpoint, or takes its name away with an empty one.
///
/// The SHA may be the short form the table prints, which is resolved against
/// the manifest here rather than on the server: the server takes one name for
/// a checkpoint, its whole digest, and a prefix that matched two of them would
/// be a thing for a person to disambiguate rather than for a route to guess.
pub async fn label_checkpoint(identifier: &str, sha: &str, label: String, server_flag: String) {
    let server = server_from(&server_flag);
    let slug = resolve_identifier(identifier, &server, "").await;
    let token = require_token_for(&server);
    let checkpoints = manifest_for(&server, &slug, &Credentials::token(&token))
        .await
        .unwrap_or_else(|err| die(err));
    let full = checkpoint_sha(&checkpoints, &slug, sha).unwrap_or_else(|err| die(err));

    let (status, payload) = crate::http::patch_json(
        &format!("{server}/api/documents/{slug}/history/{full}"),
        &json!({"label": label}),
        &token,
        Duration::from_secs(60),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!("label failed ({status}): {}", detail_of(&payload)));
    }
    let given = text(&payload, "label");
    let short = full.chars().take(7).collect::<String>();
    if given.is_empty() {
        println!("{short}  unnamed");
    } else {
        println!("{short}  {given}");
    }
}

#[cfg(test)]
mod diff_tests {
    use super::{unified_range, unified_source_diff};

    #[test]
    fn zero_line_ranges_keep_the_zero_count() {
        assert_eq!(unified_range(0, 0), "0,0");
        assert_eq!(unified_range(4, 0), "4,0");
    }

    #[test]
    fn added_and_deleted_files_have_valid_unified_ranges() {
        let added = unified_source_diff("new.md", "", "added\n", "old", "new", false, true);
        assert!(added.contains("--- /dev/null\n+++ b/new.md@new\n"));
        assert!(added.contains("@@ -0,0 +1 @@"), "{added}");

        let deleted = unified_source_diff("gone.md", "removed\n", "", "old", "new", true, false);
        assert!(deleted.contains("--- a/gone.md@old\n+++ /dev/null\n"));
        assert!(deleted.contains("@@ -1 +0,0 @@"), "{deleted}");
    }

    #[test]
    fn empty_file_membership_has_headers_without_a_fake_hunk() {
        let added = unified_source_diff("empty.md", "", "", "old", "new", false, true);
        assert!(added.contains("--- /dev/null\n+++ b/empty.md@new\n"));
        assert!(!added.contains("@@"));
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;
    #[tokio::test]
    async fn unavailable_history_is_not_an_empty_manifest() {
        use axum::{http::StatusCode, routing::get};
        let app = axum::Router::new().route(
            "/api/documents/paper/history",
            get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = manifest_for(&server, "paper", &Credentials::default())
            .await
            .unwrap_err();
        assert!(error.contains("503"), "{error}");
        task.abort();
    }
}
