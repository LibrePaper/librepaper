//! A document's own routes: publishing and replacing it, reading its source,
//! state and snapshot, its comments over HTTP, and deleting it.

use super::*;

pub(super) struct Upload {
    pub(super) title: String,
    pub(super) slug: String,
    /// The document itself, in the format below. There is no rendered form
    /// here: nothing derived is stored.
    pub(super) source: String,
    pub(super) source_format: String,
    /// What the main file is called. A publish of one file is a directory of
    /// one file, and this is its name in it.
    pub(super) main: String,
    /// The rest of the directory, when a whole one was published: every file
    /// but the main one, by path. Texts are UTF-8 and go into the shared
    /// document; the rest are figures and go to the store under their digest.
    pub(super) files: Vec<(String, Vec<u8>)>,
}

impl Server {
    pub async fn delete_document(&self, slug: &str) -> Result<usize, String> {
        let storage_id = self.store.begin_delete(slug).await?;
        // Tear down the document's live chat channels only after deletion is
        // admitted; a refused delete must leave those conversations running.
        self.chat.purge(slug).await;
        self.rooms
            .purge_with_identity(slug, storage_id.as_deref())
            .await;
        self.store.remove(slug).await
    }

    /// Removes every document older than `retention` seconds, measured from
    /// `from`. Returns how many went.
    pub async fn delete_expired(&self, now: i64, retention: i64, from: &str) -> usize {
        let mut removed = 0;
        let cutoff = now - retention;
        let entries = match self.store.list_result().await {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("could not enumerate documents for retention: {error}");
                return 0;
            }
        };
        for entry in entries {
            if let Some(stamp) = entry.expiry_time(from) {
                if stamp <= cutoff {
                    // The janitor runs unattended: a document whose index entry
                    // could not be rewritten is left for the next pass.
                    if let Err(err) = self.delete_document(&entry.slug).await {
                        eprintln!("could not expire {}: {err}", entry.slug);
                        continue;
                    }
                    removed += 1;
                }
            }
        }
        removed
    }

    pub(super) async fn handle_comments(
        &self,
        request: Request<Body>,
        peer: SocketAddr,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if who.auth_failed {
            return write_json(
                401,
                &json!({"error": "authentication expired or was revoked"}),
            );
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let author = self.comment_author(&headers, arrival, &who.id);
        let may_edit = who.at_least(Role::Editor);

        match *request.method() {
            Method::GET => write_json(
                200,
                &json!({"comments": room.snapshot_for(&author, may_edit).await}),
            ),
            Method::POST => {
                if cross_site_refused(&headers, arrival) {
                    return write_json(403, &cross_site_refusal());
                }
                let Ok(body) = to_bytes(request.into_body(), 1 << 20).await else {
                    return write_json(400, &json!({"error": "bad request"}));
                };
                let Ok(incoming) = serde_json::from_slice::<RoomMessage>(&body) else {
                    return write_json(400, &json!({"error": "bad request"}));
                };
                if incoming.motivation == "editing" && !who.at_least(Role::Editor) {
                    return write_json(
                        403,
                        &json!({"error": "editor access is required for suggestions"}),
                    );
                }
                // Body reads are an await boundary. Resolve identity and
                // document rights again afterwards so revocation/transfer
                // cannot race a large request into a mutation.
                let current_entry = match self.checked_entry(slug).await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => return write_json(404, &json!({"error": "not found"})),
                    Err(response) => return response,
                };
                let current_who = self
                    .viewer(&current_entry, &headers, arrival, query.as_deref())
                    .await;
                if current_who.auth_failed {
                    return write_json(
                        401,
                        &json!({"error": "authentication expired or was revoked"}),
                    );
                }
                if !self.may_read(&current_entry, &current_who)
                    || (who.at_least(Role::Commenter) && !current_who.at_least(Role::Commenter))
                {
                    return write_json(403, &json!({"error": "comment access changed"}));
                }
                // A read link can open the rendered publication but never
                // mutate its annotation channel. Refuse at the HTTP boundary
                // so a role failure is not misreported as a room validation
                // error after work has already begun.
                if !current_who.at_least(Role::Commenter) {
                    return write_json(403, &json!({"error": "commenter access is required"}));
                }
                // Readers and commenters only annotate the current rendered
                // publication. An empty ID is never a fallback to source or
                // an unchecked annotation write. Editors still use empty IDs
                // for source-side comments and suggestions.
                if incoming.kind == "comment"
                    && !current_who.at_least(Role::Editor)
                    && incoming.publication_id.is_empty()
                {
                    return write_json(
                        409,
                        &json!({"error": "publication is required; refresh before annotating"}),
                    );
                }
                let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
                // A rendered annotation is checked and committed under the
                // same per-storage gate as publication activation. Do not
                // take this for source suggestions: they remain editor work
                // against the editable revision and have no publication.
                let _rendered_publication_guard =
                    if incoming.kind == "comment" && !incoming.publication_id.is_empty() {
                        Some(
                            crate::server::publication::publication_lock(&current_entry.storage_id)
                                .lock_owned()
                                .await,
                        )
                    } else {
                        None
                    };
                if let Some(publication_id) = (incoming.kind == "comment"
                    && !incoming.publication_id.is_empty())
                .then_some(incoming.publication_id.as_str())
                {
                    let current =
                        crate::server::publication::PublicationStore::for_store(self.store.clone())
                            .current(&current_entry.storage_id)
                            .await
                            .ok()
                            .flatten()
                            .map(|publication| publication.publication_id);
                    if current.as_deref() != Some(publication_id) {
                        return write_json(
                            409,
                            &json!({"error": "publication changed; refresh before annotating"}),
                        );
                    }
                }
                // Keep source/session publication work ordered after the
                // rendered-publication gate; activation never takes this
                // room-local lock, so this order cannot invert.
                let _publication_guard = room.publication_write.lock().await;
                if incoming.kind == "accept" || incoming.kind == "reject" {
                    return write_json(
                        410,
                        &json!({"error": "suggestion decisions are not supported"}),
                    );
                }
                let (result, ok) = self
                    .apply_from(&room, incoming, &address, &current_who, &author)
                    .await;
                if ok {
                    let shared = room.comment_event_for(&result, "", false).await;
                    room.broadcast(&shared).await;
                    let targeted = room.comment_event_for(&result, &author, may_edit).await;
                    return write_json(200, &targeted);
                }
                let status = result
                    .get("status")
                    .and_then(Value::as_u64)
                    .filter(|status| matches!(status, 400 | 403 | 404 | 409 | 410 | 503))
                    .unwrap_or(400) as u16;
                write_json(status, &result)
            }
            _ => plain(405, "method not allowed"),
        }
    }

    pub(super) async fn handle_upload(&self, request: Request<Body>, arrival: &Arrival) -> Reply {
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        // Checked before the body is read, so an unauthorised upload costs
        // nothing.
        let who = match self.publisher(request.headers(), arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let parsed = match self.read_upload(request).await {
            Ok(parsed) => parsed,
            Err(response) => return response,
        };

        let mut base = slugify(&parsed.slug, &self.config);
        if base.is_empty() {
            base = slugify(&parsed.title, &self.config);
        }
        if base.is_empty() {
            return write_json(400, &json!({"error": "could not derive a slug"}));
        }
        // An exact slug that already exists is a replacement of that document,
        // and keeps its URL and its comments. Anything else is a new document,
        // and gets a random suffix so the link cannot be guessed from the
        // title. Someone else's document is not yours to replace, and guessing
        // its slug should not even tell you it is there: a title that collides
        // with another publisher's document simply becomes a new document of
        // your own.
        let existing = match self.store.get_result(&base).await {
            Ok(existing) => existing,
            Err(error) => {
                return write_json(
                    503,
                    &json!({
                        "error": error.to_string(),
                        "retryable": true,
                    }),
                )
            }
        };
        let mine = existing
            .as_ref()
            .is_some_and(|e| e.owned_by(&who.key, &who.id));
        let key = if mine {
            base.clone()
        } else {
            format!("{base}-{}", random_suffix(&self.config))
        };
        let actor = crate::document::store::MutationActor {
            account_id: who.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: true,
            unowned_publisher: false,
        };
        // Publishing over a document is an edit into its live Room.  The Room
        // holds the restore/publication/version gates while merging the
        // directory, so concurrent CRDT edits are preserved and the resulting
        // version sees the same source generation it admitted.
        if mine {
            let room = match self.rooms.try_get(&key).await {
                Ok(room) => room,
                Err(error) => {
                    return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
                }
            };
            let entry = match self
                .edit_into_session(&room, &parsed, &who, &existing.clone().unwrap())
                .await
            {
                Ok(entry) => entry,
                Err(response) => return response,
            };
            // Share-link credentials are stored only as digests, so a
            // revision cannot reproduce an existing URL. It also must not
            // silently rotate the link merely to manufacture one.
            return write_json(
                201,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "url": format!("/docs/{}", entry.slug),
                    "share_url": Value::Null,
                }),
            );
        }

        // What a directory publish named, or -- for one file, which is a
        // directory of one file -- the name its format implies, which is
        // what the migration gives a document published before there were
        // directories.
        let main = if parsed.main.is_empty() {
            crate::room::main_path_for("", &parsed.source_format)
        } else {
            parsed.main.clone()
        };
        // Every file is checked against the size and encoding rules before
        // anything is written: a document is a directory, and it is either
        // published whole or refused whole, never left half-written because
        // the eleventh file was the one that broke a rule the first ten
        // happened to keep.
        if let Err(response) =
            self.preflight_directory(&parsed, &main, &crate::document::history::Tree::default())
        {
            return response;
        }
        let entry = match self
            .store
            .put_directory_as_actor(
                Publication {
                    slug: key.clone(),
                    title: parsed.title.clone(),
                    source: parsed.source.clone(),
                    source_format: parsed.source_format.clone(),
                    main: main.clone(),
                },
                parsed.files.clone(),
                actor,
            )
            .await
        {
            Ok(entry) => entry,
            Err(PutError::Quota { status, message }) => {
                return write_json(status, &json!({"error": message}))
            }
            Err(PutError::Authorization { status, message }) => {
                return write_json(status, &json!({"error": message}))
            }
            Err(PutError::Storage(_)) => {
                return write_json(500, &json!({"error": "could not store the document"}))
            }
        };
        let share_url = match self.mint_read_link(&key, &who).await {
            Ok(url) => Value::String(url),
            Err(error) => {
                eprintln!("warning: could not mint the read link of {key}: {error:?}");
                Value::Null
            }
        };
        write_json(
            201,
            &json!({
                "slug": entry.slug,
                "title": entry.title,
                "sha": entry.sha,
                "created_at": entry.created_at,
                "updated_at": entry.updated_at,
                "url": format!("/docs/{}", entry.slug),
                "share_url": share_url,
            }),
        )
    }

    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    /// A publish onto a document that already exists: the whole upload is
    /// reconciled into the live session as one operation, everyone with the
    /// document open sees it arrive, and a checkpoint marks the moment.
    ///
    /// An upload is the whole directory, not just its main file: a chapter it
    /// changes is diffed in by path, a file it adds appears, a file it does
    /// not mention any more is gone, and the main path follows what the
    /// upload named -- the same reconciliation `Room::restore` does when a
    /// checkpoint is brought back, because a republish and a restore are the
    /// same operation with a different source for the tree. A file both this
    /// upload and a concurrent editor leave untouched is carried over
    /// unchanged; one this upload changes is diffed by prefix and suffix
    /// exactly as a single-file publish always has been, so a concurrent
    /// editor of it keeps their words and sees the rest change under them.
    pub(super) async fn edit_into_session(
        &self,
        room: &Room,
        parsed: &Upload,
        who: &Caller,
        existing: &IndexEntry,
    ) -> Result<IndexEntry, Reply> {
        // Restore and suggestion acceptance use this gate already. Take it
        // first so a publication cannot mutate the live CRDT between their
        // merge-base read and their checkpoint; ordinary socket edits remain
        // free to proceed while either operation is in storage I/O.
        let _restore_writer = room.restore_write.lock().await;
        let _publication_writer = room.publication_write.lock().await;
        let _publication_checkpoint = room.publication_checkpoint.write().await;
        let (current, rollback_bodies, rollback_format) = {
            let state = room.state.lock().await;
            let (tree, bodies) =
                crate::room::tree_of(&state.session.doc, &state.session.asset_sizes);
            (tree, bodies, state.session.format.clone())
        };
        let main_path = if !parsed.main.is_empty() {
            parsed.main.clone()
        } else if !parsed.source_format.is_empty()
            && crate::room::format_from_path(&current.main) != parsed.source_format
        {
            // A one-file replacement that changes format also changes the
            // implied main file.  Keeping `main.md` while installing an HTML
            // source makes checkpoint's path-derived format immediately turn
            // the replacement back into markdown.
            crate::room::main_path_for("", &parsed.source_format)
        } else {
            current.main.clone()
        };
        // Validated whole, against what the document already holds, before
        // any of it touches storage or the session: a bad file in a republish
        // must leave the live document exactly as it was, not half-applied.
        self.preflight_directory(parsed, &main_path, &current)?;

        // A title given on the command line renames the document; an empty one
        // leaves it as it is.
        let title = if parsed.title.is_empty() {
            existing.title.clone()
        } else {
            parsed.title.clone()
        };
        self.store
            .check_project_title(&existing.slug, &title)
            .await
            .map_err(|error| write_json(409, &json!({"error": error})))?;

        // Replacements are source uploads even though the live Room owns the
        // merge. Charge the durable document owner, rather than the editor's
        // account or link, so collaborators share one bounded bucket.

        let mut wanted: std::collections::HashSet<String> =
            parsed.files.iter().map(|(path, _)| path.clone()).collect();
        wanted.insert(main_path.clone());

        // Every new asset is stored under its digest before it is named in
        // the tree, outside any lock this call holds: a failure here is a
        // storage fault the preflight above could not have caught, and it
        // leaves an unreferenced blob rather than a half-written document,
        // because nothing has named it yet.
        let mut new_assets: HashMap<String, crate::document::history::TreeEntry> = HashMap::new();
        for (path, raw) in &parsed.files {
            if let Ok(crate::document::paths::Kind::Asset) =
                crate::document::paths::check(&self.config.paths(), path)
            {
                let (sha, size) = match room
                    .put_asset_authorized(
                        raw.clone(),
                        (self.config.max_asset, self.config.max_assets),
                        &crate::document::store::MutationActor {
                            account_id: who.id.clone(),
                            owner_key: who.key.clone(),
                            session_generation: who.session_generation.clone(),
                            link_hash: String::new(),
                            policy_editor: true,
                            unowned_publisher: false,
                        },
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(why) => {
                        eprintln!(
                            "warning: could not store {path} of {}: {why}",
                            existing.slug
                        );
                        return Err(write_json(
                            500,
                            &json!({"error": "could not store the document"}),
                        ));
                    }
                };
                new_assets.insert(
                    path.clone(),
                    crate::document::history::TreeEntry {
                        kind: "asset".to_string(),
                        id: String::new(),
                        sha,
                        size,
                    },
                );
            }
        }

        // Applied as one operation under one lock: the tree and its bodies
        // are read fresh here, right before `restore` is given them, rather
        // than from the snapshot above -- so a concurrent edit to a file this
        // upload does not touch, landed in the time it took to validate and
        // store the assets above, is carried forward as it now stands rather
        // than overwritten with a stale copy of it.
        let update = {
            let mut state = room.state.lock().await;
            let (mut tree, mut bodies) =
                crate::room::tree_of(&state.session.doc, &state.session.asset_sizes);
            let main_id = tree
                .files
                .get(&main_path)
                .map(|entry| entry.id.clone())
                .unwrap_or_default();
            let main_sha = crate::document::store::digest_of(&parsed.source);
            tree.files.insert(
                main_path.clone(),
                crate::document::history::TreeEntry {
                    kind: "text".to_string(),
                    id: main_id,
                    sha: main_sha.clone(),
                    size: parsed.source.len() as i64,
                },
            );
            bodies.insert(main_sha, parsed.source.clone());
            for (path, raw) in &parsed.files {
                match crate::document::paths::check(&self.config.paths(), path) {
                    Ok(crate::document::paths::Kind::Text) => {
                        // `preflight_directory` already required this to decode.
                        let Ok(body) = std::str::from_utf8(raw) else {
                            return Err(write_json(
                                400,
                                &json!({"error": format!("{path} is not valid UTF-8")}),
                            ));
                        };
                        let sha = crate::document::store::digest_of(body);
                        let id = tree
                            .files
                            .get(path)
                            .map(|entry| entry.id.clone())
                            .unwrap_or_default();
                        tree.files.insert(
                            path.clone(),
                            crate::document::history::TreeEntry {
                                kind: "text".to_string(),
                                id,
                                sha: sha.clone(),
                                size: body.len() as i64,
                            },
                        );
                        bodies.insert(sha, body.to_string());
                    }
                    Ok(crate::document::paths::Kind::Asset) => {
                        if let Some(entry) = new_assets.get(path) {
                            tree.files.insert(path.clone(), entry.clone());
                        }
                    }
                    Err(why) => return Err(write_json(400, &json!({"error": why}))),
                }
            }
            // Anything this upload does not name and the document did not
            // already have under one of these paths is gone: an upload is the
            // whole directory, so a chapter left out of it is a chapter
            // removed. ...but only when a directory was uploaded. A one-file
            // source upload -- the JSON body a browser upload sends,
            // which names no main -- is a new version of the main file, not a
            // claim that the document has no other files, and it has never
            // emptied a directory it was published over.
            if !parsed.main.is_empty() {
                tree.files.retain(|path, _| wanted.contains(path));
            }
            tree.main = main_path.clone();

            let before = crate::document::session::encode_vector(&state.session.doc);
            if let Err(error) = room.checked_edit(&state.session.doc, |candidate| {
                crate::document::session::restore(candidate, &tree, &bodies);
                Ok::<_, crate::room::WriteError>(())
            }) {
                drop(state);
                return Err(write_json(
                    error.status(),
                    &json!({"error": error.client_message()}),
                ));
            }
            if !parsed.source_format.is_empty() {
                state.session.format = parsed.source_format.clone();
            }
            state.session.dirty = true;
            // Every mutation of the document moves its generation; that is
            // what lets a checkpoint tell an edit that landed after its
            // snapshot from one it covered.
            state.session.generation += 1;
            state.session.updated_at = now_unix();
            crate::document::session::encode_diff(&state.session.doc, &before)
                .unwrap_or_else(|_| crate::document::session::encode_state(&state.session.doc))
        };
        let sha = match room
            .checkpoint("cli", crate::room::Attribution::account(&who.id, &who.key))
            .await
        {
            Ok(Some(sha)) => sha,
            // Deferred: the text is in the session and durable at the next
            // write, and the checkpoint follows when the window passes.
            Ok(None) => existing.sha.clone(),
            Err(error) => {
                if let Err(error) = room
                    .rollback_publication_inner(&current, &rollback_bodies, &rollback_format)
                    .await
                {
                    eprintln!("warning: could not roll back {}: {error}", existing.slug);
                }
                return Err(write_json(
                    error.status(),
                    &json!({"error": error.client_message()}),
                ));
            }
        };
        room.broadcast_editors_except(
            None,
            &json!({"type": "doc-update", "update": encode_update(&update)}),
        )
        .await;
        self.store
            .rename(&existing.slug, &title)
            .await
            .map_err(|error| write_json(409, &json!({"error": error})))?;
        let mut entry = self
            .store
            .get_result(&existing.slug)
            .await
            .map_err(|error| {
                write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            })?
            .unwrap_or_else(|| existing.clone());
        entry.sha = sha;
        // Comments survive the edit; they re-anchor in the reader. Everyone
        // with the document open is told, over the same socket their comments
        // arrive on.
        room.broadcast(&json!({"type": "published", "sha": entry.sha, "title": entry.title}))
            .await;
        Ok(entry)
    }

    /// Parses a publish request's body, in either format it may arrive as, and
    /// applies the checks common to both: a title and some HTML are present,
    /// and the HTML is not over the size ceiling. It answers the request
    /// itself on any problem, so `handle_upload` only has to decide where to
    /// store what comes back.
    #[allow(clippy::result_large_err)] // as publisher: the error is a response
    pub(super) async fn read_upload(&self, request: Request<Body>) -> Result<Upload, Reply> {
        let max_document = self.config.max_document;
        let content_type = header_of(request.headers(), "content-type").unwrap_or_default();
        let (mut title, mut slug, mut html) = (String::new(), String::new(), String::new());
        // A whole directory: the main file's name, and every other file in it.
        // Both stay empty for the one-file publish this route has always taken.
        let mut main = String::new();
        let mut sent: Vec<(String, Vec<u8>)> = Vec::new();
        let (mut source, mut source_format) = (String::new(), String::new());
        let mut requested_engine: Option<String> = None;
        let mut requested_draft_format: Option<String> = None;

        if content_type.contains("multipart/form-data") {
            // The whole request is bounded, not just the document: without
            // this an oversized body would be read in full before the HTML
            // limit below is even consulted. The slack covers the part
            // headers and the other fields; the document and the figure
            // budget are counted separately because a directory is allowed
            // both at once, not one shared between them.
            let ceiling = max_document + self.config.max_assets.max(0) as usize + MULTIPART_SLACK;
            let (parts, body) = request.into_parts();
            let limited = Request::from_parts(parts, Body::new(Limited::new(body, ceiling)));
            let mut multipart = match Multipart::from_request(limited, &()).await {
                Ok(multipart) => multipart,
                Err(_) => return Err(write_json(400, &json!({"error": "bad upload"}))),
            };
            let mut filename = String::new();
            loop {
                let field = match multipart.next_field().await {
                    Ok(Some(field)) => field,
                    Ok(None) => break,
                    Err(err) => return Err(upload_limit_exceeded(&err, ceiling)),
                };
                let name = field.name().unwrap_or_default().to_string();
                match name.as_str() {
                    "title" => title = field.text().await.unwrap_or_default(),
                    "slug" => slug = field.text().await.unwrap_or_default(),
                    // Which file is the document. A directory sends it; a
                    // single file does not, and is named by its own filename.
                    "main" => main = field.text().await.unwrap_or_default(),
                    "execution_engine" => {
                        requested_engine = Some(field.text().await.unwrap_or_default())
                    }
                    "draft_format" => {
                        requested_draft_format = Some(field.text().await.unwrap_or_default())
                    }
                    "file" => {
                        // A directory arrives as several of these, each named
                        // by its path within the document. One of them is the
                        // one-file publish this route has always taken.
                        let at = field.file_name().unwrap_or_default().to_string();
                        let bytes = match field.bytes().await {
                            Ok(bytes) => bytes,
                            Err(err) => return Err(upload_limit_exceeded(&err, ceiling)),
                        };
                        sent.push((at, bytes.to_vec()));
                    }
                    _ => {}
                }
            }
            // Which of them is the document, and what the rest are. A single
            // part is what this route has always taken and keeps its meaning;
            // several is a directory, and one of them has to be named.
            if sent.len() == 1 && main.is_empty() {
                let (at, bytes) = &sent[0];
                filename = at.clone();
                html = String::from_utf8_lossy(&bytes[..bytes.len().min(max_document + 1)])
                    .to_string();
                sent.clear();
            } else if !sent.is_empty() {
                let wanted = if main.is_empty() {
                    sent[0].0.clone()
                } else {
                    main.clone()
                };
                let Some(at) = sent.iter().position(|(path, _)| *path == wanted) else {
                    return Err(write_json(
                        400,
                        &json!({"error": format!("the main file {wanted} is not among the files sent")}),
                    ));
                };
                let (path, bytes) = sent.remove(at);
                main = path.clone();
                filename = path;
                html = String::from_utf8_lossy(&bytes[..bytes.len().min(max_document + 1)])
                    .to_string();
            }
            // Markdown dropped on the page is stored as markdown. It is not
            // rendered here and never was worth rendering here: the browser
            // showing it renders it, with the same module the editor previews
            // with.
            //
            // What the file is called is the whole of the question, and
            // `document_format` is the one place it is answered. The command
            // line asks it too, and a directory whose main file this route
            // named differently from the way `publish` named it would be a
            // document that opened in the wrong editor depending on how it
            // arrived. Typst and LaTeX reach here from `publish <directory>`
            // rather than from the upload form, which takes what a browser can
            // drop; a paper is the ordinary case either way.
            if let Some(format) = crate::document::render::document_format(&filename) {
                if title.trim().is_empty() {
                    title = match format {
                        "typst" => crate::document::render::title_from_typst(&html),
                        "markdown" => title_from_markdown(&html),
                        "quarto" => crate::document::render::title_from_quarto(&html),
                        // There is no TeX here to ask, so the `\title` is
                        // scanned for; see `render::title_from_latex`.
                        "latex" => crate::document::render::title_from_latex(&html),
                        // An HTML document's source is its own bytes, through
                        // the identity renderer, so it opens in the editor like
                        // the others.
                        _ => title_from_html(&html),
                    };
                }
                source = html.clone();
                source_format = format.to_string();
            }
        } else {
            // JSON escaping can inflate the document, so the body is allowed to
            // be larger than the document limit; the real check is on the
            // decoded html below. Refusing early keeps a huge body from being
            // read at all, and says why rather than failing to parse.
            let ceiling = max_document * 2 + 1024;
            if let Some(length) =
                header_of(request.headers(), "content-length").and_then(|v| v.parse::<usize>().ok())
            {
                if length > ceiling {
                    return Err(write_json(413, &json!({"error": "document too large"})));
                }
            }
            #[derive(Deserialize, Default)]
            struct Body_ {
                #[serde(default)]
                title: String,
                #[serde(default)]
                slug: String,
                #[serde(default)]
                html: String,
                #[serde(default)]
                source: String,
                #[serde(default)]
                source_format: String,
                #[serde(default)]
                execution_engine: Option<String>,
                #[serde(default)]
                draft_format: Option<String>,
            }
            let Ok(bytes) = to_bytes(request.into_body(), ceiling).await else {
                return Err(write_json(413, &json!({"error": "document too large"})));
            };
            let Ok(body) = serde_json::from_slice::<Body_>(&bytes) else {
                return Err(write_json(400, &json!({"error": "bad request"})));
            };
            title = body.title;
            slug = body.slug;
            html = body.html;
            source = body.source;
            source_format = body.source_format;
            requested_engine = body.execution_engine;
            requested_draft_format = body.draft_format;
        }

        // Nothing derived is stored, so what arrives has to be the document
        // itself. A caller that sends only HTML -- a page dropped on the
        // upload form, or a client written against the older API -- has sent a
        // document whose source is that HTML and whose renderer is the
        // identity, which is what `html` has meant since it became a source
        // format like the other two.
        if source.is_empty() && !html.trim().is_empty() {
            source = html;
            source_format = "html".to_string();
        }
        if !self.config.storable_source(&source_format) {
            return Err(write_json(
                400,
                &json!({"error": "this deployment cannot store a document in that format"}),
            ));
        }
        let expected_engine = crate::results::document_metadata(&source_format).execution_engine;
        let expected_draft = crate::results::document_metadata(&source_format).draft_format;
        if let Some(raw_engine) = requested_engine {
            let supplied = crate::results::ExecutionEngine::parse(&raw_engine)
                .map_err(|error| write_json(400, &json!({"error": error})))?;
            if supplied != expected_engine {
                return Err(write_json(
                    400,
                    &json!({"error": "execution engine does not match source_format"}),
                ));
            }
        }
        if let Some(raw_draft) = requested_draft_format {
            let supplied = crate::results::DraftFormat::parse(&raw_draft)
                .map_err(|error| write_json(400, &json!({"error": error})))?;
            if supplied != expected_draft {
                return Err(write_json(
                    400,
                    &json!({"error": "draft format does not match source_format"}),
                ));
            }
        }
        if source.len() > max_document {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }

        let title = title.trim().to_string();
        if title.is_empty() || source.trim().is_empty() {
            return Err(write_json(
                400,
                &json!({"error": "title and a document are required"}),
            ));
        }
        // Stripped of control characters the same way every other free-text
        // field is, then capped: refused rather than truncated, and before
        // anything is written, so a caller sees the limit rather than a
        // silently shortened title.
        let title = clean(&title, title.chars().count());
        if title.chars().count() > self.config.max_title {
            return Err(write_json(400, &json!({"error": "title too long"})));
        }
        // Every path a directory sent, checked before any of it is stored. The
        // command line refuses these first so the reason arrives on the
        // laptop; this is the enforcement, and it is the same rule.
        let mut kept = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        if !main.is_empty() {
            match crate::document::paths::check(&self.config.paths(), &main) {
                Ok(crate::document::paths::Kind::Text) => {}
                Ok(crate::document::paths::Kind::Asset) => {
                    return Err(write_json(
                        400,
                        &json!({"error": format!("{main} cannot be the main file; it is a figure")}),
                    ))
                }
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
            seen.insert(crate::document::paths::collision_key(&main));
        }
        for (path, bytes) in sent {
            let kind = match crate::document::paths::check(&self.config.paths(), &path) {
                Ok(kind) => kind,
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            };
            if !seen.insert(crate::document::paths::collision_key(&path)) {
                return Err(write_json(
                    400,
                    &json!({"error": format!("{path}: two files cannot share one name")}),
                ));
            }
            kept.push((crate::document::paths::normalise(&path), kind, bytes));
        }
        if seen.len() > self.config.max_files {
            return Err(write_json(
                413,
                &json!({"error": format!("a document may hold {} files", self.config.max_files)}),
            ));
        }
        // The texts are measured together with the main file, because that is
        // what the ceiling bounds: a paper split into thirty files is allowed
        // exactly what a paper in one file is allowed.
        let texts: usize = kept
            .iter()
            .filter(|(_, kind, _)| *kind == crate::document::paths::Kind::Text)
            .map(|(_, _, bytes)| bytes.len())
            .sum();
        if source.len() + texts > max_document {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }
        let figures: i64 = kept
            .iter()
            .filter(|(_, kind, _)| *kind == crate::document::paths::Kind::Asset)
            .map(|(_, _, bytes)| bytes.len() as i64)
            .sum();
        if figures > self.config.max_assets {
            return Err(write_json(
                413,
                &json!({"error": format!(
                    "this document has reached the {} MB it may keep in figures",
                    self.config.max_assets >> 20
                )}),
            ));
        }

        Ok(Upload {
            title,
            slug,
            source,
            source_format,
            main,
            files: kept
                .into_iter()
                .map(|(path, _, bytes)| (path, bytes))
                .collect(),
        })
    }

    /// Checks a whole directory upload against the size and encoding rules
    /// before any of it touches storage or the session. A document is a
    /// directory, so it is admitted whole or refused whole, naming the file
    /// and the rule it broke -- never accepted with the eleventh file silently
    /// missing because it was the one that turned out to be too large.
    ///
    /// `main_path` is where the upload's main file will live and `current` is
    /// the directory as it stands before this upload lands, which is empty
    /// for a creation and the live tree for a republish: a file this upload
    /// does not mention is judged by whether the document already keeps it,
    /// not refused twice or forgotten from the aggregate ceilings.
    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    pub(super) fn preflight_directory(
        &self,
        parsed: &Upload,
        main_path: &str,
        current: &crate::document::history::Tree,
    ) -> Result<(), Reply> {
        for (path, bytes) in &parsed.files {
            match crate::document::paths::check(&self.config.paths(), path) {
                Ok(crate::document::paths::Kind::Text) => {
                    if std::str::from_utf8(bytes).is_err() {
                        return Err(write_json(
                            400,
                            &json!({"error": format!("{path} is not valid UTF-8")}),
                        ));
                    }
                }
                Ok(crate::document::paths::Kind::Asset) => {
                    if bytes.len() as i64 > self.config.max_asset {
                        return Err(write_json(
                            413,
                            &json!({"error": format!(
                                "{path} is larger than the {} MB one file may be",
                                self.config.max_asset >> 20
                            )}),
                        ));
                    }
                }
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
        }
        // Build the resulting tree's retained payload.  A directory upload
        // replaces the directory, so absent old files are deleted and must
        // not be counted; paths that survive but are not part of this upload
        // (or are replaced by it) retain their old/new entry respectively.
        let directory = !parsed.main.is_empty();
        let mut resulting: HashMap<String, (String, String, i64)> = HashMap::new();
        for (path, entry) in &current.files {
            if directory && !parsed.files.iter().any(|(sent, _)| sent == path) && path != main_path
            {
                continue;
            }
            resulting.insert(
                path.clone(),
                (entry.kind.clone(), entry.sha.clone(), entry.size),
            );
        }
        resulting.insert(
            main_path.to_string(),
            (
                "text".to_string(),
                crate::document::store::digest_of(&parsed.source),
                parsed.source.len() as i64,
            ),
        );
        for (path, bytes) in &parsed.files {
            let kind = crate::document::paths::check(&self.config.paths(), path)
                .expect("validated directory path");
            let (kind, sha) = match kind {
                crate::document::paths::Kind::Text => (
                    "text".to_string(),
                    crate::document::store::digest_of(
                        std::str::from_utf8(bytes).expect("validated UTF-8 text"),
                    ),
                ),
                crate::document::paths::Kind::Asset => (
                    "asset".to_string(),
                    crate::document::store::digest_of_bytes(bytes),
                ),
            };
            resulting.insert(path.clone(), (kind, sha, bytes.len() as i64));
        }
        let mut by_sha: HashMap<String, i64> = HashMap::new();
        for (kind, sha, size) in resulting.values() {
            if kind == "asset" {
                by_sha.entry(sha.clone()).or_insert(*size);
            }
        }
        let assets_total: i64 = by_sha.values().sum();
        if assets_total > self.config.max_assets {
            return Err(write_json(
                413,
                &json!({"error": format!(
                    "this document has reached the {} MB it may keep in figures",
                    self.config.max_assets >> 20
                )}),
            ));
        }
        let text_total: usize = resulting
            .values()
            .filter(|(kind, _, _)| kind == "text")
            .map(|(_, _, size)| *size as usize)
            .sum();
        if text_total > self.config.max_document {
            return Err(write_json(413, &json!({"error": "document too large"})));
        }
        Ok(())
    }

    pub(super) async fn handle_state(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let (entry, who) = match self.entry_viewer(slug, headers, arrival, query).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        // The signature says this link was minted here; it does not say who is
        // holding it. Who may read is asked again, from the request itself,
        // which is the same rule the socket answers `y-open` under.
        // Full document state is a source synchronization transport. Readers and
        // commenters receive the rendered publication and annotation channel.
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return plain(404, "not found");
        }
        let fields: HashMap<String, String> = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let until = fields
            .get("until")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let token = fields.get("token").cloned().unwrap_or_default();
        if until < crate::util::now_unix()
            || !crate::auth::verifies(
                &self.key,
                "socket-state-v1",
                &format!("state:{slug}:{until}"),
                &token,
            )
        {
            return plain(403, "that link has expired");
        }
        // Cross-site fetches are refused here as everywhere else: a document's
        // source is not another site's to read out of a signed-in browser.
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let (state, _) = room.open_state(None).await;
        if room.agent_recovery_pending() {
            return plain(503, "room state is awaiting recovery");
        }
        let mut response = Response::new(Body::from(state));
        set(&mut response, "content-type", "application/octet-stream");
        set(&mut response, "cache-control", "no-store");
        privacy_headers(&mut response);
        response
    }

    /// The source of a document, which is the document. Readable by anyone
    /// who may read it.
    pub(super) async fn handle_source(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // Source is editable project material. Readers receive the current
        // publication through the publication route and never this endpoint.
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let source = room.source().await;
        if room.agent_recovery_pending() {
            return plain(503, "room state is awaiting recovery");
        }
        let format = {
            let held = room.format().await;
            if held.is_empty() {
                if entry.source_format.is_empty() {
                    "html".to_string()
                } else {
                    entry.source_format.clone()
                }
            } else {
                held
            }
        };
        write_json(
            200,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                "format": format, "source": source,
            }),
        )
    }

    pub(super) async fn handle_snapshot(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let author = self.comment_author(headers, arrival, &who.id);
        let may_edit = who.at_least(Role::Editor);
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let (source, held_format, tree, texts, comments) =
            room.snapshot_bundle(&author, may_edit).await;
        if room.agent_recovery_pending() {
            return plain(503, "room state is awaiting recovery");
        }
        let format = if held_format.is_empty() {
            if entry.source_format.is_empty() {
                "html".to_string()
            } else {
                entry.source_format.clone()
            }
        } else {
            held_format
        };
        let files = tree.files.clone();
        let tree_sha = tree.digest();
        write_json(
            200,
            &json!({
                "version": 1,
                "protocol": "librepaper.snapshot.v1",
                "slug": entry.slug,
                "title": entry.title,
                "format": format,
                "main": tree.main,
                "tree": tree,
                "files": files,
                "texts": texts,
                "source": source,
                // Canonical live tree identity for assistant anchors. This
                // remains useful before a quiet checkpoint is written.
                "sha": tree_sha,
                "source_sha": crate::document::store::digest_of(&source),
                "comments": comments,
                "role": who.role.as_str(),
                "capabilities": {
                    "read": true,
                    "comment": who.at_least(Role::Commenter),
                    "edit": who.at_least(Role::Editor),
                },
            }),
        )
    }

    pub(super) async fn handle_delete(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let who = match self.publisher(headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        // Another publisher's document answers exactly as a missing one does,
        // so a guessed slug reveals nothing.
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) if entry.owned_by(&who.key, &who.id) => entry,
            Ok(Some(_)) | Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        match self.delete_document(slug).await {
            Ok(removed) => write_json(
                200,
                &json!({"deleted": slug, "title": entry.title, "objects_removed": removed}),
            ),
            Err(error) => {
                eprintln!("could not remove {slug}: {error}");
                write_json(500, &json!({"error": "could not remove the document"}))
            }
        }
    }
}
