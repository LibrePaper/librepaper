//! `librepaper sync`: the file on disk as a peer in the session.
//!
//! A document published from markdown or typst can be edited in the page it is
//! read in, from as many tabs and machines as its editors like at once. That
//! editor is the only door into a live session, and an author who writes in
//! vim and renders from a Makefile has had to choose between their own tools
//! and it. This command removes the choice: edits in a local editor flow into
//! the session while a browser tab types in it, edits in the browser land in
//! the file, and writing the file marks a checkpoint in the timeline.
//!
//! It is a peer, not an importer. The server holds the document -- a
//! `yrs::Doc` in the room, persisted, answering `y-open` whether or not
//! anyone is attached -- so this needs no route of its own and speaks exactly
//! the protocol `web/src/lib/collab.js` speaks. It is also the first program
//! that is not a browser to read and write the shared document, which is why
//! `crates/librepaper/src/tests/sync.rs` checks a Yrs client and the server against each
//! other rather than trusting the shared encoding.
//!
//! The hard part is not the socket. A text editor is a snapshot client: it
//! read the file at some moment, holds a buffer, and writes the buffer back
//! whole when the author saves. If the session moved meanwhile -- the same
//! author in a browser tab, a restore from the reader -- the buffer does not
//! know, and a diff of the document against the file would delete the words
//! that arrived. `The merge` below is what answers that, over the three-way
//! merge in `wasm-helpers`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::cli::{
    link_key, require_token_for, resolve_identifier, server_or_die, stored_token_for,
};
use crate::document::session;
use crate::http::{detail_of, get_as, text, Credentials, KEY_HEADER};
use crate::room::{decode_update, encode_update};
use crate::util::die;

/// How long the file has to stay quiet before it is read, and the session
/// before it is written. Short enough that a save shows up in a browser while
/// the author is still looking at it, long enough that an editor writing a
/// file in three syscalls is one event.
const DEFAULT_INTERVAL: Duration = Duration::from_millis(250);

/// How long to wait before dialling again, and the ceiling it backs off to. A
/// process that runs all day on a laptop that sleeps reconnects as its
/// ordinary case, so the first retry is quick and the last is not a busy loop.
const RECONNECT_FIRST: Duration = Duration::from_millis(500);
const RECONNECT_MOST: Duration = Duration::from_secs(30);

pub async fn sync_document(
    identifier: &str,
    file: &str,
    server: Option<String>,
    explicit_token: Option<String>,
    interval: String,
    key: String,
    dry_run: bool,
) {
    let target = Path::new(file);
    if target.is_dir() {
        sync_project(
            identifier,
            target,
            server,
            explicit_token,
            interval,
            key,
            dry_run,
        )
        .await;
        return;
    }
    if dry_run {
        println!("would sync file {file}");
        return;
    }
    let server = server_or_die(server);
    let every = parse_interval(&interval).unwrap_or_else(|err| die(err));
    // A link is a credential in its own right: with one, a sign-in is sent
    // if there is one and not insisted on, since the link authorizes and the
    // account only attributes. Without one, the sign-in is the whole story.
    let key = link_key(&key);
    let token = if key.is_empty() {
        require_token_for(&server, explicit_token.as_deref())
    } else {
        stored_token_for(&server, explicit_token.as_deref())
    };
    let presence_name = if token.is_empty() {
        "librepaper".to_string()
    } else {
        match crate::http::get_with_token(
            &format!("{server}/api/me"),
            &token,
            Duration::from_secs(30),
        )
        .await
        {
            Ok((200, payload)) => crate::http::text(&payload, "name"),
            _ => String::new(),
        }
    };
    let presence_name = if presence_name.is_empty() {
        "librepaper".to_string()
    } else {
        presence_name
    };
    let slug = resolve_identifier(identifier, &server, &key, explicit_token.as_deref()).await;

    // Only an editor may change a document's source, in the browser and here.
    // Asked before anything is opened, because the server drops anyone else's
    // `y-*` messages silently and a client that ran anyway would sit there
    // doing nothing.
    check_edit_permission(&server, &slug, &token, &key)
        .await
        .unwrap_or_else(|err| match err {
            PermissionError::Terminal(message) | PermissionError::Retry(message) => die(message),
        });

    let target = PathBuf::from(file);
    let _lock = Lock::take(&target).unwrap_or_else(|err| die(err));

    println!("syncing {file} with {server}/docs/{slug}");

    // The watcher outlives every connection: a save that lands while the
    // socket is down is still a save, and is reconciled on the next join.
    let (events, mut watched) = tokio::sync::mpsc::channel(64);
    let _watcher = watch(&target, events).unwrap_or_else(|err| die(err));

    let mut wait = RECONNECT_FIRST;
    let mut client = Client::new(target.clone(), every, server.clone(), token.clone())
        .with_key(&key)
        .with_presence_name(&presence_name);
    loop {
        // A socket can be closed after the server's periodic authorization
        // check. Recheck before reconnecting so a revoked editor does not
        // spin forever trying to regain a session it may no longer write.
        if client.established {
            if let Err(err) = check_edit_permission(&server, &slug, &token, &key).await {
                match err {
                    PermissionError::Terminal(message) => {
                        eprintln!("{message}");
                        return;
                    }
                    PermissionError::Retry(message) => {
                        eprintln!("{message}; reconnecting in {}", describe(wait));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = tokio::signal::ctrl_c() => return,
                        }
                        wait = (wait * 2).min(RECONNECT_MOST);
                        continue;
                    }
                }
            }
        }
        match client.run(&slug, &mut watched).await {
            // Terminal policy/limit errors end the command. Restart, writer
            // handoff, and temporary rate limits return through Err so the
            // caller preserves the outbox and reconnects with backoff.
            Ok(reason) => {
                if !reason.is_empty() {
                    eprintln!("{reason}");
                }
                return;
            }
            Err(err) => {
                eprintln!("{err}; reconnecting in {}", describe(wait));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = tokio::signal::ctrl_c() => return,
                }
                wait = (wait * 2).min(RECONNECT_MOST);
            }
        }
    }
}

/* ------------------------------------------------------------------ the peer */

/// The document, the file, and what the two last agreed on.
///
/// One value, owned by one task, so that no remote update can land between
/// reading the document and writing to it -- which is the invariant the merge
/// below rests on and the reason nothing here is behind a lock.
pub struct Client {
    target: PathBuf,
    every: Duration,
    doc: yrs::Doc,
    /// The text the file and the session last agreed on: what was last written
    /// to disk, or last read from it without conflict. The `base` of the
    /// three-way merge.
    base: String,
    /// Whether this client has ever finished a join. `false` until the first
    /// one, so a file that already diverges from the session at that point is
    /// read as the edit it plainly is rather than folded silently into
    /// `base`. `true` after that, so a later join -- a reconnection -- merges
    /// against the `base` the file and the session last agreed on instead of
    /// replacing it with whatever the session became while the socket was
    /// down, which is what `joined` below is for.
    established: bool,
    /// The digest of what this client last wrote to the file, so the event its
    /// own write causes is not read back as an edit.
    wrote: String,
    /// A change has arrived from the session, or from disk, and has not been
    /// acted on yet. The instant is when it last moved, so a burst is one act.
    from_session: Option<Instant>,
    from_disk: Option<Instant>,
    /// A file write is a deliberate act, and a deliberate act is worth a mark
    /// in the timeline -- but a burst of saves is one mark, which is what the
    /// server's own spacing and this flag together give.
    wants_checkpoint: bool,
    seq: i64,
    /// Where the deployment is and who is asking, for the one message that has
    /// to fetch something over HTTP rather than read it off the socket.
    server: String,
    token: String,
    /// The share link's key, when the client joined by one; sent beside the
    /// bearer on the upgrade and on the one fetch, the way a browser sends it.
    key: String,
    presence_clock: u32,
    last_presence: Instant,
    presence_name: String,
    /// A reconnect must receive its state before debounce work can act on the
    /// document. Otherwise a stale in-memory snapshot may overwrite the file
    /// during the interval between `y-open` and `y-state`.
    state_ready: bool,
    /// What is to be sent, in order. Every method below writes here rather
    /// than to the socket, so the whole of this client -- the merge included
    /// -- can be driven by a test with no socket at all, and so that nothing
    /// holds a Yrs transaction across an await.
    outbox: Vec<Value>,
}

impl Client {
    pub fn new(target: PathBuf, every: Duration, server: String, token: String) -> Client {
        Client {
            target,
            every,
            server,
            token,
            key: String::new(),
            presence_clock: 1,
            last_presence: Instant::now(),
            presence_name: "librepaper".to_string(),
            state_ready: false,
            outbox: Vec::new(),
            doc: session::new_doc(),
            base: String::new(),
            established: false,
            wrote: String::new(),
            from_session: None,
            from_disk: None,
            wants_checkpoint: false,
            seq: 0,
        }
    }

    /// The same client, joining by a share link's key.
    pub fn with_key(mut self, key: &str) -> Client {
        self.key = key.to_string();
        self
    }

    fn with_presence_name(mut self, name: &str) -> Client {
        self.presence_name = name.to_string();
        self
    }

    /// One message to send. The socket is drained from `run`; nothing else
    /// here knows there is one.
    fn say(&mut self, payload: Value) {
        self.outbox.push(payload);
    }

    /// A `y-update` carrying whatever this client has done since `before`.
    fn send_update(&mut self, before: &[u8]) -> Result<(), String> {
        let update = session::encode_diff(&self.doc, before)?;
        self.seq += 1;
        let seq = self.seq;
        for message in update_messages(&update, seq) {
            self.say(message);
        }
        Ok(())
    }

    /// One connection, from the handshake to the socket closing. `Ok` with a
    /// reason means the room ended it and there is nothing to reconnect to;
    /// `Err` means the connection failed and the caller should dial again.
    pub(crate) async fn run(
        &mut self,
        slug: &str,
        watched: &mut tokio::sync::mpsc::Receiver<()>,
    ) -> Result<String, String> {
        use futures_util::{SinkExt, StreamExt};

        let mut request = socket_url(&self.server, slug)
            .into_client_request()
            .map_err(|err| format!("{err}"))?;
        if !self.token.is_empty() {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| "the stored token is not a header value".to_string())?,
            );
        }
        if !self.key.is_empty() {
            request.headers_mut().insert(
                KEY_HEADER,
                self.key
                    .parse()
                    .map_err(|_| "the link key is not a header value".to_string())?,
            );
        }
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| "timed out joining the session".to_string())?
        .map_err(|err| format!("could not join the session: {err}"))?;
        let (mut write, mut read) = socket.split();
        self.state_ready = false;

        // What this client already has, so the server answers with the rest
        // and nothing more. Empty on the first connection and not on a
        // reconnection, which is what makes rejoining cheap.
        // A failed flush leaves its complete update in `outbox`. Put the
        // handshake ahead of that replay so the server has re-established the
        // sync session before it sees any queued update frames.
        self.outbox.insert(
            0,
            json!({"type": "y-open", "vector": encode_update(&session::encode_vector(&self.doc))}),
        );
        // Awareness identifies the headless client in the browser's peer
        // list. It carries no caret or editor state: the sync process has no
        // position to publish, and presence never changes its authority.
        let awareness = json!({
            "type": "y-awareness",
            "update": encode_update(&crate::cli::peer::awareness_update(
                crate::cli::peer::awareness_client_id(&self.doc),
                self.presence_clock,
                &format!("{} (sync)", self.presence_name),
                "#4f46e5",
            )),
        });
        self.outbox.insert(1, awareness);
        self.flush(&mut write).await?;

        let mut ticker = tokio::time::interval(self.every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                incoming = read.next() => {
                    let Some(frame) = incoming else {
                        return Err("the session closed".into());
                    };
                    match frame.map_err(|err| format!("the session closed: {err}"))? {
                        Message::Text(raw) => {
                            let ended = self.receive(&raw).await?;
                            self.flush(&mut write).await?;
                            if let Some(reason) = ended {
                                return Ok(reason);
                            }
                        }
                        Message::Close(frame) => {
                            let reason = frame
                                .map(|frame| frame.reason.to_string())
                                .unwrap_or_default();
                            if terminal_close_reason(&reason) {
                                return Ok(reason);
                            }
                            return Err(if reason.is_empty() {
                                "the server closed the session; reconnecting".to_string()
                            } else {
                                format!("the server closed the session ({reason}); reconnecting")
                            });
                        }
                        Message::Ping(payload) => {
                            write.send(Message::Pong(payload)).await.map_err(|err| err.to_string())?;
                        }
                        _ => {}
                    }
                }
                Some(()) = watched.recv() => {
                    self.from_disk = Some(Instant::now());
                }
                _ = ticker.tick() => {
                    if self.last_presence.elapsed() >= Duration::from_secs(10) {
                        self.presence_clock = self.presence_clock.wrapping_add(1).max(1);
                        self.last_presence = Instant::now();
                        self.say(json!({
                            "type": "y-awareness",
                            "update": encode_update(&crate::cli::peer::awareness_update(
                                crate::cli::peer::awareness_client_id(&self.doc),
                                self.presence_clock,
                                &format!("{} (sync)", self.presence_name),
                                "#4f46e5",
                            )),
                        }));
                    }
                    self.settle()?;
                    self.flush(&mut write).await?;
                }
                // Ctrl-C is how this command is stopped, so it is a way out of
                // the loop rather than something the process is killed over:
                // the lock beside the file comes off on the way, and does not
                // have to be explained to the author on their next run.
                _ = tokio::signal::ctrl_c() => {
                    return Ok("stopped".to_string());
                }
            }
        }
    }

    /// The document this client holds, for the tests that check it against a
    /// second peer.
    #[cfg(test)]
    pub fn document(&self) -> &yrs::Doc {
        &self.doc
    }

    /// What is waiting to be sent, taken. The socket is the only other reader
    /// of the outbox, and a test has none.
    #[cfg(test)]
    pub fn take_outbox(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }

    /// The debounce, without the wait. `settle` does nothing until a side has
    /// been quiet for the interval, which is a thing to make a test wait for
    /// or a thing to say has already happened; this says it.
    #[cfg(test)]
    pub fn settle_now(&mut self) -> Result<(), String> {
        if let Some(at) = self.from_session {
            self.from_session = Some(at - self.every);
        }
        if let Some(at) = self.from_disk {
            self.from_disk = Some(at - self.every);
        }
        self.settle()
    }

    #[cfg(test)]
    pub fn settle_session_now(&mut self) -> Result<(), String> {
        if let Some(at) = self.from_session {
            self.from_session = Some(at - self.every);
        }
        self.settle()
    }

    #[cfg(test)]
    pub fn mark_pending_for_test(&mut self) {
        self.from_session = Some(Instant::now() - Duration::from_secs(1));
        self.from_disk = Some(Instant::now());
    }

    #[cfg(test)]
    pub fn has_pending_disk_for_test(&self) -> bool {
        self.from_disk.is_some()
    }

    /// Reads the file now, as the watcher and the debounce together would.
    #[cfg(test)]
    pub fn read_now(&mut self) -> Result<(), String> {
        self.read_file()
    }

    /// Drains the outbox onto the socket. The one place this module writes to
    /// it, so everything above can be run without one.
    async fn flush(&mut self, write: &mut Socket) -> Result<(), String> {
        let pending = std::mem::take(&mut self.outbox);
        for payload in pending.iter().cloned() {
            if let Err(err) = send(write, payload).await {
                // A multipart update is meaningful only as a complete
                // sequence. Restore everything, including frames that may
                // have reached the old socket, so the next connection starts
                // with its y-update-start and can safely replay the update.
                self.outbox = pending;
                return Err(err);
            }
        }
        Ok(())
    }

    /// One message from the room. `Some(reason)` means it ended the session.
    pub async fn receive(&mut self, raw: &str) -> Result<Option<String>, String> {
        let Ok(message): Result<Value, _> = serde_json::from_str(raw) else {
            return Ok(None);
        };
        match text(&message, "type").as_str() {
            // The document as the server holds it. It arrives inline, or --
            // when it is too large for a text frame -- as a same-origin URL to
            // fetch it from, which is the path the browser takes too.
            "y-state" => {
                let update = match message.get("ref").and_then(Value::as_str) {
                    Some(reference) => {
                        fetch_state(&self.server, reference, &self.token, &self.key).await?
                    }
                    None => decode_update(&text(&message, "update")).unwrap_or_default(),
                };
                if !update.is_empty() {
                    session::apply_update(&self.doc, &update)?;
                }
                self.joined()?;
                self.state_ready = true;
            }
            "y-update" => {
                let Some(update) = decode_update(&text(&message, "update")) else {
                    return Ok(None);
                };
                session::apply_update(&self.doc, &update)?;
                self.from_session = Some(Instant::now());
            }
            // An older server asking for everything. One that holds the
            // document never does, but answering costs one encode.
            "y-snapshot" => {
                let whole = session::encode_state(&self.doc);
                self.seq += 1;
                let seq = self.seq;
                for message in update_messages(&whole, seq) {
                    self.say(message);
                }
            }
            "y-peers" => {
                if let Some(count) = message.get("count").and_then(Value::as_i64) {
                    println!("session has {count} peer{}", plural(count));
                }
            }
            "y-checkpoint" => {
                let sha = text(&message, "sha");
                println!("checkpoint {}", sha.chars().take(7).collect::<String>());
            }
            "error" => {
                let said = text(&message, "message");
                if !said.is_empty() {
                    eprintln!("{said}");
                }
            }
            // Awareness is who is here now, and this client has no caret to
            // show. It is applied by nobody and relayed to nobody.
            _ => {}
        }
        Ok(None)
    }

    /// What to do once the document has arrived: reconcile the file against
    /// it, and hand the server everything this client has that it may not.
    ///
    /// The catch-up is the whole state as one update. Applying it is
    /// idempotent, so it costs nothing when there was nothing to catch up, and
    /// it is strictly better than replaying the individual updates a dropped
    /// socket never acknowledged: it cannot miss one.
    ///
    /// A `y-state` arrives both on the first join and on every reconnection
    /// after, and the two must not be reconciled the same way. On a first
    /// join nothing has been agreed on yet, so the file's whole text is
    /// rightly taken as the change it plainly is. On a reconnection `base`
    /// still holds the last text the file and the session agreed on before
    /// the socket dropped, and the session may well have moved on its own
    /// while this client was away -- an author typing in a browser, a
    /// restore. Replacing `base` with the state that just arrived, before
    /// reconciling, would make that remote progress look like something the
    /// file deliberately deleted: an untouched file, diffed against the new
    /// remote text instead of the old base, is missing every word the
    /// session gained, and the merge would carry those deletions right back
    /// out to the server. So a reconnection reconciles against the base that
    /// was already there, exactly as `read_file` does for an ordinary local
    /// edit, and only `reconcile` -- once it has done that three-way merge --
    /// moves `base` forward.
    fn joined(&mut self) -> Result<(), String> {
        let remote = session::text_of(&self.doc);
        match std::fs::read(&self.target) {
            Ok(raw) => {
                let local = normalise(&raw)?;
                if local == remote {
                    // Nothing to reconcile: the file already says what the
                    // session does, on a first join or a reconnection alike.
                    self.base = remote;
                } else if self.established {
                    // A reconnection: merge against the base the file and the
                    // session last agreed on, not against the state that just
                    // arrived.
                    self.reconcile(&local)?;
                } else {
                    // A first join: seed `base` with the session text, then
                    // merge so the file's whole divergence is treated as its
                    // deliberate change.
                    self.base = remote.clone();
                    self.reconcile(&local)?;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // How an author pulls a document down to edit locally.
                self.write_file(&remote)?;
                println!("wrote {} from the session", self.target.display());
                self.base = remote;
            }
            Err(err) => return Err(format!("could not read {}: {err}", self.target.display())),
        }
        self.established = true;
        let whole = session::encode_state(&self.doc);
        self.seq += 1;
        let seq = self.seq;
        for message in update_messages(&whole, seq) {
            self.say(message);
        }
        Ok(())
    }

    /// The debounce, both directions. Called on every tick; does nothing until
    /// one side has been quiet for the interval.
    fn settle(&mut self) -> Result<(), String> {
        if !self.state_ready {
            return Ok(());
        }
        let now = Instant::now();
        if self
            .from_disk
            .is_some_and(|at| now.duration_since(at) >= self.every)
        {
            self.from_disk = None;
            self.read_file()?;
        }
        if self
            .from_session
            .is_some_and(|at| now.duration_since(at) >= self.every)
        {
            // A local save may still be in its own debounce window. Reading
            // that save and merging it is the only safe way to decide what
            // the session should write; replacing the file first loses it.
            if self.from_disk.is_some() {
                return Ok(());
            }
            self.from_session = None;
            let wanted = session::text_of(&self.doc);
            if wanted != self.base {
                self.write_file(&wanted)?;
                println!("session changed: wrote {}", self.target.display());
                self.base = wanted;
            }
        }
        self.finish_checkpoint()
    }

    fn finish_checkpoint(&mut self) -> Result<(), String> {
        if self.wants_checkpoint {
            self.wants_checkpoint = false;
            // The server spaces requested checkpoints and writes nothing when
            // the text is already the newest one, so a burst of saves is one
            // mark in the timeline.
            self.say(json!({"type": "y-checkpoint", "why": "sync"}));
        }
        Ok(())
    }

    /// The file changed. Read it, and put whatever it says into the document.
    fn read_file(&mut self) -> Result<(), String> {
        let raw = match std::fs::read(&self.target) {
            Ok(raw) => raw,
            // A save by rename can leave the path missing for an instant. It
            // is not gone; the next event will find it.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(format!("could not read {}: {err}", self.target.display())),
        };
        let local = match normalise(&raw) {
            Ok(text) => text,
            Err(why) => {
                eprintln!("{}: {why}", self.target.display());
                return Ok(());
            }
        };
        let local_digest = digest(&local);
        // The write this client made itself, coming back through the watcher.
        // Consume the marker. Keeping it forever makes a deliberate undo to a
        // previous client-written version look like an echo and skip sync.
        if local_digest == self.wrote {
            self.wrote.clear();
            return Ok(());
        }
        self.wrote.clear();
        if local == session::text_of(&self.doc) {
            // The file caught up with the session by some other route, or an
            // editor wrote the same bytes back. Nothing moved.
            self.base = local;
            return Ok(());
        }
        self.reconcile(&local)
    }

    /// The merge, which is the only hard part.
    ///
    /// `base` is what the file and the session last agreed on, `local` what
    /// the file says now, `remote` what the document says now. When the
    /// session has not moved the file is simply right, and the merge says so
    /// exactly. When both have moved, edits to different regions all go
    /// through; where both changed the same words the session wins, because it
    /// is what every other peer and every reader is looking at, and because
    /// the file's author is looking at a terminal that can tell them.
    fn reconcile(&mut self, local: &str) -> Result<(), String> {
        let remote = session::text_of(&self.doc);
        let merged = wasm_helpers::text::merge(&self.base, local, &remote);
        for conflict in &merged.conflicts {
            println!(
                "kept the session's words over yours near {:?} (yours: {:?})",
                trim(&conflict.remote),
                trim(&conflict.local)
            );
        }
        // What the file has that the session does not. Empty when the file was
        // merely catching up, which is the case where the author did nothing
        // and nothing is owed to the timeline.
        let edits = wasm_helpers::text::diff(&remote, &merged.text);
        if !edits.is_empty() {
            let before = session::encode_vector(&self.doc);
            session::apply_edits(&self.doc, &edits);
            self.send_update(&before)?;
            println!("{} changed", self.target.display());
            // Writing the file is a deliberate act, and a deliberate act is
            // worth a mark in the timeline. Writing the session's own words
            // back into the file is not one.
            self.wants_checkpoint = true;
        }
        // Where the session won, or moved while the file was being read, the
        // file is behind what everyone else is looking at, so it is brought
        // forward. Otherwise the file already says what the merge says and
        // rewriting it would only wake the author's editor for nothing.
        if merged.text != local {
            self.write_file(&merged.text)?;
            println!("session changed: wrote {}", self.target.display());
        }
        self.base = merged.text;
        Ok(())
    }

    /// Written to a temporary file beside the target and renamed over it, so
    /// an editor never reads half a write and a crash never leaves half a
    /// file. The permissions of what is replaced are kept.
    fn write_file(&mut self, body: &str) -> Result<(), String> {
        let beside = self
            .target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = self
            .target
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "document".to_string());
        let temporary = beside.join(format!(".{name}.librepaper-{}", std::process::id()));
        std::fs::write(&temporary, body)
            .map_err(|err| format!("could not write beside {}: {err}", self.target.display()))?;
        #[cfg(unix)]
        if let Ok(was) = std::fs::metadata(&self.target) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &temporary,
                std::fs::Permissions::from_mode(was.permissions().mode()),
            );
        }
        std::fs::rename(&temporary, &self.target).map_err(|err| {
            let _ = std::fs::remove_file(&temporary);
            format!("could not write {}: {err}", self.target.display())
        })?;
        // Remembered before the watcher can report it, so the event this write
        // causes is not read back as somebody's edit.
        self.wrote = digest(body);
        Ok(())
    }
}

type Socket = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

pub(crate) async fn send(write: &mut Socket, payload: Value) -> Result<(), String> {
    use futures_util::SinkExt;
    write
        .send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|err| format!("could not write to the session: {err}"))
}

/// The document's whole state, when it was too large for a text frame. Same
/// origin, signed and short-lived, and the signature is not the authorization:
/// the bearer and the link key say who is asking, as they do everywhere else.
pub(crate) async fn fetch_state(
    server: &str,
    reference: &str,
    token: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let target = state_reference(server, reference)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("could not create HTTP client: {err}"))?;
    let mut request = client.get(&target);
    for (name, value) in Credentials::new(token, key).headers() {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("could not fetch the document: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "could not fetch the document ({})",
            response.status().as_u16()
        ));
    }
    Ok(response
        .bytes()
        .await
        .map_err(|err| format!("could not fetch the document: {err}"))?
        .to_vec())
}

/// Resolve a state reference while keeping credentials on the deployment that
/// issued it. A malicious or stale server response must not turn the bearer
/// token or link key into credentials for an unrelated origin.
pub(crate) fn state_reference(server: &str, reference: &str) -> Result<String, String> {
    let origin = url::Url::parse(server).map_err(|_| "invalid document server URL")?;
    let target = origin
        .join(reference)
        .map_err(|_| "invalid document state reference")?;
    if target.origin() != origin.origin()
        || !target.username().is_empty()
        || target.password().is_some()
    {
        return Err("refused to send document credentials to another origin".into());
    }
    Ok(target.to_string())
}

/// Build bounded frames for the room's multipart update protocol. The raw Yjs
/// update is split before base64 encoding, keeping every JSON text frame well
/// below the server's one-megabyte WebSocket limit.
pub(crate) fn update_messages(update: &[u8], seq: i64) -> Vec<Value> {
    const CHUNK_SIZE: usize = 600_000;
    if update.is_empty() {
        return Vec::new();
    }
    let chunks = update.len().div_ceil(CHUNK_SIZE);
    let mut messages = Vec::with_capacity(chunks + 2);
    messages.push(json!({
        "type": "y-update-start",
        "seq": seq,
        "size": update.len(),
        "chunks": chunks,
    }));
    for (index, chunk) in update.chunks(CHUNK_SIZE).enumerate() {
        messages.push(json!({
            "type": "y-update-chunk",
            "seq": seq,
            "index": index,
            "update": encode_update(chunk),
        }));
    }
    messages.push(json!({"type": "y-update-end", "seq": seq}));
    messages
}

/// Whether a close reason means there is no point reconnecting.
///
/// A close frame carries nothing but text, so this has to read one. The
/// refusals the room itself decides are not listed again here: it owns that
/// table, and asking it keeps a reworded refusal from quietly turning a
/// permanent one into an endless reconnect loop. What remains are the
/// reasons the socket layer sends on its own.
fn terminal_close_reason(reason: &str) -> bool {
    crate::room::error::permanent_close_reason(reason)
        || matches!(
            reason,
            "document deleted" | "invalid multipart document update" | "editing is not permitted"
        )
}

enum PermissionError {
    Terminal(String),
    Retry(String),
}

async fn check_edit_permission(
    server: &str,
    slug: &str,
    token: &str,
    key: &str,
) -> Result<(), PermissionError> {
    let (status, document) = get_as(
        &format!("{server}/api/documents/{slug}"),
        &Credentials::new(token, key),
        Duration::from_secs(30),
    )
    .await
    .map_err(|err| {
        PermissionError::Retry(format!("could not check document permissions: {err}"))
    })?;
    if status == 404 {
        return Err(PermissionError::Terminal(format!(
            "no document {slug:?} at {server} ({}); `librepaper publish` makes one",
            detail_of(&document)
        )));
    }
    if matches!(status, 401 | 403) {
        return Err(PermissionError::Terminal(format!(
            "access to document {slug:?} no longer permits syncing ({})",
            detail_of(&document)
        )));
    }
    if status != 200 {
        return Err(PermissionError::Retry(format!(
            "could not check document permissions at {server} ({})",
            detail_of(&document)
        )));
    }
    let role = text(&document, "role");
    let may_edit = document.get("can_edit") == Some(&Value::Bool(true))
        || matches!(role.as_str(), "editor" | "owner");
    if !may_edit {
        return Err(PermissionError::Terminal(format!(
            "you may read {slug} but not edit it; syncing writes the document"
        )));
    }
    Ok(())
}

pub fn socket_url(server: &str, slug: &str) -> String {
    let base = server.trim_end_matches('/');
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .unwrap_or_else(|| format!("ws://{base}"));
    format!("{base}/ws/{slug}")
}

/* --------------------------------------------------------------- the watcher */

/// Watches the file's parent directory, filtered to the one name.
///
/// The directory rather than the file, because vim and its like write a new
/// file and rename it over the old one: the inode changes, and a watch on the
/// file itself would be watching something nobody will write to again. The
/// filter is what keeps an editor's swap file, backup and `paper.typ~` out.
///
/// The channel carries no payload. Which event it was does not matter -- the
/// file is read and compared by digest either way -- and the debounce means a
/// save reported as three events is one read.
fn watch(
    target: &Path,
    events: tokio::sync::mpsc::Sender<()>,
) -> Result<notify::RecommendedWatcher, String> {
    use notify::Watcher;
    let beside = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = target
        .file_name()
        .map(|name| name.to_os_string())
        .ok_or_else(|| format!("{} is not a file to watch", target.display()))?;
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else { return };
        if !matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Modify(_)
                | notify::EventKind::Remove(_)
        ) {
            return;
        }
        if event
            .paths
            .iter()
            .any(|path| path.file_name() == Some(name.as_os_str()))
        {
            let _ = events.try_send(());
        }
    })
    .map_err(|err| format!("could not watch {}: {err}", beside.display()))?;
    watcher
        .watch(&beside, notify::RecursiveMode::NonRecursive)
        .map_err(|err| format!("could not watch {}: {err}", beside.display()))?;
    Ok(watcher)
}

/* ------------------------------------------------------------------ the lock */

/// Two people cannot have the same file, but one person can run this twice.
/// The second is refused, and told which process holds it.
#[derive(Debug)]
pub struct Lock {
    file: File,
}

impl Lock {
    pub fn take(target: &Path) -> Result<Lock, String> {
        let at = lock_path(target);
        use fs2::FileExt;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&at)
            .map_err(|err| format!("could not take {}: {err}", at.display()))?;
        if file.try_lock_exclusive().is_err() {
            let held = std::fs::read_to_string(&at).unwrap_or_default();
            return Err(format!(
                "{} is already being synced{}; stop it and let its process release the lock",
                target.display(),
                if held.trim().is_empty() {
                    String::new()
                } else {
                    format!(" by process {}", held.trim())
                }
            ));
        }
        let mut file = file;
        file.set_len(0)
            .and_then(|_| {
                use std::io::{Seek, SeekFrom, Write};
                file.seek(SeekFrom::Start(0))?;
                write!(&file, "{}", std::process::id())
            })
            .map_err(|err| {
                let _ = file.unlock();
                format!("could not write {}: {err}", at.display())
            })?;
        Ok(Lock { file })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn lock_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".librepaper-lock");
    target.with_file_name(name)
}

/* ----------------------------------------------------------------- odds and ends */

/// What the file says, as the document would hold it: UTF-8, with CRLF folded
/// to LF. A trailing newline is not touched -- an editor that adds one has
/// made an edit like any other, and a rule that stripped it would fight that
/// editor forever.
fn normalise(raw: &[u8]) -> Result<String, String> {
    let text = String::from_utf8(raw.to_vec())
        .map_err(|_| "not valid UTF-8; leaving it alone".to_string())?;
    Ok(text.replace("\r\n", "\n"))
}

fn digest(body: &str) -> String {
    crate::document::store::digest_of_bytes(body.as_bytes())
}

/// A conflicting region, short enough for one line of a terminal.
fn trim(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 40 {
        return flat;
    }
    format!("{}…", flat.chars().take(39).collect::<String>())
}

fn plural(count: i64) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn describe(wait: Duration) -> String {
    if wait.as_secs() >= 1 {
        format!("{}s", wait.as_secs())
    } else {
        format!("{}ms", wait.as_millis())
    }
}

/// `--interval`, in the shape the other durations on the command line take:
/// `250ms`, `1s`, or a bare number of milliseconds.
pub fn parse_interval(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(DEFAULT_INTERVAL);
    }
    let invalid = || format!("invalid interval {value:?}; try 250ms or 1s");
    let (number, scale) = if let Some(rest) = value.strip_suffix("ms") {
        (rest, 1.0)
    } else if let Some(rest) = value.strip_suffix('s') {
        (rest, 1000.0)
    } else {
        (value, 1.0)
    };
    let amount: f64 = number.trim().parse().map_err(|_| invalid())?;
    let millis = amount * scale;
    // Below fifty milliseconds an editor writing a file in several syscalls is
    // read half written; above ten seconds nobody would call it syncing.
    if !(50.0..=10_000.0).contains(&millis) {
        return Err("--interval must be between 50ms and 10s".into());
    }
    Ok(Duration::from_millis(millis as u64))
}

/* ----------------------------------------------------------- project mirror */

/// A persisted, path-keyed agreement between the local project and the room.
/// It is a readable JSON sidecar with a compact Yjs checkpoint: the sidecar
/// remains usable while the process is offline and is enough to distinguish an
/// intentional deletion from a file that was never shared.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct ProjectBaseline {
    version: u8,
    /// The server and document this checkpoint belongs to. A sidecar follows
    /// the directory, so it must never become the base for another document.
    #[serde(default)]
    identity: String,
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default)]
    assets: BTreeMap<String, String>,
    /// The complete Yjs state at the checkpoint. Text and asset maps make the
    /// sidecar readable, while this state preserves local updates that may
    /// have been queued but not acknowledged when the process stopped.
    #[serde(default)]
    state: Vec<u8>,
}

#[derive(Default)]
struct ProjectInventory {
    all: BTreeSet<String>,
    files: BTreeMap<String, String>,
    assets: BTreeMap<String, (Vec<u8>, String)>,
}

/// Directory mode uses the same source and asset maps as the browser. The
/// single-file path above remains the default, while passing a directory as
/// the existing `file` argument opts into this project mirror.
pub async fn sync_project(
    identifier: &str,
    root: &Path,
    server: Option<String>,
    explicit_token: Option<String>,
    interval: String,
    key: String,
    dry_run: bool,
) {
    if dry_run {
        let main = discover_project_main(root).unwrap_or_default();
        let inventory =
            project_inventory(root, &main, &BTreeSet::new()).unwrap_or_else(|err| die(err));
        println!(
            "would sync project {} ({} source files, {} assets){}",
            root.display(),
            inventory.files.len(),
            inventory.assets.len(),
            if main.is_empty() {
                String::new()
            } else {
                format!(", main {main}")
            },
        );
        for path in inventory.files.keys() {
            println!("source {path}");
        }
        for path in inventory.assets.keys() {
            println!("asset {path}");
        }
        return;
    }
    let server = server_or_die(server);
    let every = parse_interval(&interval).unwrap_or_else(|err| die(err));
    let key = link_key(&key);
    let token = if key.is_empty() {
        require_token_for(&server, explicit_token.as_deref())
    } else {
        stored_token_for(&server, explicit_token.as_deref())
    };
    let slug = resolve_identifier(identifier, &server, &key, explicit_token.as_deref()).await;
    check_edit_permission(&server, &slug, &token, &key)
        .await
        .unwrap_or_else(|err| match err {
            PermissionError::Terminal(message) | PermissionError::Retry(message) => die(message),
        });
    let root = root.canonicalize().unwrap_or_else(|err| {
        die(format!(
            "could not resolve project {}: {err}",
            root.display()
        ))
    });
    let _lock = Lock::take(&root.join(".librepaper-project")).unwrap_or_else(|err| die(err));
    let (events, mut watched) = tokio::sync::mpsc::channel(128);
    let _watcher = watch_project(&root, events).unwrap_or_else(|err| die(err));
    let mut wait = RECONNECT_FIRST;
    let mut client = ProjectClient::new(
        root.clone(),
        every,
        server.clone(),
        token.clone(),
        key.clone(),
    )
    .with_slug(slug.clone());
    println!(
        "syncing project {} with {server}/docs/{slug}",
        root.display()
    );
    loop {
        if client.established {
            if let Err(err) = check_edit_permission(&server, &slug, &token, &key).await {
                match err {
                    PermissionError::Terminal(message) => {
                        eprintln!("{message}");
                        return;
                    }
                    PermissionError::Retry(message) => {
                        eprintln!("{message}; reconnecting in {}", describe(wait));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = tokio::signal::ctrl_c() => return,
                        }
                        wait = (wait * 2).min(RECONNECT_MOST);
                        continue;
                    }
                }
            }
        }
        match client.run(&mut watched).await {
            Ok(reason) => {
                if !reason.is_empty() {
                    eprintln!("{reason}");
                }
                return;
            }
            Err(err) => {
                eprintln!("{err}; reconnecting in {}", describe(wait));
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = tokio::signal::ctrl_c() => return,
                }
                wait = (wait * 2).min(RECONNECT_MOST);
            }
        }
    }
}

struct ProjectClient {
    root: PathBuf,
    every: Duration,
    server: String,
    token: String,
    key: String,
    slug: String,
    identity: String,
    doc: yrs::Doc,
    baseline: ProjectBaseline,
    established: bool,
    state_ready: bool,
    from_disk: Option<Instant>,
    from_session: Option<Instant>,
    seq: i64,
    outbox: Vec<Value>,
    incoming: Option<(i64, Vec<u8>)>,
    /// A restored state must be offered to the server after every reconnect.
    /// It remains set until the flush containing the full update succeeds.
    needs_full_sync: bool,
    full_sync_queued: bool,
}

impl ProjectClient {
    fn new(root: PathBuf, every: Duration, server: String, token: String, key: String) -> Self {
        let baseline = load_project_baseline(&root).unwrap_or_default();
        let established = baseline.version == 1;
        Self {
            root,
            every,
            server,
            token,
            key,
            slug: String::new(),
            identity: String::new(),
            doc: session::new_doc(),
            baseline,
            established,
            state_ready: false,
            from_disk: None,
            from_session: None,
            seq: 0,
            outbox: Vec::new(),
            incoming: None,
            needs_full_sync: false,
            full_sync_queued: false,
        }
    }

    fn with_slug(mut self, slug: String) -> Self {
        self.slug = slug;
        self.identity = format!("{}/docs/{}", self.server.trim_end_matches('/'), self.slug);
        if self.baseline.identity != self.identity {
            self.baseline = ProjectBaseline::default();
            self.doc = session::new_doc();
            self.established = false;
        }
        if !self.baseline.state.is_empty() {
            if session::apply_update(&self.doc, &self.baseline.state).is_ok() {
                self.needs_full_sync = true;
            } else {
                // A corrupt or incompatible checkpoint must not prevent a
                // fresh reconciliation from the local files and server.
                self.baseline = ProjectBaseline::default();
                self.doc = session::new_doc();
                self.established = false;
            }
        }
        self
    }

    fn say(&mut self, message: Value) {
        self.outbox.push(message);
    }

    async fn run(
        &mut self,
        watched: &mut tokio::sync::mpsc::Receiver<()>,
    ) -> Result<String, String> {
        use futures_util::{SinkExt, StreamExt};
        let mut request = socket_url(&self.server, &self.slug)
            .into_client_request()
            .map_err(|err| err.to_string())?;
        if !self.token.is_empty() {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", self.token)
                    .parse()
                    .map_err(|_| "the stored token is not a header value".to_string())?,
            );
        }
        if !self.key.is_empty() {
            request.headers_mut().insert(
                KEY_HEADER,
                self.key
                    .parse()
                    .map_err(|_| "the link key is not a header value".to_string())?,
            );
        }
        let (socket, _) = tokio::time::timeout(
            Duration::from_secs(30),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .map_err(|_| "timed out joining the session".to_string())?
        .map_err(|err| format!("could not join the session: {err}"))?;
        let (mut write, mut read) = socket.split();
        self.state_ready = false;
        self.needs_full_sync = !self.baseline.state.is_empty();
        self.full_sync_queued = false;
        self.outbox.insert(
            0,
            json!({
                "type": "y-open",
                "vector": encode_update(&session::encode_vector(&self.doc)),
            }),
        );
        let awareness = json!({
            "type": "y-awareness",
            "update": encode_update(&crate::cli::peer::awareness_update(
                crate::cli::peer::awareness_client_id(&self.doc),
                1,
                "librepaper (sync)",
                "#4f46e5",
            )),
        });
        self.outbox.insert(1, awareness);
        self.flush(&mut write).await?;
        let mut ticker = tokio::time::interval(self.every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                incoming = read.next() => {
                    let Some(frame) = incoming else { return Err("the session closed".into()); };
                    match frame.map_err(|err| format!("the session closed: {err}"))? {
                        Message::Text(raw) => {
                            if let Some(reason) = self.receive(&raw).await? {
                                return Ok(reason);
                            }
                            self.flush(&mut write).await?;
                        }
                        Message::Close(frame) => {
                            let reason = frame.map(|value| value.reason.to_string()).unwrap_or_default();
                            if terminal_close_reason(&reason) { return Ok(reason); }
                            return Err(if reason.is_empty() { "the server closed the session; reconnecting".into() } else { format!("the server closed the session ({reason}); reconnecting") });
                        }
                        Message::Ping(payload) => { write.send(Message::Pong(payload)).await.map_err(|err| err.to_string())?; }
                        _ => {}
                    }
                }
                Some(()) = watched.recv() => { self.from_disk = Some(Instant::now()); }
                _ = ticker.tick() => {
                    self.settle().await?;
                    self.flush(&mut write).await?;
                }
                _ = tokio::signal::ctrl_c() => return Ok("stopped".into()),
            }
        }
    }

    async fn flush(&mut self, write: &mut Socket) -> Result<(), String> {
        let pending = std::mem::take(&mut self.outbox);
        for message in pending.iter().cloned() {
            if let Err(err) = send(write, message).await {
                self.outbox = pending;
                return Err(err);
            }
        }
        if self.full_sync_queued {
            self.full_sync_queued = false;
            self.needs_full_sync = false;
        }
        Ok(())
    }

    async fn receive(&mut self, raw: &str) -> Result<Option<String>, String> {
        let Ok(message): Result<Value, _> = serde_json::from_str(raw) else {
            return Ok(None);
        };
        match text(&message, "type").as_str() {
            "y-state" => {
                let update = match message.get("ref").and_then(Value::as_str) {
                    Some(reference) => {
                        fetch_state(&self.server, reference, &self.token, &self.key).await?
                    }
                    None => decode_update(&text(&message, "update")).unwrap_or_default(),
                };
                if !update.is_empty() {
                    session::apply_update(&self.doc, &update)?;
                }
                self.state_ready = true;
                self.reconcile_local().await?;
                if self.needs_full_sync {
                    let whole = session::encode_state(&self.doc);
                    self.seq += 1;
                    for frame in update_messages(&whole, self.seq) {
                        self.say(frame);
                    }
                    self.full_sync_queued = true;
                }
            }
            "y-update" => {
                if let Some(update) = decode_update(&text(&message, "update")) {
                    session::apply_update(&self.doc, &update)?;
                    self.from_session = Some(Instant::now());
                }
            }
            "y-update-start" => {
                let seq = message.get("seq").and_then(Value::as_i64).unwrap_or(0);
                self.incoming = Some((seq, Vec::new()));
            }
            "y-update-chunk" => {
                if let Some((_, bytes)) = &mut self.incoming {
                    if let Some(chunk) = decode_update(&text(&message, "update")) {
                        bytes.extend(chunk);
                    }
                }
            }
            "y-update-end" => {
                if let Some((_, bytes)) = self.incoming.take() {
                    if !bytes.is_empty() {
                        session::apply_update(&self.doc, &bytes)?;
                        self.from_session = Some(Instant::now());
                    }
                }
            }
            "y-snapshot" => {
                let update = session::encode_state(&self.doc);
                self.seq += 1;
                for frame in update_messages(&update, self.seq) {
                    self.say(frame);
                }
            }
            "y-peers" => {
                if let Some(count) = message.get("count").and_then(Value::as_i64) {
                    println!("session has {count} peer{}", plural(count));
                }
            }
            "y-checkpoint" => {
                let sha = text(&message, "sha");
                println!("checkpoint {}", sha.chars().take(7).collect::<String>());
            }
            "error" => {
                let said = text(&message, "message");
                if !said.is_empty() {
                    eprintln!("{said}");
                }
            }
            _ => {}
        }
        Ok(None)
    }

    async fn settle(&mut self) -> Result<(), String> {
        if !self.state_ready {
            return Ok(());
        }
        let now = Instant::now();
        if self
            .from_disk
            .is_some_and(|at| now.duration_since(at) >= self.every)
        {
            self.from_disk = None;
            self.reconcile_local().await?;
        }
        if self
            .from_session
            .is_some_and(|at| now.duration_since(at) >= self.every)
        {
            self.from_session = None;
            // A remote update and a local save can cross while disconnected.
            // Reconcile both against the persisted base before writing either
            // side; writing the remote snapshot directly loses the local edit.
            self.reconcile_local().await?;
        }
        Ok(())
    }

    async fn reconcile_local(&mut self) -> Result<(), String> {
        let mut remote = session::texts_of(&self.doc);
        let remote_assets = session::assets_of(&self.doc);
        let main = session::main_path(&self.doc);
        if !crate::document::render::is_quarto(&main) {
            return Err("Project synchronization requires a Quarto .qmd entrypoint".into());
        }
        let mut known_paths: BTreeSet<String> = self.baseline.files.keys().cloned().collect();
        known_paths.extend(self.baseline.assets.keys().cloned());
        known_paths.extend(remote.keys().cloned());
        known_paths.extend(remote_assets.keys().cloned());
        let inventory = project_inventory(&self.root, &main, &known_paths)?;
        let merged = merge_project_texts(&self.baseline.files, &inventory, &remote);
        let mut asset_conflicts = BTreeMap::new();
        let before = session::encode_vector(&self.doc);
        let mut changed = false;
        // Preserve Y.Text identity for a local filesystem rename whenever the
        // old shared body is still present and the new path is its only match.
        // This keeps browser carets and concurrent edits attached to the file.
        for (new_path, body) in &merged {
            if remote.contains_key(new_path) || !inventory.all.contains(new_path) {
                continue;
            }
            if merged
                .iter()
                .filter(|(path, value)| *value == body && !remote.contains_key(path.as_str()))
                .count()
                != 1
            {
                continue;
            }
            let candidates: Vec<_> = self
                .baseline
                .files
                .iter()
                .filter(|(old_path, old_body)| {
                    *old_body == body
                        && !merged.contains_key((*old_path).as_str())
                        && !inventory.all.contains((*old_path).as_str())
                        && remote.get((*old_path).as_str()) == Some(old_body)
                })
                .collect();
            if candidates.len() != 1 {
                continue;
            }
            let (old_path, _) = candidates[0];
            let old_path = old_path.clone();
            if session::rename_path(&self.doc, &old_path, new_path) {
                remote.remove(&old_path);
                remote.insert(new_path.clone(), body.clone());
                changed = true;
            }
        }
        for (path, body) in &merged {
            if remote.get(path) != Some(body) {
                session::put_text(&self.doc, path, body);
                changed = true;
            }
        }
        for path in remote.keys().filter(|path| !merged.contains_key(*path)) {
            if session::remove_path(&self.doc, path) {
                changed = true;
            }
        }
        for (path, (bytes, digest)) in &inventory.assets {
            if remote_assets.get(path) == Some(digest) {
                continue;
            }
            if self.baseline.assets.get(path) == Some(digest) {
                // The local bytes did not move. A changed or removed remote
                // asset therefore wins the three-way decision; the tree
                // writer below fetches a replacement, while a missing remote
                // name removes the local copy.
                if !remote_assets.contains_key(path) {
                    remove_project_file(&self.root, path)?;
                }
                continue;
            }
            if let Some(base_digest) = self.baseline.assets.get(path) {
                if remote_assets.get(path).is_some_and(|remote_digest| {
                    remote_digest != base_digest && remote_digest != digest
                }) {
                    // Keep the local bytes and preserve the remote bytes in a
                    // sidecar. The next save can resolve this explicitly;
                    // silently choosing either binary would be data loss.
                    if let Some(remote_digest) = remote_assets.get(path) {
                        asset_conflicts.insert(path.clone(), remote_digest.clone());
                    }
                    continue;
                }
            }
            let sha = self.upload_asset(bytes).await?;
            if remote_assets.get(path) != Some(&sha) {
                session::put_asset(&self.doc, path, &sha);
                changed = true;
            }
        }
        for (path, digest) in &remote_assets {
            if inventory.assets.contains_key(path) {
                continue;
            }
            if inventory.all.contains(path) {
                continue;
            }
            if self.baseline.assets.get(path) == Some(digest) {
                // A local replacement has already been dealt with above; a
                // missing previously shared path is an intentional deletion.
                if session::remove_asset(&self.doc, path) {
                    changed = true;
                }
            }
        }
        if changed {
            self.seq += 1;
            let update = session::encode_diff(&self.doc, &before)?;
            for frame in update_messages(&update, self.seq) {
                self.say(frame);
            }
        }
        self.write_project_tree(&merged, &inventory, &asset_conflicts)
            .await?;
        self.baseline = ProjectBaseline {
            version: 1,
            identity: self.identity.clone(),
            files: merged,
            assets: session::assets_of(&self.doc),
            state: session::encode_state(&self.doc),
        };
        save_project_baseline(&self.root, &self.baseline)?;
        self.established = true;
        Ok(())
    }

    async fn write_project_tree(
        &self,
        files: &BTreeMap<String, String>,
        inventory: &ProjectInventory,
        asset_conflicts: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        let main = session::main_path(&self.doc);
        for (path, body) in files {
            if !project_path_is_shared(&self.root, &main, path, &inventory.all)? {
                continue;
            }
            if inventory.files.get(path) != Some(body) {
                write_project_file(&self.root, path, body)?;
            }
        }
        for path in inventory
            .files
            .keys()
            .filter(|path| !files.contains_key(*path))
        {
            remove_project_file(&self.root, path)?;
        }
        for (path, digest) in session::assets_of(&self.doc) {
            if asset_conflicts.contains_key(&path) {
                continue;
            }
            if !project_path_is_shared(&self.root, &main, &path, &inventory.all)? {
                continue;
            }
            let at = safe_project_path(&self.root, &path)?;
            let same = std::fs::read(&at)
                .ok()
                .is_some_and(|bytes| digest_of(&bytes) == digest);
            if !same {
                let bytes =
                    fetch_project_asset(&self.server, &self.slug, &digest, &self.token, &self.key)
                        .await?;
                write_project_bytes(&self.root, &path, &bytes)?;
            }
        }
        // Binary files cannot carry inline conflict markers. Keep the local
        // bytes in place and put the remote bytes beside them so neither
        // concurrent edit is silently discarded.
        for (path, digest) in asset_conflicts {
            let bytes =
                fetch_project_asset(&self.server, &self.slug, digest, &self.token, &self.key)
                    .await?;
            write_project_bytes(&self.root, &asset_conflict_path(path, digest), &bytes)?;
        }
        Ok(())
    }

    async fn upload_asset(&self, bytes: &[u8]) -> Result<String, String> {
        let endpoint = format!("{}/api/documents/{}/assets", self.server, self.slug);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| format!("could not create asset client: {err}"))?;
        let mut request = client
            .put(endpoint)
            .header("x-librepaper-client", "1")
            .body(bytes.to_vec());
        if !self.token.is_empty() {
            request = request.header("authorization", format!("Bearer {}", self.token));
        }
        if !self.key.is_empty() {
            request = request.header(KEY_HEADER, &self.key);
        }
        let response = request
            .send()
            .await
            .map_err(|err| format!("could not upload asset: {err}"))?;
        let status = response.status();
        let payload: Value = response.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            return Err(format!(
                "asset upload failed ({}): {}",
                status.as_u16(),
                detail_of(&payload)
            ));
        }
        let sha = text(&payload, "sha");
        if sha.len() != 64 {
            return Err("asset upload returned no valid digest".into());
        }
        Ok(sha)
    }
}

fn load_project_baseline(root: &Path) -> Result<ProjectBaseline, String> {
    let path = root.join(".librepaper-sync.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| format!("invalid {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ProjectBaseline::default()),
        Err(err) => Err(format!("could not read {}: {err}", path.display())),
    }
}

fn save_project_baseline(root: &Path, baseline: &ProjectBaseline) -> Result<(), String> {
    let path = root.join(".librepaper-sync.json");
    let bytes = serde_json::to_vec_pretty(baseline)
        .map_err(|err| format!("could not encode sync baseline: {err}"))?;
    let temporary = unique_temporary_path(root, std::ffi::OsStr::new("librepaper-sync.json"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|err| format!("could not create sync baseline: {err}"))?;
    use std::io::Write;
    file.write_all(&bytes)
        .map_err(|err| format!("could not write sync baseline: {err}"))?;
    file.sync_all()
        .map_err(|err| format!("could not flush sync baseline: {err}"))?;
    std::fs::rename(&temporary, &path).map_err(|err| {
        let _ = std::fs::remove_file(&temporary);
        format!("could not replace sync baseline: {err}")
    })
}

fn project_inventory(
    root: &Path,
    main: &str,
    known_paths: &BTreeSet<String>,
) -> Result<ProjectInventory, String> {
    let ignored = crate::cli::git_ignores(root);
    let mut all_paths = crate::cli::files_under(root, "", &ignored);
    // Gitignored files are normally outside this peer's authority. A path
    // that was already shared is different: the browser may have created it
    // locally during an earlier reconcile, and dropping it from the next
    // inventory would look like an intentional deletion. Re-admit only known
    // paths that are present on disk; new gitignored files remain private.
    for path in known_paths {
        if all_paths.iter().any(|candidate| candidate == path) {
            continue;
        }
        let Ok(at) = safe_project_path(root, path) else {
            continue;
        };
        if at.is_file() {
            all_paths.push(path.clone());
        }
    }
    let listed = all_paths.clone();
    let listed = if crate::document::render::is_quarto(main) {
        crate::local::engine_adapter::quarto_shared_paths(root, main, listed)
            .map_err(|err| format!("could not apply Quarto sharing policy: {err}"))?
    } else {
        listed
    };
    let mut inventory = ProjectInventory::default();
    inventory.all.extend(all_paths);
    for path in listed {
        inventory.all.insert(path.clone());
        let at = safe_project_path(root, &path)?;
        let bytes = std::fs::read(&at).map_err(|err| format!("could not read {path}: {err}"))?;
        match crate::document::paths::check(&crate::config::Configuration::default().paths(), &path)
        {
            Ok(crate::document::paths::Kind::Text) => {
                inventory.files.insert(path, normalise(&bytes)?);
            }
            Ok(crate::document::paths::Kind::Asset) => {
                inventory
                    .assets
                    .insert(path, (bytes.clone(), digest_of(&bytes)));
            }
            Err(_) => {}
        }
    }
    Ok(inventory)
}

/// Apply the same Quarto sharing policy to a path arriving from the room.
/// `project_inventory` only sees paths that exist locally; remote paths must
/// be checked independently before they are materialised on disk.
fn project_path_is_shared(
    root: &Path,
    main: &str,
    path: &str,
    available: &BTreeSet<String>,
) -> Result<bool, String> {
    if crate::document::paths::check(&crate::config::Configuration::default().paths(), path)
        .is_err()
    {
        return Ok(false);
    }
    if main.is_empty() || !crate::document::render::is_quarto(main) {
        return Ok(true);
    }
    let mut candidates: Vec<String> = available.iter().cloned().collect();
    if !candidates.iter().any(|candidate| candidate == path) {
        candidates.push(path.to_string());
    }
    Ok(
        crate::local::engine_adapter::quarto_shared_paths(root, main, candidates)?
            .iter()
            .any(|candidate| candidate == path),
    )
}

fn discover_project_main(root: &Path) -> Result<String, String> {
    let ignored = crate::cli::git_ignores(root);
    let listed = crate::cli::files_under(root, "", &ignored);
    let mut quarto: Vec<_> = listed
        .iter()
        .filter(|path| crate::document::render::is_quarto(path))
        .cloned()
        .collect();
    quarto.sort();
    if let Some(main) = quarto
        .iter()
        .find(|path| matches!(path.as_str(), "index.qmd" | "main.qmd"))
        .or_else(|| quarto.first())
    {
        return Ok(main.clone());
    }
    crate::cli::main_file(&listed, "")
}

fn merge_project_texts(
    base: &BTreeMap<String, String>,
    local: &ProjectInventory,
    remote: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut paths: BTreeSet<String> = base.keys().cloned().collect();
    paths.extend(local.files.keys().cloned());
    paths.extend(remote.keys().cloned());
    let mut merged = BTreeMap::new();
    for path in paths {
        let baseline = base.get(&path);
        let local_value = if local.files.contains_key(&path) {
            local.files.get(&path)
        } else if local.all.contains(&path) {
            // The file is present but the sharing policy excludes it (for
            // example a generated output or a private input). It is outside
            // this peer's authority, so leave the room's value alone.
            remote.get(&path)
        } else if baseline.is_some() {
            None
        } else {
            remote.get(&path)
        };
        let remote_value = remote.get(&path);
        let value = merge_project_value(baseline, local_value, remote_value);
        if let Some(value) = value {
            merged.insert(path, value);
        }
    }
    merged
}

fn merge_project_conflict(base: &str, local: &str, remote: &str) -> String {
    let merged = wasm_helpers::text::merge(base, local, remote);
    if merged.conflicts.is_empty() && !local.is_empty() && !remote.is_empty() {
        return merged.text;
    }
    format!("<<<<<<< LOCAL\n{local}\n||||||| BASE\n{base}\n=======\n{remote}\n>>>>>>> SHARED\n")
}

fn merge_project_value(
    base: Option<&String>,
    local: Option<&String>,
    remote: Option<&String>,
) -> Option<String> {
    if local == base {
        return remote.cloned();
    }
    if remote == base {
        return local.cloned();
    }
    if local == remote {
        return local.cloned();
    }
    match (local, remote) {
        (Some(local), Some(remote)) => Some(merge_project_conflict(
            base.map(String::as_str).unwrap_or(""),
            local,
            remote,
        )),
        // A concurrent deletion is a real edit. Run the text merge against
        // an empty side so the surviving content is retained with conflict
        // markers instead of deleting a locally edited file.
        (None, Some(remote)) if base.is_some() => Some(merge_project_conflict(
            base.map(String::as_str).unwrap_or(""),
            "",
            remote,
        )),
        (None, Some(remote)) => Some(remote.clone()),
        (Some(local), None) if base.is_none() => Some(local.clone()),
        (Some(local), None) => Some(merge_project_conflict(
            base.map(String::as_str).unwrap_or(""),
            local,
            "",
        )),
        (None, None) => None,
    }
}

fn safe_project_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.is_absolute() || relative.is_empty() {
        return Err(format!("unsafe project path {relative:?}"));
    }
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe project path {relative:?}"));
    }
    let candidate = root.join(path);
    // New nested files have no canonical path yet. Walk to the nearest
    // existing ancestor; canonicalising it catches symlinked directories
    // before callers create anything below them.
    let mut existing = candidate.as_path();
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| format!("could not resolve project path {relative:?}"))?;
    }
    let check = existing
        .canonicalize()
        .map_err(|err| format!("could not resolve project path {relative:?}: {err}"))?;
    let canonical_root = root
        .canonicalize()
        .map_err(|err| format!("could not resolve project root: {err}"))?;
    if !check.starts_with(&canonical_root) {
        return Err(format!("project path escapes the project: {relative:?}"));
    }
    Ok(candidate)
}

fn write_project_file(root: &Path, path: &str, body: &str) -> Result<(), String> {
    write_project_bytes(root, path, body.as_bytes())
}

fn write_project_bytes(root: &Path, path: &str, body: &[u8]) -> Result<(), String> {
    let target = safe_project_path(root, path)?;
    let parent = target
        .parent()
        .ok_or_else(|| format!("project path has no parent: {path}"))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| format!("could not create directory for {path}: {err}"))?;
    let temporary = unique_temporary_path(parent, target.file_name().unwrap_or_default());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|err| format!("could not create temporary file for {path}: {err}"))?;
    use std::io::Write;
    file.write_all(body)
        .map_err(|err| format!("could not write {path}: {err}"))?;
    file.sync_all()
        .map_err(|err| format!("could not flush {path}: {err}"))?;
    std::fs::rename(&temporary, &target).map_err(|err| {
        let _ = std::fs::remove_file(&temporary);
        format!("could not replace {path}: {err}")
    })
}

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_temporary_path(parent: &Path, name: &std::ffi::OsStr) -> PathBuf {
    loop {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{}.librepaper-{}-{sequence}",
            name.to_string_lossy(),
            std::process::id()
        ));
        if !candidate.exists() {
            return candidate;
        }
    }
}

fn asset_conflict_path(path: &str, digest: &str) -> String {
    format!(
        "{path}.librepaper-conflict-{}",
        &digest[..digest.len().min(12)]
    )
}

fn remove_project_file(root: &Path, path: &str) -> Result<(), String> {
    let target = safe_project_path(root, path)?;
    match std::fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove {path}: {error}")),
    }
}

async fn fetch_project_asset(
    server: &str,
    slug: &str,
    sha: &str,
    token: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| err.to_string())?;
    let mut request = client
        .get(format!("{server}/api/documents/{slug}/assets/{sha}"))
        .header("x-librepaper-client", "1");
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    if !key.is_empty() {
        request = request.header(KEY_HEADER, key);
    }
    let response = request
        .send()
        .await
        .map_err(|err| format!("could not fetch asset {sha}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "could not fetch asset {sha} ({})",
            response.status().as_u16()
        ));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| format!("could not read asset {sha}: {err}"))
}

fn digest_of(bytes: &[u8]) -> String {
    crate::document::store::digest_of_bytes(bytes)
}

fn watch_project(
    root: &Path,
    events: tokio::sync::mpsc::Sender<()>,
) -> Result<notify::RecommendedWatcher, String> {
    use notify::Watcher;
    let root = root.to_path_buf();
    let callback_root = root.clone();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            return;
        };
        if !matches!(
            event.kind,
            notify::EventKind::Create(_)
                | notify::EventKind::Modify(_)
                | notify::EventKind::Remove(_)
        ) {
            return;
        }
        if event.paths.iter().any(|path| {
            let relative = path
                .strip_prefix(&callback_root)
                .ok()
                .and_then(|path| path.to_str())
                .unwrap_or("");
            !relative.starts_with('.') && !relative.contains(".librepaper-")
        }) {
            let _ = events.try_send(());
        }
    })
    .map_err(|err| format!("could not watch {}: {err}", root.display()))?;
    watcher
        .watch(&root, notify::RecursiveMode::Recursive)
        .map_err(|err| format!("could not watch {}: {err}", root.display()))?;
    Ok(watcher)
}

#[cfg(test)]
mod project_tests {
    use super::*;

    #[test]
    fn project_merge_keeps_independent_edits() {
        let mut base = BTreeMap::new();
        base.insert("paper.qmd".into(), "one\ntwo\n".into());
        let mut local = ProjectInventory::default();
        local.all.insert("paper.qmd".into());
        local
            .files
            .insert("paper.qmd".into(), "one local\ntwo\n".into());
        let mut remote = BTreeMap::new();
        remote.insert("paper.qmd".into(), "one\ntwo remote\n".into());
        let merged = merge_project_texts(&base, &local, &remote);
        assert!(merged["paper.qmd"].contains("local"));
        assert!(merged["paper.qmd"].contains("remote"));
    }

    #[test]
    fn project_merge_preserves_local_only_files() {
        let mut local = ProjectInventory::default();
        local.all.insert("paper.qmd".into());
        local.files.insert("paper.qmd".into(), "local\n".into());
        let merged = merge_project_texts(&BTreeMap::new(), &local, &BTreeMap::new());
        assert_eq!(merged.get("paper.qmd").map(String::as_str), Some("local\n"));
    }

    #[test]
    fn project_merge_keeps_local_edit_when_remote_deleted_file() {
        let base = [("paper.qmd".into(), "old\n".into())].into_iter().collect();
        let mut local = ProjectInventory::default();
        local.all.insert("paper.qmd".into());
        local
            .files
            .insert("paper.qmd".into(), "local edit\n".into());
        let merged = merge_project_texts(&base, &local, &BTreeMap::new());
        let value = merged.get("paper.qmd").expect("conflict is retained");
        assert!(value.contains("local edit"));
    }

    #[test]
    fn project_paths_reject_escape_and_absolute_names() {
        let root = tempfile::tempdir().expect("temp root");
        assert!(safe_project_path(root.path(), "../outside.qmd").is_err());
        assert!(safe_project_path(root.path(), "/outside.qmd").is_err());
    }

    #[test]
    fn project_paths_allow_new_nested_files() {
        let root = tempfile::tempdir().expect("temp root");
        let path = safe_project_path(root.path(), "figures/new/plot.png").expect("safe path");
        assert_eq!(path, root.path().join("figures/new/plot.png"));
    }

    #[test]
    fn asset_conflicts_get_a_stable_sidecar_name() {
        assert_eq!(
            asset_conflict_path("fig/plot.png", &"a".repeat(64)),
            "fig/plot.png.librepaper-conflict-aaaaaaaaaaaa"
        );
    }

    #[test]
    fn project_baseline_round_trips() {
        let root = tempfile::tempdir().expect("temp root");
        let baseline = ProjectBaseline {
            version: 1,
            identity: "https://example.test/docs/demo".into(),
            files: [("paper.qmd".into(), "source\n".into())]
                .into_iter()
                .collect(),
            assets: [("fig/plot.png".into(), "a".repeat(64))]
                .into_iter()
                .collect(),
            state: Vec::new(),
        };
        save_project_baseline(root.path(), &baseline).expect("save baseline");
        assert_eq!(
            load_project_baseline(root.path())
                .expect("load baseline")
                .files,
            baseline.files
        );
    }

    #[tokio::test]
    async fn project_restart_restores_and_replays_unacknowledged_state() {
        let root = tempfile::tempdir().expect("temp root");
        std::fs::write(root.path().join("paper.qmd"), "local edit\n").expect("local file");
        let doc = session::new_doc();
        let main = session::put_text(&doc, "paper.qmd", "local edit\n");
        session::set_main(&doc, &main);
        let identity = "https://example.test/docs/demo";
        save_project_baseline(
            root.path(),
            &ProjectBaseline {
                version: 1,
                identity: identity.into(),
                files: [("paper.qmd".into(), "local edit\n".into())]
                    .into_iter()
                    .collect(),
                assets: BTreeMap::new(),
                // This represents a local edit whose update was queued just
                // before the old process stopped, before the server applied
                // it or sent an acknowledgement.
                state: session::encode_state(&doc),
            },
        )
        .expect("persist checkpoint");

        let mut restarted = ProjectClient::new(
            root.path().to_path_buf(),
            Duration::from_millis(50),
            "https://example.test".into(),
            String::new(),
            String::new(),
        )
        .with_slug("demo".into());
        assert_eq!(session::text_of(&restarted.doc), "local edit\n");

        restarted
            .receive(r#"{"type":"y-state","update":""}"#)
            .await
            .expect("reconcile restored state");
        assert!(restarted
            .outbox
            .iter()
            .any(|message| message["type"] == "y-update-start"));
        assert!(restarted.needs_full_sync);
        assert!(restarted.full_sync_queued);
    }
}
