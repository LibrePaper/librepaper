//! The timeline routes: the manifest, one checkpoint, its label, and restoring
//! the document to it.

use super::*;

/// The longest a checkpoint's label may be, in characters. Long enough for
/// "sent to the journal, second round" and short enough that the history panel
/// is a list of names rather than of paragraphs.
pub(super) const MAX_LABEL: usize = 120;

impl Server {
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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let page = query.and_then(|raw| {
            let values: HashMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
                .into_owned()
                .collect();
            if !values.contains_key("after") && !values.contains_key("limit") {
                return None;
            }
            let after = values
                .get("after")
                .and_then(|value| value.parse::<i64>().ok());
            let limit = values
                .get("limit")
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(64)
                .clamp(1, 200);
            Some((after, limit))
        });
        let (checkpoints, next_cursor) = if let Some((after, limit)) = page {
            match room.checkpoint_page(after, limit).await {
                Ok(result) => result,
                Err(error) => return write_json(503, &json!({"error": error})),
            }
        } else {
            (room.manifest().await.checkpoints, None)
        };
        let mut body = json!({
            "slug": entry.slug,
            "main": entry.main,
            "checkpoints": checkpoints,
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
        // A digest and nothing else: this becomes a storage key.
        if !is_sha(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, None).await;
        if !self.may_read(&entry, &who) {
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
                "at": point.at,
                "by": point.by,
                "why": point.why,
                "label": point.label,
                "source_format": point.source_format,
                "main": tree.main,
                "files": tree.files,
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
        if !is_sha(sha) {
            return plain(404, "not found");
        }
        if cross_site_refused(request.headers(), arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
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
        match room
            .label_as_authority(
                sha,
                &label,
                Some(crate::storage::catalog::MutationAuthority {
                    account_id: who.id.id.as_str(),
                    owner_key: who.key.as_str(),
                    generation: who.id.session_generation.as_str(),
                    link_hash: who.link.as_str(),
                    policy_editor: self.publishers.allows(&who.id.handle),
                    automation: who.automation,
                    unowned_publisher: false,
                }),
            )
            .await
        {
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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, request.headers(), arrival, None).await;
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
        if requested.is_empty()
            || requested.len() > 64
            || !requested
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return write_json(400, &json!({"error": "a checkpoint SHA is required"}));
        }

        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        // The owner may have transferred the document, or a link may have
        // been revoked, while the request body was being read. Recheck the
        // role immediately before the room mutation as well as before it.
        let current_entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        let current_who = self.viewer(&current_entry, &headers, arrival, None).await;
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
        let target_main = match room.checkpoint_texts(&point).await {
            Ok((tree, _)) => tree.main,
            Err(error) => return write_json(503, &json!({"error": error})),
        };
        let by = current_who.attribution();
        let (update, sha) = match room.restore_and_checkpoint(&point, &by).await {
            Ok(result) => result,
            Err(error) => {
                return refused(&format!("could not restore {slug}"), &error);
            }
        };
        room.broadcast(&json!({
            "type": "y-update",
            "update": encode_update(&update),
        }))
        .await;
        let checkpoint = match room.checkpoint_by_sha(&sha).await {
            Ok(point) => point,
            Err(error) => return write_json(503, &json!({"error": error})),
        };
        let authority = crate::storage::catalog::MutationAuthority {
            account_id: current_who.id.id.as_str(),
            owner_key: current_who.key.as_str(),
            generation: current_who.id.session_generation.as_str(),
            link_hash: current_who.link.as_str(),
            policy_editor: self.publishers.allows(&current_who.id.handle),
            automation: current_who.automation,
            unowned_publisher: false,
        };
        let quarto_selection = match room
            .reconcile_quarto_selections_after_restore(point.content_sha(), &target_main, authority)
            .await
        {
            Ok((reselected, cleared)) if reselected > 0 => json!({
                "status": "reselected",
                "reason": "associated retained Quarto bundle selected for restored source",
                "reselected": reselected,
                "cleared": cleared,
            }),
            Ok((_, cleared)) => json!({
                "status": "cleared",
                "reason": "source restored; no associated retained Quarto bundle was selected",
                "contexts": cleared,
            }),
            Err(error) => json!({
                "status": "unknown",
                "reason": "source restored; Quarto selection reconciliation failed; retry the restore or select saved results",
                "error": error,
            }),
        };
        write_json(
            200,
            &json!({
                "sha": sha,
                "checkpoint": checkpoint,
                "quarto_selection": quarto_selection,
            }),
        )
    }

    /* -------------------------------------------------------------- assets */
}
