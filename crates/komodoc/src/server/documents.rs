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

pub(super) fn upload_digest(upload: &Upload) -> String {
    let mut hasher = Sha256::new();
    hasher.update(upload.source.as_bytes());
    for (path, body) in &upload.files {
        hasher.update(path.as_bytes());
        hasher.update(body);
    }
    hex::encode(hasher.finalize())
}

impl Server {
    pub async fn delete_document(&self, slug: &str) -> Result<usize, String> {
        let storage_id = self.store.begin_delete(slug)?;
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
                let address = client_address(peer, &headers);
                let (result, ok) = if incoming.kind == "accept" || incoming.kind == "reject" {
                    if who.at_least(Role::Editor) && !current_who.at_least(Role::Editor) {
                        return write_json(403, &json!({"error": "edit access changed"}));
                    }
                    let by = if current_who.key.is_empty() {
                        current_who.id.handle.clone()
                    } else {
                        current_who.key.clone()
                    };
                    self.decide_suggestion(
                        &room,
                        &incoming,
                        current_who.at_least(Role::Editor),
                        &by,
                    )
                    .await
                } else {
                    self.apply_from(&room, incoming, &address, &current_who, &author)
                        .await
                };
                if ok {
                    let shared = room.comment_event_for(&result, "", false).await;
                    room.broadcast(&shared).await;
                    let targeted = room.comment_event_for(&result, &author, may_edit).await;
                    return write_json(200, &targeted);
                }
                write_json(400, &result)
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
            Ok(existing) => match existing {
                Some(existing) => Some(existing),
                None => match self.store.pending_publication_result(&base) {
                    Ok(pending) => pending,
                    Err(error) => {
                        return write_json(
                            503,
                            &json!({"error": error.to_string(), "retryable": true}),
                        )
                    }
                },
            },
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
        // Publishing over a document that already exists is an edit into its
        // live session rather than a new version beside the old one. There is
        // one document, so there is nothing to conflict with: the source the
        // command line sends is diffed into the session, so an editor typing
        // at that moment keeps their words and sees the rest change under
        // them, and the write is marked with a checkpoint.
        if mine {
            if let Err(error) = self.store.admit_replacement_upload(&key) {
                return match error {
                    PutError::Quota { status, message } => {
                        write_json(status, &json!({"error": message}))
                    }
                    PutError::Authorization { status, message } => {
                        write_json(status, &json!({"error": message}))
                    }
                    PutError::Storage(_) => {
                        write_json(500, &json!({"error": "could not admit the replacement"}))
                    }
                };
            }
            let room = match self.rooms.try_get(&key).await {
                Ok(room) => room,
                Err(error) => {
                    return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
                }
            };
            let entry = match self
                .edit_into_session(&room, &parsed, &who, &existing.unwrap())
                .await
            {
                Ok(entry) => entry,
                Err(response) => return response,
            };
            // A revision changes the text and not the sharing: the read link
            // it hands back is the one the document already has, or none if
            // the owner took it away, which is theirs to have done.
            let share_url = Self::read_link_of(&entry);
            return write_json(
                201,
                &json!({
                    "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                    "created_at": entry.created_at, "updated_at": entry.updated_at,
                    "url": format!("/docs/{}", entry.slug),
                    "share_url": share_url,
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
            .put(Publication {
                slug: key.clone(),
                title: parsed.title.clone(),
                source: parsed.source.clone(),
                source_format: parsed.source_format.clone(),
                main: main.clone(),
                owner: who.key,
                owner_id: who.id,
                owner_name: who.name,
                peak_bytes: Some(self.exact_publication_peak(&parsed, &main)),
            })
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
        if self.store.catalog.is_some() {
            if let Err(error) = self
                .store
                .prepare_publication(&key, &upload_digest(&parsed), "publish", None)
                .await
            {
                let _ = self
                    .store
                    .abort_publication(&key, &format!("prepare failed: {error}"))
                    .await;
                return write_json(409, &json!({"error": error}));
            }
        }
        // The document itself is the session, and the session's first
        // checkpoint is the source it was published with. Written here rather
        // than by the store, because it is the room that owns the document.
        let room = match self.rooms.try_get(&key).await {
            Ok(room) => room,
            Err(error) => {
                return write_json(503, &json!({"error": error.to_string(), "retryable": true}))
            }
        };
        let mut publication_token = match room.reserve_publication_checkpoint() {
            Ok(token) => token,
            Err(err) => {
                let _ = self
                    .store
                    .abort_publication(&key, &format!("checkpoint admission failed: {err}"))
                    .await;
                if let Err(cleanup) = self.delete_document(&key).await {
                    eprintln!("warning: could not undo refused creation of {key}: {cleanup}");
                }
                return write_json(429, &json!({"error": err, "retryable": true}));
            }
        };
        room.set_main_file(&parsed.source, &parsed.source_format, &parsed.main)
            .await;
        // The rest of the directory, if a whole one was published. The texts
        // go into the shared document beside the main file; the figures go to
        // the store under their digests and are named in it. The preflight
        // above already ruled out every refusal this can still hit, so a
        // failure here is a storage fault, not a bad upload -- and a creation
        // whose source only ever made it into RAM is not a creation that
        // happened, so it is undone the way the delete route removes a
        // document rather than answered with 201.
        if !parsed.files.is_empty() {
            if let Err(why) = self.fill_directory(&room, &parsed).await {
                eprintln!("warning: could not store every file of {key}: {why}");
                let _ = self
                    .store
                    .abort_publication(&key, &format!("directory fill failed: {why}"))
                    .await;
                if let Err(err) = self.delete_document(&key).await {
                    eprintln!("warning: could not undo the creation of {key}: {err}");
                }
                return write_json(500, &json!({"error": "could not store the document"}));
            }
        }
        // The checkpoint names itself, and what it is named is the digest of
        // the tree rather than of the source: a document is a directory, so
        // what the index points at is the directory this document was at. A
        // creation is successful only once this has landed durably -- until
        // then the only copy of the source is in RAM, and a response saying
        // otherwise would describe a document a crash could still make
        // disappear. So a failure here undoes the creation instead of
        // answering with the SHA `put` wrote before the tree existed.
        let sha = match room
            .checkpoint_publication_now("cli", &entry.publisher, &mut publication_token)
            .await
        {
            Ok(Some(sha)) => sha,
            Ok(None) => entry.sha.clone(),
            Err(err) => {
                eprintln!("warning: could not checkpoint {key}: {err}");
                let _ = self
                    .store
                    .abort_publication(&key, &format!("checkpoint failed: {err}"))
                    .await;
                if let Err(err) = self.delete_document(&key).await {
                    eprintln!("warning: could not undo the creation of {key}: {err}");
                }
                return write_json(500, &json!({"error": "could not store the document"}));
            }
        };
        if let Err(err) = self.store.commit_publication(&key, &sha).await {
            eprintln!("warning: could not commit publication {key}: {err}");
            let _ = self
                .store
                .abort_publication(&key, &format!("commit failed: {err}"))
                .await;
            if let Err(err) = self.delete_document(&key).await {
                eprintln!("warning: could not undo the creation of {key}: {err}");
            }
            return write_json(500, &json!({"error": "could not store the document"}));
        }
        publication_token.commit();
        // A document only its owner can open is not published in any useful
        // sense, so the upload mints the read link and hands it back beside
        // the bare URL: what `komodoc publish` prints is the thing to send.
        // It has no expiry, because it stands where the URL itself used to
        // stand, and that never expired; the owner shortens, rotates or
        // revokes it from the share dialog like any other link. A failure
        // to record it is not a failure to publish -- the document is there
        // and the dialog can mint one -- so it is reported and not fatal.
        let share_url = match self.mint_read_link(&key).await {
            Ok(url) => Value::String(url),
            Err(err) => {
                eprintln!("warning: could not mint the read link of {key}: {err:?}");
                Value::Null
            }
        };
        write_json(
            201,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": sha,
                "created_at": entry.created_at, "updated_at": entry.updated_at,
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

        let request_digest = upload_digest(parsed);
        if self.store.catalog.is_some() {
            let actor = crate::document::store::MutationActor {
                account_id: who.id.clone(),
                owner_key: who.key.clone(),
                session_generation: who.session_generation.clone(),
                link_hash: String::new(),
                policy_editor: true,
                automation: false,
                unowned_publisher: false,
            };
            self.store
                .prepare_publication(&existing.slug, &request_digest, "replace", Some(&actor))
                .await
                .map_err(|error| write_json(409, &json!({"error": error})))?;
            if let Err(error) = self.store.reserve_publication_peak(
                &existing.slug,
                self.exact_publication_peak(parsed, &main_path),
            ) {
                let _ = self.store.abort_publication(&existing.slug, &error).await;
                return Err(write_json(507, &json!({"error": error})));
            }
        }
        let mut publication_token = match room.reserve_publication_checkpoint() {
            Ok(token) => token,
            Err(error) => {
                let _ = self.store.abort_publication(&existing.slug, &error).await;
                return Err(write_json(429, &json!({"error": error, "retryable": true})));
            }
        };

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
                    .put_asset_unlocked(
                        raw.clone(),
                        (self.config.max_asset, self.config.max_assets),
                    )
                    .await
                {
                    Ok(result) => result,
                    Err(why) => {
                        eprintln!(
                            "warning: could not store {path} of {}: {why}",
                            existing.slug
                        );
                        let _ = self
                            .store
                            .abort_publication(
                                &existing.slug,
                                &format!("asset staging failed: {why}"),
                            )
                            .await;
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
            // already have under one of these paths is gone: an upload is
            // the whole directory, so a chapter left out of it is a chapter
            // removed.
            // ...but only when a directory was uploaded. A one-file publish
            // -- the JSON body `komodoc publish paper.md` sends, which names
            // no main -- is a new version of the main file, not a claim that
            // the document has no other files, and it has never emptied a
            // directory it was published over.
            if !parsed.main.is_empty() {
                tree.files.retain(|path, _| wanted.contains(path));
            }
            tree.main = main_path.clone();

            let before = crate::document::session::encode_vector(&state.session.doc);
            crate::document::session::restore(&state.session.doc, &tree, &bodies);
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
        // A title given on the command line renames the document; an empty one
        // leaves it as it is.
        let title = if parsed.title.is_empty() {
            existing.title.clone()
        } else {
            parsed.title.clone()
        };
        let sha = match room
            .checkpoint_publication_now_locked("cli", &who.key, &mut publication_token)
            .await
        {
            Ok(Some(sha)) => sha,
            // Deferred: the text is in the session and durable at the next
            // write, and the checkpoint follows when the window passes.
            Ok(None) => existing.sha.clone(),
            Err(_) => {
                if let Err(error) = room
                    .rollback_publication_inner(&current, &rollback_bodies, &rollback_format)
                    .await
                {
                    eprintln!("warning: could not roll back {}: {error}", existing.slug);
                }
                let _ = self
                    .store
                    .abort_publication(&existing.slug, "checkpoint failed")
                    .await;
                return Err(write_json(
                    500,
                    &json!({"error": "could not store the document"}),
                ));
            }
        };
        if self.store.catalog.is_some() {
            if let Err(error) = self.store.commit_publication(&existing.slug, &sha).await {
                if let Err(rollback) = room
                    .rollback_publication_inner(&current, &rollback_bodies, &rollback_format)
                    .await
                {
                    eprintln!("warning: could not roll back {}: {rollback}", existing.slug);
                }
                let _ = self
                    .store
                    .abort_publication(&existing.slug, &format!("commit failed: {error}"))
                    .await;
                return Err(write_json(
                    500,
                    &json!({"error": "could not commit the publication"}),
                ));
            }
        }
        publication_token.commit();
        room.broadcast(&json!({"type": "y-update", "update": encode_update(&update)}))
            .await;
        if let Err(err) = self.store.rename(&existing.slug, &title).await {
            eprintln!("warning: could not rename {}: {err}", existing.slug);
        }
        let mut entry = self
            .store
            .get(&existing.slug)
            .await
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

    /// Calculate the initial object peak before admission.  The room's first
    /// checkpoint materializes one object for each unique text body, each
    /// unique asset, its canonical tree, and its encoded fresh CRDT session.
    /// Build that same shape in memory so quota admission is exact rather than
    /// relying on an arbitrary safety cushion.
    pub(super) fn exact_publication_peak(&self, parsed: &Upload, main_path: &str) -> i64 {
        let mut tree = Tree {
            main: main_path.to_string(),
            ..Tree::default()
        };
        let mut bodies = HashMap::new();
        let main_sha = crate::document::store::digest_of(&parsed.source);
        bodies.insert(main_sha.clone(), parsed.source.clone());
        tree.files.insert(
            main_path.to_string(),
            TreeEntry {
                kind: "text".to_string(),
                id: "000000000000".to_string(),
                sha: main_sha,
                size: parsed.source.len() as i64,
            },
        );
        for (path, raw) in &parsed.files {
            match crate::document::paths::check(&self.config.paths(), path) {
                Ok(crate::document::paths::Kind::Text) => {
                    if let Ok(body) = std::str::from_utf8(raw) {
                        let sha = crate::document::store::digest_of(body);
                        bodies
                            .entry(sha.clone())
                            .or_insert_with(|| body.to_string());
                        tree.files.insert(
                            path.clone(),
                            TreeEntry {
                                kind: "text".to_string(),
                                id: "000000000000".to_string(),
                                sha,
                                size: raw.len() as i64,
                            },
                        );
                    }
                }
                Ok(crate::document::paths::Kind::Asset) => {
                    let sha = crate::document::store::digest_of_bytes(raw);
                    tree.files.insert(
                        path.clone(),
                        TreeEntry {
                            kind: "asset".to_string(),
                            id: String::new(),
                            sha,
                            size: raw.len() as i64,
                        },
                    );
                }
                Err(_) => {}
            }
        }
        let doc = crate::document::session::new_doc();
        crate::document::session::replace_text(&doc, &parsed.source, main_path);
        for (path, raw) in &parsed.files {
            match crate::document::paths::check(&self.config.paths(), path) {
                Ok(crate::document::paths::Kind::Text) => {
                    if let Ok(body) = std::str::from_utf8(raw) {
                        crate::document::session::put_text(&doc, path, body);
                    }
                }
                Ok(crate::document::paths::Kind::Asset) => {
                    crate::document::session::put_asset(
                        &doc,
                        path,
                        &crate::document::store::digest_of_bytes(raw),
                    );
                }
                Err(_) => {}
            }
        }
        let text_bytes: i64 = bodies.values().map(|body| body.len() as i64).sum();
        let mut assets = HashMap::new();
        for entry in tree.files.values().filter(|entry| entry.kind == "asset") {
            assets.entry(entry.sha.clone()).or_insert(entry.size);
        }
        let asset_bytes: i64 = assets.values().sum();
        let session_bytes = crate::document::session::encode_state(&doc);
        // Production local catalogues attach the journal.  Its first segment
        // identity has a fixed 32-hex storage id; the actual id has the same
        // length, so this is exact before Store::put allocates it.
        let journal_attached = self.rooms.journal_attached();
        let journal_bytes = if journal_attached {
            crate::storage::journal::initial_segment_bytes(&"0".repeat(32), &session_bytes)
                .unwrap_or(0) as i64
        } else {
            0
        };
        let session_object_bytes = if journal_attached {
            0
        } else {
            session_bytes.len() as i64
        };
        // The room can advance its journal cursor while admission is being
        // completed (and the framing retry identity includes that cursor).
        // Keep a small fixed framing allowance so the preflight reservation
        // remains conservative at the exact quota boundary.
        // Checkpoint accounting stores the logical tree payload size (which
        // includes asset entries), while the live object inventory charges
        // the asset blobs as well.  Mirror that two-sided accounting here so
        // admission cannot under-reserve a directory containing figures.
        text_bytes
            .saturating_add(asset_bytes)
            .saturating_add(asset_bytes)
            .saturating_add(tree.to_bytes().len() as i64)
            .saturating_add(session_object_bytes)
            .saturating_add(journal_bytes)
            .saturating_add(1024)
    }

    /// Puts the rest of a published directory where it belongs: every text
    /// into the shared document, every figure into the store with its name
    /// written beside its digest.
    ///
    /// The main file is already in the session -- `set_source` put it there --
    /// so this adds the others and names none of them the document.
    /// `preflight_directory` has already ruled out every refusal this can
    /// still hit, so a failure here is a storage fault: the caller decides
    /// whether that leaves an inconsistent document worth undoing.
    pub(super) async fn fill_directory(&self, room: &Room, parsed: &Upload) -> Result<(), String> {
        for (path, bytes) in &parsed.files {
            match crate::document::paths::check(&self.config.paths(), path) {
                Ok(crate::document::paths::Kind::Text) => {
                    let body = std::str::from_utf8(bytes)
                        .map_err(|_| format!("{path} is not valid UTF-8"))?;
                    room.add_text(path, body).await;
                }
                Ok(crate::document::paths::Kind::Asset) => {
                    let (sha, _) = room
                        .put_asset(
                            bytes.clone(),
                            (self.config.max_asset, self.config.max_assets),
                        )
                        .await?;
                    room.name_asset(path, &sha).await;
                }
                Err(why) => return Err(why),
            }
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
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return plain(404, "not found"),
            Err(response) => return response,
        };
        // The signature says this link was minted here; it does not say who is
        // holding it. Who may read is asked again, from the request itself,
        // which is the same rule the socket answers `y-open` under.
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
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
        // "Anyone who may read the document" is the reader role as this spec
        // defines it, which for a private document is the people named on it.
        let who = self.viewer(&entry, headers, arrival, query).await;
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // The source is readable by anyone who may read the document. It has
        // to be: the browser cannot render what it is not given, and nothing
        // rendered is stored any more. `docs/specs/history.md` accepts that and
        // offers no way around it -- a source that must not be seen is not
        // published here as that source.
        //
        // What is answered is the live document, not a stored copy of it:
        // there is one version, and this is it.
        let room = match self.rooms.try_get(slug).await {
            Ok(room) => room,
            Err(error) => return plain(503, &error.to_string()),
        };
        let source = room.source().await;
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
        if !self.may_read(&entry, &who) {
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
                "protocol": "komodoc.snapshot.v1",
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
                &json!({"deleted": slug, "title": entry.title, "versions_removed": removed}),
            ),
            Err(error) => {
                eprintln!("could not remove {slug}: {error}");
                write_json(500, &json!({"error": "could not remove the document"}))
            }
        }
    }
}
