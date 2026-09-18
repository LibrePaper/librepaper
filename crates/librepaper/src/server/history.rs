//! The timeline routes: the manifest, one checkpoint, its label, and restoring
//! the document to it.

use super::*;

/// The longest a checkpoint's label may be, in characters. Long enough for
/// "submitted after review, second round" and short enough that the history panel
/// is a list of names rather than of paragraphs.
pub(super) const MAX_LABEL: usize = 120;

fn is_checkpoint_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

/// Content authorship is separate from the checkpoint event actor. Current
/// session writes do not carry a durable contributor interval proof, so the
/// browser receives an explicit unknown value rather than mistaking `by` for
/// the editor. A future evidence-bearing writer can replace this field at the
/// capture boundary without changing the history API shape.
fn checkpoint_wire(point: &crate::document::history::Checkpoint) -> serde_json::Value {
    let mut value = serde_json::to_value(point).unwrap_or_else(|_| json!({}));
    if let Some(object) = value.as_object_mut() {
        object
            .entry("authorship")
            .or_insert_with(|| json!({"kind": "unknown"}));
    }
    value
}

impl Server {
    /// When the document was worked on: one row per minute in which an update
    /// was admitted, oldest first.
    ///
    /// Not the same question as the manifest above. A checkpoint is a moment
    /// somebody's work was named or the room went quiet, and there are a few
    /// dozen of them; this is the shape of the typing itself, and it is what a
    /// calendar or a histogram is drawn from. Editor-only for the same reason
    /// the manifest is: when a document was written is information about the
    /// people writing it.
    pub(super) async fn handle_activity(
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
        // `since` bounds the scan rather than the answer's meaning: a caller
        // drawing one year asks for one year, and the buckets before it are
        // not read at all.
        let since = query.and_then(|raw| {
            let values: HashMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
                .into_owned()
                .collect();
            values.get("since").and_then(|value| {
                time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .ok()
            })
        });
        let catalog = self.store.catalog.clone();
        let Ok(Some(document)) = catalog.document_by_slug(slug).await else {
            return plain(404, "not found");
        };
        let buckets = match catalog.document_activity(document.id, since).await {
            Ok(buckets) => buckets,
            Err(error) => return write_json(503, &json!({"error": error.to_string()})),
        };
        let rows: Vec<_> = buckets
            .iter()
            .map(|bucket| {
                json!({
                    "at": bucket
                        .bucket
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                    // Empty for every row today: nothing records an author on
                    // an update yet, and the browser must not draw one.
                    "peer": bucket.peer,
                    "changes": bucket.changes,
                    // The document at this moment, for a reader that wants to
                    // go there: what `fork_at` takes. Absent rather than empty
                    // when a bucket predates the anchor.
                    "frontier": (!bucket.frontier.is_empty())
                        .then(|| crate::room::encode_update(&bucket.frontier)),
                    // The document, not the edit: difference consecutive
                    // buckets to see the work.
                    "state_bytes": bucket.state_bytes,
                })
            })
            .collect();
        let mut response = write_json(
            200,
            &json!({
                "slug": entry.slug,
                // Said rather than assumed: a client that buckets by day or
                // hour is summing these, and needs to know what one row is.
                "resolution": "minute",
                "activity": rows,
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// The document at one position in its own history.
    ///
    /// `frontier` is what the activity index hands out per bucket, so this is
    /// how a moment on that timeline becomes something to look at. It answers
    /// for moments nobody checkpointed, which is the whole point of keeping
    /// the operation history rather than thinning it: the reader is not
    /// limited to the versions somebody happened to save.
    ///
    /// Read-only and side-effect free -- it forks the room's document and
    /// never writes. Restoring the document to one of these remains the
    /// restore route's business, which asks for confirmation.
    pub(super) async fn handle_at(
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
        let asked = query.and_then(|raw| {
            let values: HashMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
                .into_owned()
                .collect();
            values.get("frontier").cloned()
        });
        let Some(asked) = asked else {
            return write_json(400, &json!({"error": "a frontier is required"}));
        };
        let Some(frontier) = crate::room::decode_update(&asked) else {
            return write_json(400, &json!({"error": "the frontier is not base64"}));
        };
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // A frontier this document never had is the caller's mistake, not a
        // failure of the room: it is refused as a bad request rather than
        // reported as one.
        let (main, texts, assets) = match room.project_at_frontier(&frontier).await {
            Ok(found) => found,
            Err(error) => return write_json(400, &json!({"error": error})),
        };
        let mut response = write_json(
            200,
            &json!({
                "slug": entry.slug,
                "storage_id": entry.storage_id,
                "frontier": asked,
                "source_format": entry.source_format,
                "main": main,
                "texts": texts,
                // Path to digest, as the tree holds them: the bytes are the
                // asset routes' business, and a historical state names the
                // same objects the live one does.
                "assets": assets,
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// The document's manifest: every checkpoint, oldest first, with the paths
    /// each of them changed.
    ///
    /// `changed` is what makes a timeline of a directory readable. Without it,
    /// saying which files moved between two moments means fetching both trees
    /// and comparing them, for every row; the checkpoint records it once, when
    /// it is taken and both trees are already in hand.
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
        // History contains source paths, revisions, and source-bearing
        // checkpoint metadata, so it is editor-only.
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let page = query
            .map(|raw| {
                let values: HashMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
                    .into_owned()
                    .collect();
                let after = values
                    .get("after")
                    .and_then(|value| value.parse::<i64>().ok());
                let limit = values
                    .get("limit")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(64)
                    .clamp(1, 200);
                (after, limit)
            })
            .unwrap_or((None, 64));
        let (checkpoints, next_cursor) = {
            let (after, limit) = page;
            match room.checkpoint_page(after, limit).await {
                Ok(result) => result,
                Err(error) => return write_json(503, &json!({"error": error})),
            }
        };
        let checkpoints: Vec<_> = checkpoints.iter().map(checkpoint_wire).collect();
        let durability = room.history_durability().await;
        let mut body = json!({
            "slug": entry.slug,
            "main": entry.main,
            "checkpoints": checkpoints,
            // Live session persistence and historical checkpoint admission
            // are separate.  Keep the states explicit so the panel cannot
            // turn a delayed checkpoint into a false unsaved-edit claim (or
            // the reverse).
            "durability": {
                "live_save": if !durability.known {
                    "unknown"
                } else if durability.live_save_pending {
                    "pending"
                } else {
                    "saved"
                },
                "history_checkpoint": if !durability.known {
                    "unknown"
                } else if durability.checkpoint_pending {
                    "pending"
                } else {
                    "current"
                },
            },
        });
        if let Some(next) = next_cursor {
            body["next_cursor"] = json!(next);
        }
        let mut response = write_json(200, &body);
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// What the document said at one checkpoint: every file it had, and the
    /// text of each. This is what the panel shows in the document pane when a
    /// reader picks a moment out of the timeline, and what the comment cards
    /// look a passage up in.
    ///
    /// Texts only. A checkpoint records its figures by digest, and those are
    /// already served, immutably, by the figures route -- so the tree names
    /// them and the browser fetches the ones it needs, rather than this
    /// answer carrying every image the document has ever had.
    pub(super) async fn handle_checkpoint(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        sha: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        // The checkpoint event ID addresses a catalog row; its tree digest
        // and physical object locator are separate identities.
        if !is_checkpoint_id(sha) {
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
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // The manifest is the list of checkpoints this document has, and an
        // object under the history prefix that the manifest does not name is
        // not one of them -- a shed checkpoint whose object is still there,
        // most likely. Asking the manifest rather than the store is what keeps
        // the two from disagreeing.
        let point = match room.checkpoint_by_sha(sha).await {
            Ok(Some(point)) => point,
            Ok(None) => return plain(404, "not found"),
            Err(error) => return write_json(503, &json!({"error": error})),
        };
        let (tree, bodies) = match room.checkpoint_texts(&point).await {
            Ok(found) => found,
            Err(err) => return write_json(500, &json!({"error": err})),
        };
        let texts: HashMap<&str, &str> = tree
            .files
            .iter()
            .filter(|(_, file)| file.kind == "text")
            .filter_map(|(path, file)| {
                bodies
                    .get(&file.sha)
                    .map(|body| (path.as_str(), body.as_str()))
            })
            .collect();
        let mut response = write_json(
            200,
            &json!({
                "sha": point.sha,
                "tree_sha": point.content_sha(),
                "storage_id": entry.storage_id,
                "at": point.at,
                "by": point.by,
                "authorship": {"kind": "unknown"},
                "original_parent": point.original_parent,
                "ancestry_gap": point.ancestry_gap,
                "why": point.why,
                "label": point.label,
                "source_format": point.source_format,
                "main": tree.main,
                "files": tree.files,
                "settings": tree.settings,
                "texts": texts,
            }),
        );
        set(&mut response, "cache-control", "private, no-store");
        response
    }

    /// Names a checkpoint, or takes its name away.
    ///
    /// A `PATCH` because it changes one field of an entry that already exists,
    /// and the only field of one that ever changes. It takes an editor: a
    /// label is what the document says about its own past, and saying that is
    /// the same right as changing the text.
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
        // `current` is a virtual row used by the history panel. Naming it
        // first captures pending edits, then labels the resulting checkpoint.
        let current = sha == "current";
        if !current && !is_checkpoint_id(sha) {
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
        // As everywhere else: a document somebody may not change is not a
        // document they need to learn the shape of.
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
        // Trimmed rather than refused, the way every other text a caller sends
        // is: control characters out, one line, and short enough that the
        // panel is a list of names rather than of paragraphs.
        let label = crate::util::clean(label, MAX_LABEL);
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let sha = if current {
            match room.checkpoint("label", who.attribution()).await {
                Ok(Some(sha)) => sha,
                Ok(None) => return plain(404, "not found"),
                Err(error) => {
                    return refused_with(
                        &format!("could not name the current draft of {slug}"),
                        &error,
                        &[],
                    )
                }
            }
        } else {
            sha.to_string()
        };
        match room.label_version(&sha, &label).await {
            Ok(true) => write_json(200, &json!({"sha": sha, "label": label})),
            Ok(false) => plain(404, "not found"),
            Err(error) => refused_with(
                &format!("could not label a checkpoint of {slug}"),
                &error,
                // The checkpoint the caller asked about, kept so a client can
                // match a refusal to the row it labelled.
                &[("sha", json!(sha))],
            ),
        }
    }

    /// Restores one checkpoint into the live room. The caller must be an
    /// editor, which includes an editor share link; readers can inspect the
    /// same checkpoint through GET but cannot change the document with it.
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
        if !is_checkpoint_id(requested) {
            return write_json(400, &json!({"error": "a checkpoint ID is required"}));
        }

        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // The owner may have transferred the document, or a link may have
        // been revoked, while the request body was being read. Recheck the
        // role immediately before the room mutation as well as before it.
        let (_current_entry, current_who) =
            match self.entry_viewer(slug, &headers, arrival, None).await {
                Ok(result) => result,
                Err(response) => return response,
            };
        if !current_who.at_least(Role::Editor) {
            return plain(404, "not found");
        }
        let point = match room.checkpoints_prefix(requested).await {
            Ok(point) => point,
            Err(error) => return write_json(503, &json!({"error": error})),
        };
        let point = match point.as_slice() {
            [] => return plain(404, "not found"),
            [point] => point.clone(),
            _ => return write_json(409, &json!({"error": "checkpoint prefix is ambiguous"})),
        };
        let by = current_who.attribution();
        let (update, sha) = match room.restore_and_checkpoint(&point, &by).await {
            Ok(result) => result,
            Err(error) => {
                return refused(&format!("could not restore {slug}"), &error);
            }
        };
        room.broadcast_editors_except(
            None,
            &json!({
                "type": "doc-update",
                "update": encode_update(&update),
            }),
        )
        .await;
        let checkpoint = match room.checkpoint_by_sha(&sha).await {
            Ok(point) => point,
            Err(error) => return write_json(503, &json!({"error": error})),
        };
        write_json(
            200,
            &json!({
                "sha": sha,
                "checkpoint": checkpoint,
            }),
        )
    }

    /* -------------------------------------------------------------- assets */
}
