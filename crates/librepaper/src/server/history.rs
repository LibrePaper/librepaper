//! The timeline routes: the manifest, one label, naming it, and restoring the
//! document to it.
//!
//! `document_versions` and `document_checkpoints` are gone (§8.2, §12): the
//! timeline is `document_labels`, a name and a moment -- vector, frontier and
//! projection digest -- with nothing eagerly rendered or archived beside it.
//! A label's content is read the same way the live head is, by asking the
//! sequencer to project at a frontier (§4.4); there is no separate "ready"
//! state to poll before that answer exists. What *is* produced on request is
//! a plain-source archive for export (§8.5), which `handle_label_read` asks
//! for on `?archive=1` rather than assuming a client wants one on every read.

use super::*;

/// The longest a label's name may be, in characters. Long enough for
/// "submitted after review, second round" and short enough that the history
/// panel is a list of names rather than of paragraphs.
pub(super) const MAX_LABEL: usize = 120;

fn is_label_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

/// The prefix that spells a comment's own frontier rather than a named
/// label, shared with `web/src/lib/moment.js`'s `MOMENT`. A comment made
/// against the live draft has no label to fetch by (nothing writes a source
/// archive for a comment, §7.3), but its `OriginalAnchor.frontier` is a
/// position `with_fork_at` can reconstruct directly (§2.1, §8.2), so the
/// same `GET .../history/{sha}` route answers it too rather than needing a
/// second endpoint and a second client branch.
const MOMENT: &str = "frontier:";

/// The naming metadata a label carries and a comment's own frontier does
/// not: who wrote it, when, and under what name. Grouped so `projection_wire`
/// takes one bundle of "what this moment is called" alongside the moment
/// itself, rather than four positional strings a caller could transpose.
struct MomentMetadata<'a> {
    sha: &'a str,
    at: &'a str,
    by: &'a str,
    label: Option<&'a str>,
    reason: &'a str,
}

/// The JSON shape both a label and a comment's own frontier answer in, so a
/// client reading a historical passage does not need to know which one it
/// asked for.
fn projection_wire(
    metadata: MomentMetadata<'_>,
    storage_id: &str,
    source_format: &str,
    projected: &librepaper_document_core::Projected,
) -> Value {
    let MomentMetadata {
        sha,
        at,
        by,
        label,
        reason,
    } = metadata;
    let texts: HashMap<&str, &str> = projected
        .projection
        .files
        .iter()
        .filter(|(_, entry)| entry.kind == "text")
        .filter_map(|(path, _)| {
            projected
                .texts
                .get(path)
                .map(|body| (path.as_str(), body.as_str()))
        })
        .collect();
    json!({
        "sha": sha,
        "tree_sha": projected.projection.digest(),
        "storage_id": storage_id,
        "at": at,
        "by": by,
        "label": label,
        "reason": reason,
        "source_format": source_format,
        "main": projected.projection.main,
        "files": projected.projection.files,
        "texts": texts,
    })
}

/// A command's request id, cast to the number a batch's `client_seq` column
/// holds (§7.3). Nothing compares this to another peer's own sequence, so
/// the exact mapping does not matter -- only that it is stable across a
/// retry that reuses the same request id.
fn client_seq_of(id: uuid::Uuid) -> i64 {
    i64::from_be_bytes(id.as_bytes()[..8].try_into().expect("a uuid is 16 bytes"))
}

/// Names the current head, or the head a flush is about to produce, as a
/// label. No precondition (§7.1: "label | none") and no edit -- but running
/// it through the sequencer still forces a flush of whatever is buffered, so
/// "current" always names a real, durable row rather than a promise of one.
pub(super) struct Label {
    pub request_id: uuid::Uuid,
    pub document_id: uuid::Uuid,
    pub catalog: std::sync::Arc<crate::storage::postgres::PostgresCatalog>,
    /// "label" for a name a caller typed, or the producing command's own
    /// name when a label is a byproduct of something else (§7.2). This
    /// command is only ever used for the former.
    pub reason: String,
    pub label: Option<String>,
    pub author_account_id: Option<uuid::Uuid>,
    pub author_label: String,
}

impl crate::log::Command for Label {
    type Output = crate::storage::postgres::LabelRecord;

    fn name(&self) -> &'static str {
        "label"
    }

    fn evaluate(
        &mut self,
        _head: &crate::log::Head<'_>,
    ) -> std::result::Result<Option<crate::log::PreparedSource>, crate::log::CommandError> {
        Ok(None)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a crate::log::Evidence,
    ) -> futures_util::future::BoxFuture<
        'a,
        std::result::Result<Self::Output, crate::log::CommandError>,
    > {
        Box::pin(async move {
            let tree_digest = hex::decode(&evidence.before_digest)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok());
            self.catalog
                .insert_label(
                    tx,
                    &crate::storage::postgres::NewLabel {
                        id: uuid::Uuid::new_v4(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier: evidence.before_frontier.clone(),
                        tree_digest,
                        label: self.label.clone(),
                        reason: self.reason.clone(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await
                .map_err(crate::log::CommandError::Storage)
        })
    }
}

/// Restores the document to an earlier projection (§7.1: "restore |
/// `expected_frontier` equals the head frontier"). The precondition is what
/// keeps a restore from silently discarding a concurrent edit the caller
/// never saw: if the head has moved since the caller last looked, this
/// refuses rather than guessing which state was meant.
struct Restore {
    request_id: uuid::Uuid,
    document_id: uuid::Uuid,
    catalog: std::sync::Arc<crate::storage::postgres::PostgresCatalog>,
    expected_frontier: Vec<u8>,
    /// Each restored text as `(path, body, original id)`. The id comes from
    /// the projection being restored and is what keeps a restore from
    /// severing identity: see `evaluate`.
    texts: Vec<(String, String, String)>,
    assets: Vec<(String, String)>,
    main: String,
    author_account_id: Option<uuid::Uuid>,
    author_label: String,
}

impl crate::log::Command for Restore {
    type Output = ();

    fn name(&self) -> &'static str {
        "restore"
    }

    // §7.2: a retry with the same `request_id` finds the `document_labels`
    // row this command's transaction wrote and returns without merging
    // twice. Checked here, before the sequencer's lock is taken, because by
    // the time `transact` could look this up the sequencer has already
    // prepared a second copy of the restore and inserted the log row
    // carrying it.
    fn replay(
        &mut self,
    ) -> futures_util::future::BoxFuture<
        '_,
        std::result::Result<Option<Self::Output>, crate::log::CommandError>,
    > {
        Box::pin(async move {
            self.catalog
                .label_by_request(self.document_id, self.request_id)
                .await
                .map(|found| found.map(|_| ()))
                .map_err(crate::log::CommandError::Storage)
        })
    }

    fn evaluate(
        &mut self,
        head: &crate::log::Head<'_>,
    ) -> std::result::Result<Option<crate::log::PreparedSource>, crate::log::CommandError> {
        if head.frontier.encode() != self.expected_frontier {
            return Err(crate::log::CommandError::Conflict(
                "the document changed since that moment; refresh before restoring".into(),
            ));
        }
        let texts = &self.texts;
        let assets = &self.assets;
        let main = self.main.as_str();
        let client_seq = client_seq_of(self.request_id);
        head.prepare(client_seq, move |draft| {
            let wanted_texts: std::collections::HashSet<&str> =
                texts.iter().map(|(path, _, _)| path.as_str()).collect();
            // Also by id, because a file that was renamed since the state
            // being restored is wanted under its OLD path and is sitting
            // under its new one. Removing it here and putting it back below
            // would be the delete-and-recreate this whole block exists to
            // avoid; the rename in the loop below moves it instead.
            let wanted_ids: std::collections::HashSet<&str> =
                texts.iter().map(|(_, _, id)| id.as_str()).collect();
            let wanted_assets: std::collections::HashSet<&str> =
                assets.iter().map(|(path, _)| path.as_str()).collect();
            for (path, id) in crate::document::session::text_ids_of(draft) {
                if !wanted_texts.contains(path.as_str()) && !wanted_ids.contains(id.as_str()) {
                    crate::document::session::remove_path(draft, &path);
                }
            }
            for path in crate::document::session::assets_of(draft).into_keys() {
                if !wanted_assets.contains(path.as_str()) {
                    crate::document::session::remove_asset(draft, &path);
                }
            }
            // Identity, not just words. A restore rewrites the document to
            // an earlier state, and the naive way to do that -- delete
            // everything and put it back -- is wrong three times over:
            //
            //  * a concurrent editor sees their file disappear and reappear
            //    under their caret rather than words changing in it;
            //  * the version vector loses everyone who had touched the text;
            //  * and a comment names its file by the stable Loro id, never
            //    by the path, so a file that comes back under a new id
            //    silently orphans every comment on it -- on exactly the
            //    operation most likely to bring a deleted file back.
            //
            // So each text is put back as itself wherever it can be.
            // `put_text` already applies a minimal `LoroText::update` diff
            // to a file that is still there, which covers the first two; the
            // two branches below cover the third.
            let live_paths = crate::document::session::text_ids_of(draft);
            let mut path_of_id: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for (path, id) in &live_paths {
                path_of_id.insert(id.clone(), path.clone());
            }
            let mut main_id = String::new();
            for (path, body, original) in texts {
                let id = if let Some(moved_to) = path_of_id.get(original) {
                    // Renamed since the state being restored. Move it back
                    // rather than replacing it, so the carets and the
                    // authorship attached to that `LoroText` come with it.
                    if moved_to != path {
                        crate::document::session::rename_path(draft, moved_to, path);
                    }
                    crate::document::session::put_text(draft, path, body);
                    original.clone()
                } else if !original.is_empty()
                    && crate::document::session::put_text_with_id(draft, original, path, body)
                {
                    // Deleted since the state being restored. It comes back
                    // as itself: the comments anchored to it name this id.
                    // `put_text_with_id` refuses unless both the id and the
                    // path are free, so this cannot collide with anything
                    // the live document holds.
                    original.clone()
                } else {
                    crate::document::session::put_text(draft, path, body)
                };
                if path == main {
                    main_id = id;
                }
            }
            for (path, digest) in assets {
                crate::document::session::put_asset(draft, path, digest);
            }
            if !main_id.is_empty() {
                crate::document::session::set_main(draft, &main_id);
            }
            Ok(())
        })
        .map_err(crate::log::CommandError::Conflict)
    }

    fn transact<'a>(
        &'a mut self,
        tx: &'a mut sqlx::Transaction<'_, sqlx::Postgres>,
        evidence: &'a crate::log::Evidence,
    ) -> futures_util::future::BoxFuture<
        'a,
        std::result::Result<Self::Output, crate::log::CommandError>,
    > {
        Box::pin(async move {
            let frontier = evidence
                .after_frontier
                .clone()
                .unwrap_or_else(|| evidence.before_frontier.clone());
            let digest_hex = evidence
                .after_digest
                .as_deref()
                .unwrap_or(&evidence.before_digest);
            let tree_digest = hex::decode(digest_hex)
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok());
            self.catalog
                .insert_label(
                    tx,
                    &crate::storage::postgres::NewLabel {
                        id: uuid::Uuid::new_v4(),
                        document_id: self.document_id,
                        source_sequence: evidence.source_sequence,
                        vector: evidence.vector.clone(),
                        frontier,
                        tree_digest,
                        label: None,
                        reason: self.name().to_string(),
                        request_id: Some(self.request_id),
                        author_account_id: self.author_account_id,
                        author_label: self.author_label.clone(),
                    },
                )
                .await
                .map_err(crate::log::CommandError::Storage)?;
            Ok(())
        })
    }
}

/// A label row, on the wire.
fn label_wire(row: &crate::storage::postgres::LabelRecord) -> Value {
    json!({
        "sha": row.id,
        "sequence": row.sequence,
        "at": row.created_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
        "by": row.author_label,
        "label": row.label,
        "reason": row.reason,
        "tree_sha": row.tree_digest.as_ref().map(hex::encode),
        "frontier": crate::room::encode_update(&row.frontier),
        "archive_status": if row.archive_key.is_some() {
            "ready"
        } else if row.archive_error.is_some() {
            "failed"
        } else if row.archive_requested_at.is_some() {
            "pending"
        } else {
            "none"
        },
    })
}

impl Server {
    /// The document's manifest: every label, newest first.
    ///
    /// Editor-only, as the manifest has always been: a label names a moment
    /// in the document's own past, which is information about the people
    /// writing it.
    pub(super) async fn handle_history(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (entry, who) = match self.entry_viewer(slug, headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let (after, limit) = query
            .map(|raw| {
                let values: HashMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
                    .into_owned()
                    .collect();
                let after = values
                    .get("after")
                    .and_then(|value| value.parse::<i64>().ok());
                let limit = values
                    .get("limit")
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(64)
                    .clamp(1, 200);
                (after, limit)
            })
            .unwrap_or((None, 64));
        let rows = match self
            .store
            .catalog
            .label_page(room.document_id, after, limit)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let next_cursor = (rows.len() as i64 == limit)
            .then(|| rows.last().map(|row| row.sequence))
            .flatten();
        let labels: Vec<_> = rows.iter().map(label_wire).collect();
        let mut body = json!({
            "slug": entry.slug,
            "main": entry.main,
            "labels": labels,
        });
        if let Some(next) = next_cursor {
            body["next_cursor"] = json!(next);
        }
        let mut response = write_json(200, &body);
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// The document at one position in its own history, and -- on
    /// `?archive=1` -- the state of a plain-source export of it (§8.5).
    ///
    /// Reading the texts needs nothing that is not already in the log: the
    /// projection at any frontier this document ever had is computed the
    /// same way the live head's is (§4.4), so there is no "not ready yet"
    /// for the ordinary read. The archive is the one part that is produced
    /// on request rather than always available, because it is bytes written
    /// to the object store rather than a read of the log.
    pub(super) async fn handle_label_read(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let moment = sha.strip_prefix(MOMENT);
        if moment.is_none() && uuid::Uuid::parse_str(sha).is_err() {
            return plain(404, "not found");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (entry, who) = match self.entry_viewer(slug, headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        if let Some(encoded) = moment {
            // A comment's own frontier, not a named label: there is no
            // archive to poll or request for it (§8.5 is label-keyed), and
            // there is no row to look up -- `with_fork_at` reads the state
            // directly, the same way it does for a label's own frontier a
            // few lines down.
            let wants_archive = query.is_some_and(|raw| {
                url::form_urlencoded::parse(raw.as_bytes()).any(|(k, v)| k == "archive" && v != "0")
            });
            if wants_archive {
                return write_json(
                    400,
                    &json!({"error": "a live moment has no archive to request"}),
                );
            }
            let Some(frontier_bytes) = crate::room::decode_update(encoded) else {
                return write_json(400, &json!({"error": "the frontier is not base64"}));
            };
            let Ok(frontier) = loro::Frontiers::decode(&frontier_bytes) else {
                return write_json(400, &json!({"error": "the frontier is unreadable"}));
            };
            let projected = match room.log().projection_at(&frontier).await {
                Ok(projected) => projected,
                Err(error) => return sequencer_reply(&error),
            };
            let mut response = write_json(
                200,
                &projection_wire(
                    MomentMetadata {
                        sha,
                        at: "",
                        by: "",
                        label: None,
                        reason: "",
                    },
                    &entry.storage_id,
                    &entry.source_format,
                    &projected,
                ),
            );
            set(&mut response, "cache-control", "private, no-store");
            return response;
        }
        let label_id = uuid::Uuid::parse_str(sha).expect("checked above");
        let label = match self.store.catalog.label(room.document_id, label_id).await {
            Ok(Some(label)) => label,
            Ok(None) => return plain(404, "not found"),
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let wants_archive = query.is_some_and(|raw| {
            url::form_urlencoded::parse(raw.as_bytes()).any(|(k, v)| k == "archive" && v != "0")
        });
        if wants_archive {
            if label.archive_key.is_none() && label.archive_requested_at.is_none() {
                if let Err(error) = self
                    .store
                    .catalog
                    .request_label_archive(room.document_id, label_id)
                    .await
                {
                    return write_json(
                        503,
                        &json!({"error": error.to_string(), "retryable": true}),
                    );
                }
                self.background
                    .ask(crate::storage::worker::Task::Archive(label_id));
            }
            let mut response = write_json(
                if label.archive_key.is_some() {
                    200
                } else {
                    202
                },
                &label_wire(&label),
            );
            set(&mut response, "cache-control", "private, no-store");
            return response;
        }
        let Ok(frontier) = loro::Frontiers::decode(&label.frontier) else {
            return write_json(
                500,
                &json!({"error": "that label's frontier is unreadable"}),
            );
        };
        let projected = match room.log().projection_at(&frontier).await {
            Ok(projected) => projected,
            Err(error) => return sequencer_reply(&error),
        };
        let sha = label.id.to_string();
        let at = label
            .created_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let mut response = write_json(
            200,
            &projection_wire(
                MomentMetadata {
                    sha: &sha,
                    at: &at,
                    by: &label.author_label,
                    label: label.label.as_deref(),
                    reason: &label.reason,
                },
                &entry.storage_id,
                &entry.source_format,
                &projected,
            ),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// The bytes of a label's plain-source archive, once one exists (§8.5).
    /// `handle_label_read`'s `?archive=1` is what requests and polls it;
    /// this is only the download, and it is a 404 until `archive_key` is
    /// set, same as any object that has not been produced yet. `export --at`
    /// needs no CRDT decoder to read what comes back, because a plain-source
    /// tar never was one (§2.1).
    pub(super) async fn handle_label_archive(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let Ok(label_id) = uuid::Uuid::parse_str(sha) else {
            return plain(404, "not found");
        };
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (entry, who) = match self.entry_viewer(slug, headers, arrival, None).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let label = match self.store.catalog.label(room.document_id, label_id).await {
            Ok(Some(label)) => label,
            Ok(None) => return plain(404, "not found"),
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let Some(archive_key) = label.archive_key else {
            return plain(404, "not found");
        };
        crate::server::cost::blob_response(
            self.store.blobs.clone(),
            archive_key,
            &label_id.to_string(),
            headers,
            false,
        )
        .await
    }

    /// Names a label, or takes its name away.
    ///
    /// A `PATCH` because it changes the one field of a label that ever
    /// changes; unlike everything else in this file it needs no source and
    /// no fence beyond the writer epoch (see `PostgresCatalog::rename_label`).
    /// `sha == "current"` is the one exception: a virtual row the history
    /// panel uses to mean "name the document as it stands right now", which
    /// first has to become a real label before it can be named.
    pub(super) async fn handle_label(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let current = sha == "current";
        if !current && !is_label_id(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (_entry, who) = match self
            .entry_viewer(slug, request.headers(), arrival, None)
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        let Ok(body) = to_bytes(request.into_body(), 8 * 1024).await else {
            return write_json(413, &json!({"error": "that label is too long"}));
        };
        let asked: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let label = asked
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let label = crate::util::clean(label, MAX_LABEL);
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let authority = who.document_authority();
        let author_account_id = uuid::Uuid::parse_str(&who.id.id).ok();
        let label_id = if current {
            let mut command = Label {
                request_id: uuid::Uuid::new_v4(),
                document_id: room.document_id,
                catalog: self.store.catalog.clone(),
                reason: "label".to_string(),
                label: None,
                author_account_id,
                author_label: who.attribution().display().to_string(),
            };
            match room.command(&authority, &mut command).await {
                Ok(row) => row.id,
                Err(error) => return command_reply(error),
            }
        } else {
            uuid::Uuid::parse_str(sha).expect("checked by is_label_id")
        };
        let named = if label.is_empty() {
            None
        } else {
            Some(label.as_str())
        };
        match self
            .store
            .catalog
            .rename_label(room.document_id, label_id, named)
            .await
        {
            Ok(true) => write_json(200, &json!({"sha": label_id, "label": label})),
            Ok(false) => plain(404, "not found"),
            Err(error) => write_json(503, &json!({"error": error.to_string(), "retryable": true})),
        }
    }

    /// Restores one label into the live document. The caller must be an
    /// editor, which includes an editor share link; readers can inspect the
    /// same label through GET but cannot change the document with it.
    pub(super) async fn handle_restore(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let headers = request.headers().clone();
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (_entry, who) = match self
            .entry_viewer(slug, request.headers(), arrival, None)
            .await
        {
            Ok(result) => result,
            Err(response) => return response,
        };
        if !who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        let body = match to_bytes(request.into_body(), 16 * 1024).await {
            Ok(body) => body,
            Err(_) => return write_json(413, &json!({"error": "restore request too large"})),
        };
        let asked: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let requested = asked
            .get("sha")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let Ok(label_id) = uuid::Uuid::parse_str(requested) else {
            return write_json(400, &json!({"error": "a label ID is required"}));
        };
        // What the caller believes the head is right now: the restore's own
        // precondition (§7.1). Required, because a restore with nothing to
        // check itself against would silently discard whatever a concurrent
        // editor had just typed.
        let expected = asked
            .get("expected_frontier")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(expected_frontier) = crate::room::decode_update(expected) else {
            return write_json(
                400,
                &json!({"error": "expected_frontier is required and must be base64"}),
            );
        };

        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // The owner may have transferred the document, or a link may have
        // been revoked, while the request body was being read.
        let (_current_entry, current_who) =
            match self.entry_viewer(slug, &headers, arrival, None).await {
                Ok(result) => result,
                Err(response) => return response,
            };
        if !current_who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        let label = match self.store.catalog.label(room.document_id, label_id).await {
            Ok(Some(label)) => label,
            Ok(None) => return plain(404, "not found"),
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let Ok(frontier) = loro::Frontiers::decode(&label.frontier) else {
            return write_json(
                500,
                &json!({"error": "that label's frontier is unreadable"}),
            );
        };
        let target = match room.log().projection_at(&frontier).await {
            Ok(target) => target,
            Err(error) => return sequencer_reply(&error),
        };
        let mut texts = Vec::new();
        let mut assets = Vec::new();
        for (path, file) in &target.projection.files {
            if file.kind == "text" {
                if let Some(body) = target.texts.get(path) {
                    texts.push((path.clone(), body.clone(), file.id.clone()));
                }
            } else {
                assets.push((path.clone(), file.digest.clone()));
            }
        }
        let authority = current_who.document_authority();
        let author_account_id = uuid::Uuid::parse_str(&current_who.id.id).ok();
        let mut command = Restore {
            request_id: uuid::Uuid::new_v4(),
            document_id: room.document_id,
            catalog: self.store.catalog.clone(),
            expected_frontier,
            texts,
            assets,
            main: target.projection.main.clone(),
            author_account_id,
            author_label: current_who.attribution().display().to_string(),
        };
        if let Err(error) = room.command(&authority, &mut command).await {
            return command_reply(error);
        }
        // The command wrote its own label for the moment the restore
        // produced; what a caller wants back here is what it asked to
        // restore, which is still `label` as read above.
        write_json(
            200,
            &json!({
                "sha": label_id,
                "label": label_wire(&label),
            }),
        )
    }

    /* -------------------------------------------------------------- assets */
}
