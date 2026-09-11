//! History lookups shared by annotation export.

use super::*;

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
