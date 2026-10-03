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

#[allow(clippy::result_large_err)] // the invalid-source response is an HTTP reply
fn main_source(path: &str, bytes: &[u8]) -> Result<String, Reply> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| write_json(400, &json!({"error": format!("{path} is not valid UTF-8")})))
}

#[derive(serde::Serialize)]
struct SnapshotResponse<C> {
    version: u8,
    protocol: &'static str,
    slug: String,
    title: String,
    format: String,
    main: String,
    #[serde(flatten)]
    project: librepaper_document::document::projection::ProjectSnapshot,
    files: std::collections::BTreeMap<String, librepaper_document::document::projection::Entry>,
    source: String,
    source_sha: String,
    comments: C,
    comment_state: Value,
    role: String,
    capabilities: Value,
}

impl Server {
    pub async fn delete_document(&self, slug: &str) -> Result<usize, String> {
        let storage_id = self.store.begin_delete(slug).await?;
        // Tear down the document's live chat channels only after deletion is
        // admitted; a refused delete must leave those conversations running.
        self.chat.purge(slug).await;
        // The room, if one is open, is dropped; the sequencer underneath it
        // retires on the registry's own schedule after flushing (§4.3). What
        // marks the document for deletion is `documents.status = 'deleting'`
        // (already written by `begin_delete`), which is what the background
        // worker's startup scan and wake both look for (§8.6) -- there is no
        // job row to enqueue here.
        self.rooms.close(slug).await;
        if let Some(storage_id) = storage_id
            .as_deref()
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
        {
            self.background
                .ask(librepaper_engine::storage::worker::Task::Delete(storage_id));
        }
        self.store.remove(slug).await
    }

    /// Removes every document older than `retention` seconds, measured from
    /// `from`. Returns how many went.
    pub async fn delete_expired(&self, now: i64, retention: i64, from: &str) -> usize {
        let mut removed = 0;
        let cutoff = now - retention;
        let mut before = None;
        loop {
            let (entries, next) = match self.store.maintenance_page(before, 200).await {
                Ok(page) => page,
                Err(error) => {
                    eprintln!("could not enumerate documents for retention: {error}");
                    return removed;
                }
            };
            if entries.is_empty() {
                break;
            }
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
            let Some(next) = next else { break };
            before = Some(next);
        }
        removed
    }

    /// One page of a document's comments, as `librepaper.comments.v1`.
    ///
    /// The whole collection is never assembled: `?cursor=` continues the
    /// traversal and `?limit=` narrows the page. Authorization has already
    /// run at the call site and runs again on the next request -- holding a
    /// cursor is a position, not a permission.
    async fn comment_page_reply(
        &self,
        room: &Arc<librepaper_room::room::Room>,
        query: Option<&str>,
        author: &str,
        may_edit: bool,
    ) -> Reply {
        let values: HashMap<String, String> = query
            .map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let limit = values
            .get("limit")
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(librepaper_room::room::comments::COMMENT_PAGE_DEFAULT);
        let after = match values.get("cursor") {
            Some(raw) => {
                match librepaper_room::room::comments::decode_cursor(raw, room.document_id, None, may_edit) {
                    Ok(position) => Some(position),
                    Err(error) => return write_json(400, &json!({"error": error.to_string()})),
                }
            }
            None => None,
        };
        // The counts come first so a page and the totals beside it are read
        // in the same order every time; both are their own statement, and
        // neither holds a connection past its own answer.
        let state = match room.comment_state(may_edit).await {
            Ok(state) => state,
            Err(error) => return refused("read comment state", &error),
        };
        match room.comment_page(after, limit, author, may_edit).await {
            Ok((views, page)) => write_json(
                200,
                &json!({
                    "version": 1,
                    "protocol": "librepaper.comments.v1",
                    "comments": views,
                    "next_cursor": page.next.map(|at| librepaper_room::room::comments::encode_cursor(
                        room.document_id, None, may_edit, at)),
                    "complete": page.complete,
                    "oversize": page.oversize,
                    "state": librepaper_room::room::comments::comment_state_json(&state),
                }),
            ),
            Err(error) => refused("read comments", &error),
        }
    }

    /// One page of one thread's replies.
    pub(super) async fn handle_replies(
        &self,
        request: Request<Body>,
        context: &RequestContext,
        slug: &str,
        comment: &str,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let Ok(comment_id) = uuid::Uuid::parse_str(comment) else {
            return write_json(400, &json!({"error": "bad comment id"}));
        };
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self.viewer_as(
            &entry,
            context.identity(),
            &headers,
            arrival,
            query.as_deref(),
        );
        if let Err(response) = self.check_readable(&entry, &who) {
            return response;
        }
        let may_edit = who.at_least(Role::Editor);
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let values: HashMap<String, String> = query
            .as_deref()
            .map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let limit = values
            .get("limit")
            .and_then(|raw| raw.parse::<usize>().ok())
            .unwrap_or(librepaper_room::room::comments::THREAD_PAGE_DEFAULT);
        let after = match values.get("cursor") {
            Some(raw) => match librepaper_room::room::comments::decode_cursor(
                raw,
                room.document_id,
                Some(comment_id),
                may_edit,
            ) {
                Ok(position) => Some(position),
                Err(error) => return write_json(400, &json!({"error": error.to_string()})),
            },
            None => None,
        };
        match room.reply_page(comment_id, after, limit, may_edit).await {
            // A thread of a document this caller cannot see a suggestion of
            // answers exactly as a missing one does.
            Ok(None) => write_json(404, &json!({"error": "not found"})),
            Ok(Some(page)) => write_json(
                200,
                &json!({
                    "version": 1,
                    "protocol": "librepaper.comments.v1",
                    "comment_id": comment,
                    "replies": page.replies,
                    "next_cursor": page.next.map(|at| librepaper_room::room::comments::encode_cursor(
                        room.document_id, Some(comment_id), may_edit, at)),
                    "complete": page.complete,
                    "oversize": page.oversize,
                    "total": page.total,
                }),
            ),
            Err(error) => refused("read replies", &error),
        }
    }

    pub(super) async fn handle_comments(
        &self,
        request: Request<Body>,
        peer: SocketAddr,
        context: &RequestContext,
        slug: &str,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let headers = request.headers().clone();
        let query = request.uri().query().map(str::to_string);
        let who = self.viewer_as(
            &entry,
            context.identity(),
            &headers,
            arrival,
            query.as_deref(),
        );
        if let Err(response) = self.check_readable(&entry, &who) {
            return response;
        }
        let author = self.comment_author(&headers, arrival, &who.id);
        let may_edit = who.at_least(Role::Editor);

        match *request.method() {
            Method::GET => {
                self.comment_page_reply(&room, query.as_deref(), &author, may_edit)
                    .await
            }
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
                if incoming.motivation() == "editing" && !who.at_least(Role::Editor) {
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
                if self.needs_sign_in(&current_entry, &current_who) {
                    return sign_in_to_read();
                }
                if !self.may_read(&current_entry, &current_who)
                    || (who.at_least(Role::Commenter) && !current_who.at_least(Role::Commenter))
                {
                    return write_json(403, &json!({"error": "comment access changed"}));
                }
                // A read link can read the projection but never mutate its
                // annotation channel. Refuse at the HTTP boundary so a role
                // failure is not misreported as a room validation error
                // after work has already begun.
                if !current_who.at_least(Role::Commenter) {
                    return write_json(403, &json!({"error": "commenter access is required"}));
                }
                // Readers and commenters annotate the exact current projection
                // which produced their rendered page.
                if incoming.kind() == "comment"
                    && !current_who.at_least(Role::Editor)
                    && incoming.render_digest().is_empty()
                {
                    return write_json(
                        409,
                        &json!({"error": "current project identity is required; refresh before annotating"}),
                    );
                }
                let address = client_address(peer, &headers, &self.config.cost.trusted_proxies);
                let (result, ok) = self
                    .apply_from(&room, incoming, &address, &current_who, &author)
                    .await;
                if ok {
                    // One preparation for all three audiences: the two
                    // broadcasts and this caller's own view. Each used to
                    // read the comment and the collection counts again for
                    // itself.
                    let event = room.prepare_comment_event(&result).await;
                    room.broadcast_prepared(None, &event).await;
                    return write_json(200, &event.view_for(&author, may_edit));
                }
                // The status is the refusal table's, put there by
                // `command_refusal_value`. Only the shapes that carry no
                // typed error -- a malformed message, an unknown comment id
                // -- still fall back to 400. The allowed set is what
                // `WriteError::status` can produce plus the statuses this
                // route decides for itself; anything else would mean a
                // status arrived from somewhere it should not have.
                let status = result
                    .get("status")
                    .and_then(Value::as_u64)
                    .filter(|status| {
                        matches!(
                            status,
                            400 | 403 | 404 | 409 | 410 | 413 | 426 | 429 | 503 | 507
                        )
                    })
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
        // One whole project publish is one upload, regardless of how many
        // files it contains. Source edits made later over sockets or agent
        // tools are storage changes, not upload requests. Reserve before
        // reading the body so the hourly limit also bounds upload traffic.
        let owner_id = match uuid::Uuid::parse_str(&who.id) {
            Ok(id) => id,
            Err(_) => return write_json(401, &json!({"error": "invalid account"})),
        };
        let upload_admission = match self.store.catalog.reserve_upload_admission(owner_id).await {
            Ok(id) => id,
            Err(librepaper_engine::storage::postgres::Error::Conflict(_)) => {
                return write_json(
                    429,
                    &json!({"error": "too many uploads this hour; try later"}),
                )
            }
            Err(error) => return write_json(503, &json!({"error": error.to_string()})),
        };
        let parsed = match self.read_upload(request).await {
            Ok(parsed) => parsed,
            Err(response) => {
                let _ = self
                    .store
                    .catalog
                    .cancel_upload_admission(upload_admission)
                    .await;
                return response;
            }
        };

        let mut base = slugify(&parsed.slug, &self.config);
        if base.is_empty() {
            base = slugify(&parsed.title, &self.config);
        }
        if base.is_empty() {
            let _ = self
                .store
                .catalog
                .cancel_upload_admission(upload_admission)
                .await;
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
                let _ = self
                    .store
                    .catalog
                    .cancel_upload_admission(upload_admission)
                    .await;
                return write_json(
                    503,
                    &json!({
                        "error": error.to_string(),
                        "retryable": true,
                    }),
                );
            }
        };
        let mine = existing.as_ref().is_some_and(|e| e.owned_by(&who.id));
        let key = if mine {
            base.clone()
        } else {
            format!("{base}-{}", random_suffix(&self.config))
        };
        let actor = librepaper_engine::storage::store::MutationActor {
            account_id: who.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: true,
            policy_comment: self.commenters.allows(&who.handle),
            automation: false,
            unowned_publisher: false,
        };

        // What the main file is called, and the rest of the directory to
        // publish alongside it. A directory upload names both; a one-file
        // upload over a document that already has one keeps its existing
        // path (unless the format changed) and carries the rest of the
        // directory forward unchanged, because that upload never claimed to
        // be the whole document -- only `put_directory_as_actor`'s
        // whole-project replacement (§7) does, and it always takes the
        // complete file set.
        let (main, mut files) = if !parsed.main.is_empty() {
            (parsed.main.clone(), parsed.files.clone())
        } else if let Some(existing) = existing.as_ref().filter(|_| mine) {
            let main = if !parsed.source_format.is_empty()
                && librepaper_room::room::format_from_path(&existing.main) != parsed.source_format
            {
                librepaper_room::room::main_path_for("", &parsed.source_format)
            } else {
                existing.main.clone()
            };
            (main, Vec::new())
        } else {
            (
                librepaper_room::room::main_path_for("", &parsed.source_format),
                Vec::new(),
            )
        };
        if parsed.main.is_empty() && mine {
            match self.store.project_files(&key).await {
                Ok(Some(carried)) => {
                    for (path, bytes) in carried {
                        if path != main {
                            files.push((path, bytes));
                        }
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!("warning: could not read {key} before republishing it: {error}");
                }
            }
        }
        let parsed = Upload { files, ..parsed };

        // Every file is checked against the size and encoding rules before
        // anything is written: a document is a directory, and it is either
        // published whole or refused whole, never left half-written because
        // the eleventh file was the one that broke a rule the first ten
        // happened to keep.
        if let Err(response) = self.preflight_directory(&parsed) {
            let _ = self
                .store
                .catalog
                .cancel_upload_admission(upload_admission)
                .await;
            return response;
        }
        let entry = match self
            .store
            .put_directory_as_actor(
                DocumentInput {
                    slug: key.clone(),
                    title: parsed.title.clone(),
                    source: parsed.source.clone(),
                    source_format: parsed.source_format.clone(),
                    main: main.clone(),
                },
                parsed.files.clone(),
                actor.clone(),
            )
            .await
        {
            Ok(entry) => entry,
            Err(PutError::Quota { status, message }) => {
                // Staging may already have written immutable objects before
                // quota admission failed. Keep this request's hourly slot.
                return write_json(status, &json!({"error": message}));
            }
            Err(PutError::Authorization { status, message }) => {
                // Authorization can fail after staging too.
                return write_json(status, &json!({"error": message}));
            }
            Err(PutError::Storage(_)) => {
                return write_json(500, &json!({"error": "could not store the document"}))
            }
        };
        // A title given on a republish renames the document; an empty one
        // leaves it as it is. A brand-new document already has the title it
        // was created with.
        if mine && !parsed.title.is_empty() && parsed.title != entry.title {
            if let Err(error) = self.store.rename(&key, &parsed.title, &actor).await {
                return write_json(409, &json!({"error": error}));
            }
        }
        let entry = if mine {
            match self.store.get_result(&key).await {
                Ok(Some(entry)) => entry,
                Ok(None) => entry,
                Err(error) => return refused("read the catalogue", &error.into()),
            }
        } else {
            entry
        };
        // Share-link credentials are stored only as digests, so a revision
        // cannot reproduce an existing URL, and republishing must not
        // silently rotate the link merely to manufacture one -- only a
        // brand-new document is offered one here.
        let share_url = if mine {
            Value::Null
        } else {
            match self.mint_read_link(&key, &who).await {
                Ok(url) => Value::String(url),
                Err(error) => {
                    eprintln!("warning: could not mint the read link of {key}: {error:?}");
                    Value::Null
                }
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

    /// Parses a publish request's body, in either format it may arrive as, and
    /// applies the checks common to both: a title and some HTML are present.
    /// It answers the request itself on any problem, so `handle_upload` only
    /// has to decide where to store what comes back.
    #[allow(clippy::result_large_err)] // as publisher: the error is a response
    pub(super) async fn read_upload(&self, request: Request<Body>) -> Result<Upload, Reply> {
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
            // The middleware declares the content length, and the body is
            // bounded by that declared length rather than a ceiling computed
            // from document and figure limits.
            let Some(ceiling) = header_of(request.headers(), "content-length")
                .and_then(|v| v.parse::<usize>().ok())
            else {
                return Err(write_json(
                    411,
                    &json!({"error": "a multipart upload must declare its content length"}),
                ));
            };
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
                    Err(err) => return Err(upload_limit_exceeded(&err)),
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
                            Err(err) => return Err(upload_limit_exceeded(&err)),
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
                html = main_source(at, bytes)?;
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
                html = main_source(&filename, &bytes)?;
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
            if let Some(format) = librepaper_document::document::render::document_format(&filename)
            {
                if title.trim().is_empty() {
                    title = match format {
                        "typst" => librepaper_document::document::render::title_from_typst(&html),
                        "markdown" => title_from_markdown(&html),
                        "quarto" => librepaper_document::document::render::title_from_quarto(&html),
                        // There is no TeX here to ask, so the `\title` is
                        // scanned for; see `render::title_from_latex`.
                        "latex" => librepaper_document::document::render::title_from_latex(&html),
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
            // The middleware declares the content length.
            let Some(declared_length) = header_of(request.headers(), "content-length")
                .and_then(|v| v.parse::<usize>().ok())
            else {
                return Err(write_json(
                    411,
                    &json!({"error": "a JSON upload must declare its content length"}),
                ));
            };
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
            let Ok(bytes) = to_bytes(request.into_body(), declared_length).await else {
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
        let expected_engine =
            librepaper_document::results::document_metadata(&source_format).execution_engine;
        let expected_draft =
            librepaper_document::results::document_metadata(&source_format).draft_format;
        if let Some(raw_engine) = requested_engine {
            let supplied = librepaper_document::results::ExecutionEngine::parse(&raw_engine)
                .map_err(|error| write_json(400, &json!({"error": error})))?;
            if supplied != expected_engine {
                return Err(write_json(
                    400,
                    &json!({"error": "execution engine does not match source_format"}),
                ));
            }
        }
        if let Some(raw_draft) = requested_draft_format {
            let supplied = librepaper_document::results::DraftFormat::parse(&raw_draft)
                .map_err(|error| write_json(400, &json!({"error": error})))?;
            if supplied != expected_draft {
                return Err(write_json(
                    400,
                    &json!({"error": "draft format does not match source_format"}),
                ));
            }
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
            match librepaper_document::document::paths::check(
                &librepaper_document::document::paths::rules(&self.config),
                &main,
            ) {
                Ok(librepaper_document::document::paths::Kind::Text) => {}
                Ok(librepaper_document::document::paths::Kind::Asset) => {
                    return Err(write_json(
                        400,
                        &json!({"error": format!("{main} cannot be the main file; it is a figure")}),
                    ))
                }
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
            seen.insert(librepaper_document::document::paths::collision_key(&main));
        }
        for (path, bytes) in sent {
            let kind = match librepaper_document::document::paths::check(
                &librepaper_document::document::paths::rules(&self.config),
                &path,
            ) {
                Ok(kind) => kind,
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            };
            if !seen.insert(librepaper_document::document::paths::collision_key(&path)) {
                return Err(write_json(
                    400,
                    &json!({"error": format!("{path}: two files cannot share one name")}),
                ));
            }
            kept.push((
                librepaper_document::document::paths::normalise(&path),
                kind,
                bytes,
            ));
        }
        if seen.len() > self.config.max_files {
            return Err(write_json(
                413,
                &json!({"error": format!("a document may hold {} files", self.config.max_files)}),
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

    /// Checks a whole directory upload against the encoding and path rules
    /// before any of it touches storage or the session. A document is a
    /// directory, so it is admitted whole or refused whole, naming the file
    /// and the rule it broke -- never accepted with the eleventh file silently
    /// missing because it was the one that turned out to be invalid.
    ///
    /// `main_path` is where the upload's main file will live.
    #[allow(clippy::result_large_err)] // as read_upload: the error is a response
    pub(super) fn preflight_directory(&self, parsed: &Upload) -> Result<(), Reply> {
        for (path, bytes) in &parsed.files {
            match librepaper_document::document::paths::check(
                &librepaper_document::document::paths::rules(&self.config),
                path,
            ) {
                Ok(librepaper_document::document::paths::Kind::Text) => {
                    if std::str::from_utf8(bytes).is_err() {
                        return Err(write_json(
                            400,
                            &json!({"error": format!("{path} is not valid UTF-8")}),
                        ));
                    }
                }
                Ok(librepaper_document::document::paths::Kind::Asset) => {}
                Err(why) => return Err(write_json(400, &json!({"error": why}))),
            }
        }
        Ok(())
    }

    pub(super) async fn handle_state(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return plain(400, "bad slug");
        }
        let (entry, who) = match self.entry_viewer_in(slug, context, headers, query).await {
            Ok(result) => result,
            Err(response) => return response,
        };
        // The opaque reference identifies immutable bytes; it does not say
        // who is holding it. Who may read is asked again, from the request itself,
        // which is the same rule the socket answers `doc-open` under.
        // Full document state is a source synchronization transport. Readers
        // and commenters get the projection through `handle_project` instead
        // (§2.2): there is no separate rendered bundle any more.
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
        let transfer_id = fields.get("transfer").cloned().unwrap_or_default();
        // Cross-site fetches are refused here as everywhere else: a document's
        // source is not another site's to read out of a signed-in browser.
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let (bytes, digest) = {
            let mut transfers = self.state_transfers.lock().await;
            // Only what the response needs leaves the lock: the entry itself
            // holds a memory reservation and cannot be cloned.
            let Some((transfer_slug, bytes, digest, expires_at)) =
                transfers.entries.get(&transfer_id).map(|t| {
                    (
                        t.slug.clone(),
                        t.bytes.clone(),
                        t.digest.clone(),
                        t.expires_at,
                    )
                })
            else {
                return plain(410, "that baseline has expired; reconnect for a newer one");
            };
            if transfer_slug != slug || expires_at < librepaper_base::util::now_unix() {
                transfers.remove(&transfer_id);
                transfers.order.retain(|id| id != &transfer_id);
                return plain(410, "that baseline has expired; reconnect for a newer one");
            }
            (bytes, digest)
        };
        let mut response = Response::new(Body::from(bytes));
        set(&mut response, "content-type", "application/octet-stream");
        set(&mut response, "cache-control", "no-store");
        set(&mut response, "x-librepaper-state-digest", &digest);
        privacy_headers(&mut response);
        response
    }

    /// The source of a document, which is the document. Readable by anyone
    /// who may read it.
    pub(super) async fn handle_source(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer_as(&entry, context.identity(), headers, arrival, query);
        if self.needs_sign_in(&entry, &who) {
            return sign_in_to_read();
        }
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // Source is editable project material. Readers and commenters get
        // the same head through `handle_project`'s projection (§2.2); this
        // endpoint is for the editor's own working copy.
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let source = match room.source().await {
            Ok(source) => source,
            Err(error) => return sequencer_reply(&error),
        };
        let format = match room.format(&entry.source_format).await {
            Ok(format) => format,
            Err(error) => return sequencer_reply(&error),
        };
        write_json(
            200,
            &json!({
                "slug": entry.slug, "title": entry.title, "sha": entry.sha,
                "format": format, "source": source,
            }),
        )
    }

    /// The current renderer inputs for every authorized role. This projection
    /// is deliberately constructed from an allowlist instead of serializing
    /// the collaboration document and removing privileged fields afterwards.
    pub(super) async fn handle_project(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer_as(&entry, context.identity(), headers, arrival, query);
        if self.needs_sign_in(&entry, &who) {
            return sign_in_to_read();
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let projected = match room.projection().await {
            Ok(projected) => projected,
            Err(error) => return sequencer_reply(&error),
        };
        let format = match room.format(&entry.source_format).await {
            Ok(format) => format,
            Err(error) => return sequencer_reply(&error),
        };
        // The etag -- and what `source-changed {digest}` on the socket
        // names (§4.5) -- is the projection digest, not a row number: two
        // heads with the same files answer the same identity (§2.2).
        let identity = projected.projection.digest();
        let etag = format!("\"{identity}\"");
        if header_of(headers, "if-none-match").is_some_and(|value| {
            value
                .split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        }) {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            set(&mut response, "etag", &etag);
            set(&mut response, "cache-control", "private, no-cache");
            privacy_headers(&mut response);
            return response;
        }
        let mut response = write_json(
            200,
            &json!({
                "schema": 1,
                "project_digest": identity,
                "slug": entry.slug,
                "title": entry.title,
                "format": format,
                "main": projected.projection.main,
                "tree": projected.projection,
                "texts": projected.texts,
            }),
        );
        set(&mut response, "etag", &etag);
        set(&mut response, "cache-control", "private, no-cache");
        privacy_headers(&mut response);
        response
    }

    pub(super) async fn handle_snapshot(
        &self,
        headers: &HeaderMap,
        context: &RequestContext,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        let arrival = &context.arrival;
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer_as(&entry, context.identity(), headers, arrival, query);
        if self.needs_sign_in(&entry, &who) {
            return sign_in_to_read();
        }
        if !who.at_least(Role::Editor) || !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let author = self.comment_author(headers, arrival, &who.id);
        let may_edit = who.at_least(Role::Editor);
        let room = match self.rooms.get(slug).await {
            Ok(room) => room,
            Err(error) => return refused("open the room", &error),
        };
        let projected = match room.projection().await {
            Ok(projected) => projected,
            Err(error) => return sequencer_reply(&error),
        };
        let format = match room.format(&entry.source_format).await {
            Ok(format) => format,
            Err(error) => return sequencer_reply(&error),
        };
        let source = projected
            .texts
            .get(&projected.projection.main)
            .cloned()
            .unwrap_or_default();
        // The first page, not the collection. `librepaper.snapshot.v1`
        // always carried a `comments` array and still does; what changed is
        // that it is a bounded page with `comment_state` beside it saying
        // how much more there is and where to continue. A caller that
        // wants the rest walks `GET .../comments?cursor=`.
        let state = match room.comment_state(may_edit).await {
            Ok(state) => state,
            Err(error) => return refused("read snapshot comment state", &error),
        };
        let (comments, page) = match room
            .comment_page(
                None,
                librepaper_room::room::comments::COMMENT_PAGE_DEFAULT,
                &author,
                may_edit,
            )
            .await
        {
            Ok(both) => both,
            Err(error) => return refused("read snapshot comments", &error),
        };
        let mut comment_state = librepaper_room::room::comments::comment_state_json(&state);
        comment_state["complete"] = json!(page.complete);
        comment_state["next_cursor"] = json!(page
            .next
            .map(|at| librepaper_room::room::comments::encode_cursor(room.document_id, None, may_edit, at)));
        let tree_sha = projected.projection.digest();
        let main = projected.projection.main.clone();
        let files = projected.projection.files.clone();
        let snapshot = librepaper_document::document::projection::ProjectSnapshot {
            sha: tree_sha,
            tree: projected.projection.clone(),
            texts: projected.texts.clone(),
        };
        let response = SnapshotResponse {
            version: 1,
            protocol: "librepaper.snapshot.v1",
            slug: entry.slug,
            title: entry.title,
            format,
            main,
            project: snapshot,
            files,
            source_sha: librepaper_engine::storage::store::digest_of(&source),
            source,
            comments,
            comment_state,
            role: who.role.as_str().to_string(),
            capabilities: json!({
                "read": true,
                "comment": who.at_least(Role::Commenter),
                "edit": who.at_least(Role::Editor),
            }),
        };
        let payload = match serde_json::to_value(response) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("could not serialize project snapshot for {slug}: {error}");
                return write_json(500, &json!({"error": "could not read project snapshot"}));
            }
        };
        write_json(200, &payload)
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
            Ok(Some(entry)) if entry.owned_by(&who.id) => entry,
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

    /// Give a project a different name.
    ///
    /// The name and the address are two things: a project keeps its slug when
    /// it is renamed, because the slug is what every link anybody has already
    /// shared points at, and a rename is about what this project is called
    /// rather than about where it lives. Only the owner may, for the same
    /// reason a deletion is theirs alone.
    pub(super) async fn handle_rename(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let who = match self.publisher(&headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        let title = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|value| value.get("title")?.as_str().map(str::to_string))
            .unwrap_or_default();
        let title = title.trim().to_string();
        if title.is_empty() {
            return write_json(400, &json!({"error": "give the project a name"}));
        }
        if title.chars().count() > 200 {
            return write_json(400, &json!({"error": "that name is too long"}));
        }
        // Somebody else's document answers as a missing one does.
        match self.checked_entry(slug).await {
            Ok(Some(entry)) if entry.owned_by(&who.id) => entry,
            Ok(Some(_)) | Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let actor = librepaper_engine::storage::store::MutationActor {
            account_id: who.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: true,
            policy_comment: self.commenters.allows(&who.handle),
            automation: false,
            unowned_publisher: false,
        };
        match self.store.rename(slug, &title, &actor).await {
            Ok(()) => write_json(200, &json!({"slug": slug, "title": title})),
            Err(error) if error == "ownership changed" => write_json(409, &json!({"error": error})),
            Err(error) => {
                eprintln!("could not rename {slug}: {error}");
                write_json(500, &json!({"error": "could not rename the project"}))
            }
        }
    }

    /// Copy a project, with everything in it, into a new project of your own.
    ///
    /// A fork is a new document and not a branch: it has its own slug, its own
    /// links, its own comments -- which is to say none -- and its own history
    /// starting now. Nothing about it points back, because the thing being
    /// copied may be somebody else's and may go away.
    ///
    /// Anyone who may read a project may fork it, which is the same rule that
    /// lets them download it: a copy taken through the browser and a copy
    /// taken here are the same copy, and refusing the second while allowing
    /// the first would only be slower.
    pub(super) async fn handle_fork(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        self.copy_project(request, arrival, slug, false).await
    }

    /// Copy a project into a custom template: a fork that must be named and
    /// is then marked, so it leaves the project listing and is offered when
    /// starting a new project. Every check is the fork's.
    pub(super) async fn handle_template(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
    ) -> Reply {
        self.copy_project(request, arrival, slug, true).await
    }

    async fn copy_project(
        &self,
        request: Request<Body>,
        arrival: &Arrival,
        slug: &str,
        as_template: bool,
    ) -> Reply {
        let headers = request.headers().clone();
        if cross_site_refused(&headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let query = request.uri().query().map(str::to_string);
        // The name the copy is to have. The client asks for one before it
        // calls, because a project named for what it is beats a shelf of
        // "(copy)" rows; an empty or absent body still forks, so the old
        // bodiless call keeps working.
        let Ok(body) = to_bytes(request.into_body(), 1 << 16).await else {
            return write_json(400, &json!({"error": "bad request"}));
        };
        let asked_body = serde_json::from_slice::<Value>(&body).ok();
        let asked = asked_body
            .as_ref()
            .and_then(|value| value.get("title")?.as_str().map(str::to_string))
            .unwrap_or_default();
        let asked = asked.trim().to_string();
        if asked.chars().count() > 200 {
            return write_json(400, &json!({"error": "that name is too long"}));
        }
        let template_request_id = if as_template {
            let request_id = asked_body
                .as_ref()
                .and_then(|value| value.get("request_id")?.as_str().map(str::to_string))
                .and_then(|value| uuid::Uuid::parse_str(&value).ok());
            let Some(request_id) = request_id else {
                return write_json(
                    400,
                    &json!({"error": "a template save needs a stable request_id"}),
                );
            };
            Some(request_id)
        } else {
            None
        };
        if as_template && asked.is_empty() {
            return write_json(400, &json!({"error": "give the template a name"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        // Read it as whoever is asking: a private project nobody shared is
        // not forkable, and answers as missing rather than as refused.
        let viewer = self
            .viewer(&entry, &headers, arrival, query.as_deref())
            .await;
        if self.needs_sign_in(&entry, &viewer) {
            return sign_in_to_read();
        }
        if !self.may_read(&entry, &viewer) {
            return write_json(404, &json!({"error": "not found"}));
        }
        // But it is written as an account: a copy has to belong to somebody,
        // and a link-holder who is not signed in has nowhere to put one.
        let who = match self.publisher(&headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        if who.id.is_empty() {
            return write_json(401, &json!({"error": "sign in to keep a copy"}));
        }
        let account_id = match uuid::Uuid::parse_str(&who.id) {
            Ok(id) => id,
            Err(_) => return write_json(401, &json!({"error": "invalid account"})),
        };
        let template_source_id = if as_template {
            match uuid::Uuid::parse_str(&entry.storage_id) {
                Ok(id) => Some(id),
                Err(_) => return write_json(404, &json!({"error": "not found"})),
            }
        } else {
            None
        };
        let session_generation = match who.session_generation.parse::<i64>() {
            Ok(generation) => generation,
            Err(_) => return write_json(401, &json!({"error": "invalid account session"})),
        };
        let mut template_operation = if let Some(request_id) = template_request_id {
            match self
                .store
                .catalog
                .template_operation(account_id, session_generation, request_id)
                .await
            {
                Ok(operation) => operation,
                Err(librepaper_engine::storage::postgres::Error::Conflict(error)) => {
                    return write_json(409, &json!({"error": error}))
                }
                Err(error) => return write_json(503, &json!({"error": error.to_string()})),
            }
        } else {
            None
        };
        if let Some(operation) = &template_operation {
            if Some(operation.source_document_id) != template_source_id || operation.title != asked
            {
                return write_json(
                    409,
                    &json!({"error": "template request_id was reused with different input"}),
                );
            }
            if operation.target.status != "active" {
                return write_json(
                    409,
                    &json!({"error": "the saved template is no longer active"}),
                );
            }
            if operation.complete {
                return write_json(
                    200,
                    &json!({
                        "slug": operation.target.slug,
                        "title": operation.target.title,
                        "url": format!("/docs/{}", operation.target.slug),
                    }),
                );
            }
        }
        // A completed or pending replay already owns its original admission.
        // Only a genuinely new operation consumes another hourly slot.
        let upload_admission = if !as_template || template_operation.is_none() {
            match self
                .store
                .catalog
                .reserve_upload_admission(account_id)
                .await
            {
                Ok(id) => Some(id),
                Err(librepaper_engine::storage::postgres::Error::Conflict(_)) => {
                    return write_json(
                        429,
                        &json!({"error": "too many uploads this hour; try later"}),
                    )
                }
                Err(error) => return write_json(503, &json!({"error": error.to_string()})),
            }
        } else {
            None
        };
        let files = match self.store.project_files(slug).await {
            Ok(Some(files)) => files,
            Ok(None) => {
                if let Some(admission) = upload_admission {
                    let _ = self.store.catalog.cancel_upload_admission(admission).await;
                }
                return write_json(404, &json!({"error": "not found"}));
            }
            Err(error) => {
                if let Some(admission) = upload_admission {
                    let _ = self.store.catalog.cancel_upload_admission(admission).await;
                }
                eprintln!("could not read {slug} to fork it: {error}");
                return write_json(503, &json!({"error": "could not read that project"}));
            }
        };
        if files.len() > self.config.max_files {
            if let Some(admission) = upload_admission {
                let _ = self.store.catalog.cancel_upload_admission(admission).await;
            }
            return write_json(
                413,
                &json!({"error": format!("a document may hold {} files", self.config.max_files)}),
            );
        }
        let title = if asked.is_empty() {
            format!("{} (copy)", entry.title)
        } else {
            asked
        };
        // Always a fresh suffixed slug, never the bare title: two copies of
        // the same paper are the ordinary case here, and the second must not
        // land on the first.
        let mut base = {
            let stem = slugify(&title, &self.config);
            let stem = if stem.is_empty() {
                "project".into()
            } else {
                stem
            };
            format!(
                "{stem}-{}",
                librepaper_engine::storage::store::random_suffix(&self.config)
            )
        };
        if let Some(operation) = &template_operation {
            base = operation.target.slug.clone();
        }
        let source = files
            .iter()
            .find(|(path, _)| path == &entry.main)
            .and_then(|(_, bytes)| String::from_utf8(bytes.clone()).ok())
            .unwrap_or_default();
        let rest: Vec<(String, Vec<u8>)> = files
            .into_iter()
            .filter(|(path, _)| path != &entry.main)
            .collect();
        if let Err(response) = self.preflight_directory(&Upload {
            title: title.clone(),
            slug: String::new(),
            source: source.clone(),
            source_format: entry.source_format.clone(),
            main: entry.main.clone(),
            files: rest.clone(),
        }) {
            if let Some(admission) = upload_admission {
                let _ = self.store.catalog.cancel_upload_admission(admission).await;
            }
            return response;
        }
        if as_template && template_operation.is_none() {
            let request_id = template_request_id.expect("template request id was checked above");
            let source_id = template_source_id.expect("template source id was checked above");
            let (operation, created) = match self
                .store
                .catalog
                .begin_template_operation(
                    request_id,
                    source_id,
                    librepaper_engine::storage::postgres::NewDocument {
                        slug: base.clone(),
                        owner_id: account_id,
                        owner_session_generation: Some(session_generation),
                        ownership_mode: "owned".into(),
                        title: title.clone(),
                        source_format: entry.source_format.clone(),
                        main_path: entry.main.clone(),
                    },
                )
                .await
            {
                Ok(result) => result,
                Err(librepaper_engine::storage::postgres::Error::Conflict(error)) => {
                    if let Some(admission) = upload_admission {
                        let _ = self.store.catalog.cancel_upload_admission(admission).await;
                    }
                    return write_json(409, &json!({"error": error}));
                }
                Err(error) => {
                    if let Some(admission) = upload_admission {
                        let _ = self.store.catalog.cancel_upload_admission(admission).await;
                    }
                    return write_json(503, &json!({"error": error.to_string()}));
                }
            };
            if !created {
                // Another same-id request won the row-creation race; release
                // this request's unused reservation, never the winner's.
                if let Some(admission) = upload_admission {
                    let _ = self.store.catalog.cancel_upload_admission(admission).await;
                }
            }
            base = operation.target.slug.clone();
            template_operation = Some(operation);
        }
        if let Some(operation) = &template_operation {
            if operation.complete {
                return write_json(
                    200,
                    &json!({
                        "slug": operation.target.slug,
                        "title": operation.target.title,
                        "url": format!("/docs/{}", operation.target.slug),
                    }),
                );
            }
        }
        let actor = librepaper_engine::storage::store::MutationActor {
            account_id: who.id.clone(),
            owner_key: who.key.clone(),
            session_generation: who.session_generation.clone(),
            link_hash: String::new(),
            policy_editor: true,
            policy_comment: self.commenters.allows(&who.handle),
            automation: false,
            unowned_publisher: false,
        };
        let input = DocumentInput {
            slug: base,
            title: title.clone(),
            source,
            source_format: entry.source_format.clone(),
            main: entry.main.clone(),
        };
        let saved = if let Some(operation) = &template_operation {
            self.store
                .put_template_directory_as_actor(
                    input,
                    rest,
                    actor,
                    template_request_id.expect("template request id was checked above"),
                    operation.target.id,
                )
                .await
        } else {
            self.store.put_directory_as_actor(input, rest, actor).await
        };
        match saved {
            Ok(made) => write_json(
                200,
                &json!({"slug": made.slug, "title": made.title, "url": format!("/docs/{}", made.slug)}),
            ),
            Err(PutError::Quota { status, message }) => {
                // A failed create can leave its new, empty document row; keep
                // the upload counted to bound those side effects.
                write_json(status, &json!({"error": message}))
            }
            Err(PutError::Authorization { status, message }) => {
                write_json(status, &json!({"error": message}))
            }
            Err(PutError::Storage(error)) => {
                // The command may have committed before this ambiguous
                // storage error reached the request, so keep the admission.
                eprintln!("could not fork {slug}: {error:?}");
                write_json(500, &json!({"error": "could not copy that project"}))
            }
        }
    }

    /// The custom templates this account owns, as ordinary project rows.
    pub(super) async fn handle_templates(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        query: Option<&str>,
    ) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let who = match self.publisher(headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        if who.id.is_empty() {
            return write_json(401, &json!({"error": "sign in"}));
        }
        let values: HashMap<String, String> = query
            .map(|raw| {
                url::form_urlencoded::parse(raw.as_bytes())
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let limit = values
            .get("limit")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(200)
            .clamp(1, 200);
        let cursor = values
            .get("after_updated")
            .zip(values.get("after_id"))
            .map(|(updated, id)| (updated.as_str(), id.as_str()));
        match self.store.templates_page(&who.id, cursor, limit).await {
            Ok((entries, next)) => {
                let templates: Vec<Value> = entries
                    .iter()
                    .map(|entry| self.listing_row(entry, &who))
                    .collect();
                let mut body = json!({"templates": templates});
                if let Some((after_updated, after_id)) = next {
                    body["next_cursor"] =
                        json!({"after_updated": after_updated, "after_id": after_id});
                }
                write_json(200, &body)
            }
            Err(error) => {
                eprintln!("could not query templates: {error}");
                write_json(503, &json!({"error": "catalogue temporarily unavailable"}))
            }
        }
    }

    /// What this account has deleted and can still get back.
    ///
    /// Deleting has always been a mark and a job queued seven days out rather
    /// than an erasure, so this reads a recovery window that was already being
    /// kept and was simply never shown to anybody.
    pub(super) async fn handle_trash(&self, headers: &HeaderMap, arrival: &Arrival) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        let who = match self.publisher(headers, arrival).await {
            Ok(who) => who,
            Err(response) => return response,
        };
        if who.id.is_empty() {
            return write_json(401, &json!({"error": "sign in to see your trash"}));
        }
        match self.store.trashed_page(&who.id, 200).await {
            Ok(rows) => {
                let ids: Vec<uuid::Uuid> = rows
                    .iter()
                    .filter_map(|(entry, _)| uuid::Uuid::parse_str(&entry.storage_id).ok())
                    .collect();
                let templates = match self.store.catalog.template_ids(&ids).await {
                    Ok(templates) => templates,
                    Err(error) => {
                        eprintln!("could not query the trash: {error}");
                        return write_json(
                            503,
                            &json!({"error": "catalogue temporarily unavailable"}),
                        );
                    }
                };
                let documents: Vec<Value> = rows
                    .iter()
                    .map(|(entry, due)| {
                        let mut row = self.listing_row(entry, &who);
                        // Both halves of the window, so the listing can say
                        // "6 days left" without knowing what the grace is.
                        row["deleted_at"] = json!(entry.updated_at);
                        row["purge_due"] = json!(due);
                        row["template"] = json!(uuid::Uuid::parse_str(&entry.storage_id)
                            .is_ok_and(|id| templates.contains(&id)));
                        row
                    })
                    .collect();
                write_json(200, &json!({"documents": documents}))
            }
            Err(error) => {
                eprintln!("could not query the trash: {error}");
                write_json(503, &json!({"error": "catalogue temporarily unavailable"}))
            }
        }
    }

    /// Take a deleted document back out of the trash, or destroy it now.
    /// `purge` chooses which: both are the owner acting on the same queued
    /// job, one cancelling it and one bringing it forward.
    pub(super) async fn handle_untrash(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        purge: bool,
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
        if who.id.is_empty() {
            return write_json(401, &json!({"error": "sign in first"}));
        }
        let done = if purge {
            self.store.purge_now(slug, &who.id).await.map(|document| {
                if let Some(document) = document {
                    self.background
                        .ask(librepaper_engine::storage::worker::Task::Delete(document));
                    true
                } else {
                    false
                }
            })
        } else {
            self.store.restore(slug, &who.id).await
        };
        match done {
            // Somebody else's document, and a document that was never here,
            // answer alike: the trash is not a way to probe for slugs.
            Ok(true) => write_json(200, &json!({"slug": slug, "purged": purge})),
            // The window closed between the listing and the button. The purge
            // is already running and its files are already going, so there is
            // nothing left to put back.
            Ok(false) => write_json(
                409,
                &json!({"error": "this project is already being deleted and cannot be recovered"}),
            ),
            Err(ModifyError::NotFound) | Err(ModifyError::Refused(_)) => {
                write_json(404, &json!({"error": "not found"}))
            }
            Err(error) => {
                eprintln!("could not restore {slug}: {error:?}");
                write_json(500, &json!({"error": "could not restore the project"}))
            }
        }
    }

    /// Star a document, or take the star off it. A favourite is this account's
    /// note about a document it can already see, so the gate is the same one
    /// reading it is: anyone who may open it may star it, which is what makes
    /// a project shared with you starrable without it becoming yours.
    pub(super) async fn handle_favorite(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
        on: bool,
    ) -> Reply {
        self.handle_mark(headers, arrival, slug, query, Some(on))
            .await
    }

    /// Hide a shared document from this account's shared-project view, or
    /// restore it. The preference never changes the document's permissions.
    pub(super) async fn handle_shared_hidden(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
        hidden: bool,
    ) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if self.needs_sign_in(&entry, &who) {
            return sign_in_to_read();
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if !who.id.is_signed_in() {
            return write_json(401, &json!({"error": "sign in first"}));
        }
        if entry.owned_by(&who.id.id) {
            return write_json(403, &json!({"error": "only shared projects can be hidden"}));
        }
        match self.store.set_shared_hidden(slug, &who.id.id, hidden).await {
            Ok(shared_hidden) => {
                write_json(200, &json!({"slug": slug, "shared_hidden": shared_hidden}))
            }
            Err(error) => {
                eprintln!("could not update shared visibility for {slug}: {error:?}");
                write_json(500, &json!({"error": "could not save that"}))
            }
        }
    }

    /// Note that this account has just opened a document. Sent by the reader
    /// on open, and the only thing behind the Recent destination.
    pub(super) async fn handle_opened(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
    ) -> Reply {
        self.handle_mark(headers, arrival, slug, query, None).await
    }

    async fn handle_mark(
        &self,
        headers: &HeaderMap,
        arrival: &Arrival,
        slug: &str,
        query: Option<&str>,
        favorite: Option<bool>,
    ) -> Reply {
        if cross_site_refused(headers, arrival) {
            return write_json(403, &cross_site_refusal());
        }
        if !self.valid_slug(slug) {
            return write_json(400, &json!({"error": "bad slug"}));
        }
        let entry = match self.checked_entry(slug).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return write_json(404, &json!({"error": "not found"})),
            Err(response) => return response,
        };
        let who = self.viewer(&entry, headers, arrival, query).await;
        if self.needs_sign_in(&entry, &who) {
            return sign_in_to_read();
        }
        if !self.may_read(&entry, &who) {
            return write_json(404, &json!({"error": "not found"}));
        }
        if !who.id.is_signed_in() {
            return write_json(401, &json!({"error": "sign in first"}));
        }
        let account = who.id.id.clone();
        let done = match favorite {
            Some(on) => self.store.set_favorite(slug, &account, on).await.map(Some),
            None => self.store.mark_opened(slug, &account).await.map(|()| None),
        };
        match done {
            Ok(starred) => write_json(200, &json!({"slug": slug, "favorite": starred})),
            Err(error) => {
                eprintln!("could not mark {slug}: {error:?}");
                write_json(500, &json!({"error": "could not save that"}))
            }
        }
    }
}
