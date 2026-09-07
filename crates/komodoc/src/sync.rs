//! `komodoc sync`: the file on disk as a peer in the session.
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
//! `crates/komodoc/src/tests/sync.rs` checks a Yrs client and the server against each
//! other rather than trusting the shared encoding.
//!
//! The hard part is not the socket. A text editor is a snapshot client: it
//! read the file at some moment, holds a buffer, and writes the buffer back
//! whole when the author saves. If the session moved meanwhile -- the same
//! author in a browser tab, a restore from the reader -- the buffer does not
//! know, and a diff of the document against the file would delete the words
//! that arrived. `The merge` below is what answers that, over the three-way
//! merge in `komodoc-text`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::cli::{link_key, require_token_for, resolve_identifier, server_from, stored_token_for};
use crate::http::{detail_of, get_as, text, Credentials, KEY_HEADER};
use crate::room::{decode_update, encode_update};
use crate::session;
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
    server_flag: String,
    interval: String,
    key: String,
) {
    let server = server_from(&server_flag);
    let every = parse_interval(&interval).unwrap_or_else(|err| die(err));
    // A link is a credential in its own right: with one, a sign-in is sent
    // if there is one and not insisted on, since the link authorizes and the
    // account only attributes. Without one, the sign-in is the whole story.
    let key = link_key(&key);
    let token = if key.is_empty() {
        require_token_for(&server)
    } else {
        stored_token_for(&server)
    };
    let presence_name = if token.is_empty() {
        "komodoc".to_string()
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
        "komodoc".to_string()
    } else {
        presence_name
    };
    let slug = resolve_identifier(identifier, &server, &key).await;

    // Only an editor may change a document's source, in the browser and here.
    // Asked before anything is opened, because the server drops anyone else's
    // `y-*` messages silently and a client that ran anyway would sit there
    // doing nothing.
    let (status, document) = get_as(
        &format!("{server}/api/documents/{slug}"),
        &Credentials::new(&token, &key),
        Duration::from_secs(30),
    )
    .await
    .unwrap_or_else(|err| die(err));
    if status != 200 {
        die(format!(
            "no document {slug:?} at {server} ({}); `komodoc publish` makes one",
            detail_of(&document)
        ));
    }
    let role = text(&document, "role");
    let may_edit = document.get("can_edit") == Some(&Value::Bool(true))
        || matches!(role.as_str(), "editor" | "owner");
    if !may_edit {
        die(format!(
            "you may read {slug} but not edit it; syncing writes the document"
        ));
    }

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
        match client.run(&slug, &mut watched).await {
            // The room closed the socket and said why -- the document was
            // deleted, or another server took it. Neither is waited out.
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
            presence_name: "komodoc".to_string(),
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
        self.say(json!({"type": "y-update", "update": encode_update(&update), "seq": seq}));
        Ok(())
    }

    /// One connection, from the handshake to the socket closing. `Ok` with a
    /// reason means the room ended it and there is nothing to reconnect to;
    /// `Err` means the connection failed and the caller should dial again.
    async fn run(
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

        // What this client already has, so the server answers with the rest
        // and nothing more. Empty on the first connection and not on a
        // reconnection, which is what makes rejoining cheap.
        self.say(
            json!({"type": "y-open", "vector": encode_update(&session::encode_vector(&self.doc))}),
        );
        // Awareness identifies the headless client in the browser's peer
        // list. It carries no caret or editor state: the sync process has no
        // position to publish, and presence never changes its authority.
        self.say(json!({
            "type": "y-awareness",
            "update": encode_update(&crate::peer::awareness_update(
                crate::peer::awareness_client_id(&self.doc),
                self.presence_clock,
                &format!("{} (sync)", self.presence_name),
                "#4f46e5",
            )),
        }));
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
                            return Ok(frame
                                .map(|frame| frame.reason.to_string())
                                .unwrap_or_default());
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
                            "update": encode_update(&crate::peer::awareness_update(
                                crate::peer::awareness_client_id(&self.doc),
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

    /// Reads the file now, as the watcher and the debounce together would.
    #[cfg(test)]
    pub fn read_now(&mut self) -> Result<(), String> {
        self.read_file()
    }

    /// Drains the outbox onto the socket. The one place this module writes to
    /// it, so everything above can be run without one.
    async fn flush(&mut self, write: &mut Socket) -> Result<(), String> {
        for payload in std::mem::take(&mut self.outbox) {
            send(write, payload).await?;
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
                self.say(json!({
                    "type": "y-update",
                    "update": encode_update(&whole),
                    "replace": true,
                    "seq": seq,
                }));
            }
            "y-peers" => {
                if let Some(count) = message.get("count").and_then(Value::as_i64) {
                    println!("joined the session ({count} peer{})", plural(count));
                }
            }
            "y-checkpoint" => {
                let sha = text(&message, "sha");
                println!("checkpoint {}", &sha[..sha.len().min(7)]);
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
                    // A first join: `base` is still empty, which makes the
                    // merge take the file's whole text as a change -- exactly
                    // right, since nothing here has agreed with anything yet.
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
        self.say(json!({"type": "y-update", "update": encode_update(&whole), "seq": seq}));
        Ok(())
    }

    /// The debounce, both directions. Called on every tick; does nothing until
    /// one side has been quiet for the interval.
    fn settle(&mut self) -> Result<(), String> {
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
            self.from_session = None;
            let wanted = session::text_of(&self.doc);
            if wanted != self.base {
                self.write_file(&wanted)?;
                println!("session changed: wrote {}", self.target.display());
                self.base = wanted;
            }
        }
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
        // The write this client made itself, coming back through the watcher.
        if digest(&local) == self.wrote {
            return Ok(());
        }
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
        let merged = komodoc_text::merge(&self.base, local, &remote);
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
        let edits = komodoc_text::diff(&remote, &merged.text);
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
        let temporary = beside.join(format!(".{name}.komodoc-{}", std::process::id()));
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

async fn send(write: &mut Socket, payload: Value) -> Result<(), String> {
    use futures_util::SinkExt;
    write
        .send(Message::Text(payload.to_string().into()))
        .await
        .map_err(|err| format!("could not write to the session: {err}"))
}

/// The document's whole state, when it was too large for a text frame. Same
/// origin, signed and short-lived, and the signature is not the authorization:
/// the bearer and the link key say who is asking, as they do everywhere else.
async fn fetch_state(
    server: &str,
    reference: &str,
    token: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let target = if reference.starts_with("http") {
        reference.to_string()
    } else {
        format!("{server}{reference}")
    };
    let mut request = reqwest::Client::new().get(&target);
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
    at: PathBuf,
}

impl Lock {
    pub fn take(target: &Path) -> Result<Lock, String> {
        let at = lock_path(target);
        if let Ok(held) = std::fs::read_to_string(&at) {
            return Err(format!(
                "{} is already being synced by process {}; \
                 stop it, or remove {} if nothing is running",
                target.display(),
                held.trim(),
                at.display()
            ));
        }
        std::fs::write(&at, std::process::id().to_string())
            .map_err(|err| format!("could not take {}: {err}", at.display()))?;
        Ok(Lock { at })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.at);
    }
}

pub fn lock_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".komodoc-lock");
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
    crate::store::digest_of_bytes(body.as_bytes())
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
