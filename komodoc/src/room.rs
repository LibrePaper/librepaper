//! A room owns one document's comments and the open sockets of everyone
//! reading it, so a write by one reader reaches the others without anybody
//! polling. One process, so the mutex plays the part a single-threaded actor
//! would.
//!
//! Field names follow the W3C Web Annotation Data Model, so exporting is a
//! reshaping rather than a translation: exact, prefix and suffix are a
//! TextQuoteSelector, motivation is the standard vocabulary, and creator and
//! created mean what the spec says. resolved is ours; the spec has no notion
//! of it, and permits extra properties.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};

use crate::blob::{
    checkpoint_key, room_key, room_lock_key, session_key, take_room_lock, BlobError, BlobStore,
    LOCK_STALE_SECONDS,
};
use crate::clock::{now_unix, timestamp};
use crate::config::{Configuration, CHECKPOINT_DEFER_SECONDS};
use crate::history::{self, Checkpoint, Manifest};
use crate::session;
use crate::util::{clean, new_id};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub id: String,
    pub body: String,
    pub creator: String,
    pub created: String,
    /// Who actually posted this reply -- a github: or visitor: key, or "" for a
    /// caller with neither -- so a delete can be restricted to it. Never
    /// serialized: a reply is marshaled directly into broadcasts, snapshots and
    /// REST responses, none of which should carry it; `to_stored` below is the
    /// only shape that puts it on disk, and loading reads it back.
    #[serde(default, skip_serializing)]
    pub author: String,
}

/// A rectangle on an image, in percentages of the image's own size, so it
/// survives the document being displayed at any width.
///
/// Which image is a harder question than where on it. There is no text around
/// a figure to anchor to, so two identifiers are kept: a digest of the image
/// source, which survives the figure moving, and its position among the
/// document's images, which survives the image being re-encoded. The reader
/// tries the digest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Region {
    #[serde(default)]
    pub image_digest: String,
    #[serde(default)]
    pub image_index: i64,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default, rename = "w")]
    pub width: f64,
    #[serde(default, rename = "h")]
    pub height: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub seq: i64,
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// Where the passage sat when the comment was made. Null for a comment
    /// written before it was recorded, rather than claiming offset 0.
    #[serde(default)]
    pub position: Option<i64>,
    /// Set instead of the text selector when the annotation is on part of a
    /// figure rather than on a run of words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// Who actually posted this comment: "github:<login>" for a signed-in
    /// caller, "visitor:<sha256 of the visitor token>" for a verified
    /// anonymous browser, or "" for neither (including every seeded example,
    /// which belongs to nobody in particular). Kept out of every client-bound
    /// shape for the same reason as `Reply::author`.
    #[serde(default, skip_serializing)]
    pub author: String,
}

/// What lands on disk, one object per document. Author is excluded from a
/// comment's own JSON so that nothing marshaling one for a client leaks it by
/// accident; this is the one place that value is meant to travel.
#[derive(Deserialize)]
struct RoomState_ {
    #[serde(default)]
    seq: i64,
    #[serde(default)]
    comments: Vec<Comment>,
}

fn to_stored(items: &[Comment]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|item| {
                let mut stored = json!(item);
                if !item.author.is_empty() {
                    stored["author"] = json!(item.author);
                }
                stored["replies"] = Value::Array(
                    item.replies
                        .iter()
                        .map(|answer| {
                            let mut reply = json!(answer);
                            if !answer.author.is_empty() {
                                reply["author"] = json!(answer.author);
                            }
                            reply
                        })
                        .collect(),
                );
                stored
            })
            .collect(),
    )
}

/// What a caller is shown: every comment field a client ever sees, plus
/// whether this particular caller may delete it.
#[derive(Serialize)]
pub struct CommentView {
    #[serde(flatten)]
    pub comment: Comment,
    pub deletable: bool,
}

/// Rule H's authorization test: the document's owner may delete anything on
/// it, and everyone else only their own -- and "their own" never matches on
/// two callers who both have no author key, which is what an anonymous caller
/// with no visitor cookie and a nobody's-in-particular seeded example both
/// look like.
pub fn deletable(item: &Comment, author: &str, is_owner: bool) -> bool {
    is_owner || (!author.is_empty() && item.author == author)
}

/// One client frame. Every field is optional; `apply` decides which ones a
/// given type needs.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Message {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub motivation: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub exact: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub position: Option<i64>,
    #[serde(default)]
    pub region: Option<Region>,
    #[serde(default)]
    pub comment_id: String,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub temp_id: String,
    /// A Yjs update, base64-encoded.
    #[serde(default)]
    pub update: String,
    /// A Yjs state vector, base64-encoded: what the sender already has, so the
    /// server can answer with the rest and nothing more.
    #[serde(default)]
    pub vector: String,
    /// The sender's own number for this update, counted up per socket and sent
    /// back on `y-ack` once the update it names is durable. A browser holds
    /// everything above the last acknowledged number and resends it after a
    /// reconnect.
    #[serde(default)]
    pub seq: i64,
    /// Why a checkpoint was asked for: `cli`, `sync`, `restore`, `label`. Only
    /// these four arrive from outside; the rest the server decides for itself.
    #[serde(default)]
    pub why: String,
}

/// What a room sends a connected socket: a text frame, or the order to close.
#[derive(Clone, Debug)]
pub enum Outgoing {
    Text(String),
    Close(&'static str),
}

/// Bounded on purpose. A socket that cannot keep up is disconnected rather
/// than queued for, because an unbounded queue is a way for one slow reader to
/// make the server hold a session's worth of updates per connection. The
/// browser reconnects and asks for what it is missing by state vector, so
/// nothing is lost by hanging up on it.
pub type Sender = mpsc::Sender<Outgoing>;

/// One connected reader: where they are, which the rate limiter counts
/// against, the channel their frames go out on, and -- for an editor -- how
/// far their updates have got towards being durable.
pub struct Peer {
    #[allow(dead_code)]
    pub address: String,
    pub tx: Sender,
    /// Whether this socket may write the document. A reader receives updates
    /// and sends none.
    pub may_edit: bool,
    /// The highest `seq` this socket has sent, and the highest that is durable.
    pub sent: i64,
    pub acked: i64,
    /// Updates this socket has sent in the current minute, and which minute
    /// that is.
    pub minute: i64,
    pub updates: i64,
}

/// The live document, as the server holds it. This is the document, not a
/// draft of it: it is persisted, it outlives every socket, and it is seeded
/// once from the source the document was created with.
pub struct Session {
    pub doc: yrs::Doc,
    /// Whether the document has changed since `sessions/<slug>` was written.
    pub dirty: bool,
    /// When the last update arrived, and who sent it. Both feed the quiet
    /// checkpoint, whose `by` is the editor whose update last landed.
    pub updated_at: i64,
    pub by: String,
    /// A checkpoint asked for from outside and not yet taken, with the reason
    /// it was asked for and who asked. Deferred rather than refused when it
    /// arrives inside `CHECKPOINT_DEFER_SECONDS` of the last one.
    pub asked: Option<(String, String)>,
    pub last_checkpoint_at: i64,
    /// The SHA of the newest checkpoint, so quiet after quiet costs nothing.
    pub last_checkpoint: String,
    /// What the source is written in, which travels with every checkpoint.
    pub format: String,
}

pub struct RoomState {
    pub seq: i64,
    pub comments: Vec<Comment>,
    pub sockets: HashMap<u64, Peer>,
    /// "address:hour" to count.
    pub rate: HashMap<String, i64>,
    pub session: Session,
    /// The manifest as it stands, so a checkpoint does not re-read it and two
    /// checkpoints cannot interleave halfway through one.
    pub manifest: Manifest,
    /// When a socket was last attached or detached, which is what says an idle
    /// room may be evicted.
    pub touched: i64,
}

/// How old this server's claim on a room may get before a write renews it.
/// Comfortably inside the window a lock goes stale in, so a room being
/// written is never a room another server may take.
pub const RENEW_AFTER_SECONDS: i64 = LOCK_STALE_SECONDS / 3;

pub struct Room {
    pub slug: String,
    blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    /// True when another server holds this room's lock: it can be read and
    /// served, but nothing here may write over what that server is doing. Set
    /// when the room is loaded, and again if a renewal ever finds the lock in
    /// somebody else's hands.
    read_only: std::sync::atomic::AtomicBool,
    /// This server's name in the lock, and when it last said so. A lock is
    /// takeable again once it has gone stale, so holding a room in memory for
    /// longer than that without renewing would let a second server take it and
    /// leave both writing the whole comment list over each other.
    holder: String,
    renewed: Mutex<i64>,
    /// The index, for the half of a checkpoint that is bookkeeping: the size a
    /// document's history counts against its owner's quota, and the digest of
    /// the newest checkpoint. Set once, after the store exists, because the
    /// store and the rooms are made in that order.
    store: Arc<std::sync::OnceLock<Arc<crate::store::Store>>>,
    pub state: Mutex<RoomState>,
}

/// What a document update did, which is what the socket handler has to act on.
pub enum Applied {
    /// Apply it and relay it to everyone else.
    Relay,
    /// The socket may not write here, or sent nothing worth relaying.
    Ignored,
    /// Close the socket, with this reason. Either it wrote past the document's
    /// size ceiling or it wrote faster than a person can.
    Refuse(&'static str),
}

pub struct RoomSet {
    /// Who this server is, in the lock objects it takes. A name rather than a
    /// pid, because a pid means nothing to whoever reads the refusal.
    holder: String,
    /// Comments live wherever the documents do. On a bucket that makes the
    /// server genuinely stateless.
    pub blobs: Arc<dyn BlobStore>,
    config: Arc<Configuration>,
    rooms: Mutex<HashMap<String, Arc<Room>>>,
    store: Arc<std::sync::OnceLock<Arc<crate::store::Store>>>,
}

/// Names this process in a room lock: the machine it runs on and its pid,
/// which is enough to tell two of them apart and to tell a restart from a
/// second server.
fn this_server() -> String {
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "server".to_string());
    format!("{host}/{}", std::process::id())
}

impl RoomSet {
    pub fn new(blobs: Arc<dyn BlobStore>, config: Arc<Configuration>) -> RoomSet {
        RoomSet {
            holder: this_server(),
            blobs,
            config,
            rooms: Mutex::new(HashMap::new()),
            store: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Hands the rooms the index. Called once, by `Server::new`, because a
    /// checkpoint has to record its own size against the document's quota and
    /// the store is what holds that.
    pub fn attach_store(&self, store: Arc<crate::store::Store>) {
        let _ = self.store.set(store);
    }

    pub async fn get(&self, slug: &str) -> Arc<Room> {
        let mut rooms = self.rooms.lock().await;
        if let Some(existing) = rooms.get(slug) {
            return existing.clone();
        }
        // A room nobody has open, held past the ceiling, is written out and
        // let go before another is made. Done here rather than on a timer so
        // the bound is on the map itself and cannot be outrun by arrivals.
        if rooms.len() >= self.config.session.rooms_max {
            evict_idle(&mut rooms, self.config.session.rooms_max).await;
        }
        // Taken before anything is read, so a second server writing the same
        // bucket finds out it is second rather than interleaving its comments
        // with the first one's.
        let (held, by) = take_room_lock(self.blobs.as_ref(), slug, &self.holder).await;
        if !held {
            eprintln!("warning: {slug} is being written by {by}; comments here are read-only in this server");
        }
        let room = Arc::new(Room {
            slug: slug.to_string(),
            blobs: self.blobs.clone(),
            config: self.config.clone(),
            read_only: std::sync::atomic::AtomicBool::new(!held),
            holder: self.holder.clone(),
            renewed: Mutex::new(now_unix()),
            store: self.store.clone(),
            state: Mutex::new(RoomState {
                seq: 0,
                comments: Vec::new(),
                sockets: HashMap::new(),
                rate: HashMap::new(),
                session: Session {
                    doc: session::new_doc(),
                    dirty: false,
                    updated_at: 0,
                    by: String::new(),
                    asked: None,
                    last_checkpoint_at: 0,
                    last_checkpoint: String::new(),
                    format: String::new(),
                },
                manifest: Manifest::default(),
                touched: now_unix(),
            }),
        });
        room.load().await;
        rooms.insert(slug.to_string(), room.clone());
        room
    }

    /// Drops a document's comments and disconnects anyone still reading it.
    /// Reached only through the delete route, which checks ownership first.
    pub async fn purge(&self, slug: &str) {
        let room = self.get(slug).await;
        {
            let mut state = room.state.lock().await;
            state.comments.clear();
            state.seq = 0;
            for peer in state.sockets.values() {
                let _ = peer.tx.try_send(Outgoing::Close("document deleted"));
            }
        }
        let _ = self
            .blobs
            .delete(&[room_key(slug), room_lock_key(slug), session_key(slug)])
            .await;
        // The history goes with the document, which is what destroy has
        // promised in the README since before there was a history to delete.
        if let Ok(found) = self.blobs.list(&crate::blob::history_prefix(slug)).await {
            let keys: Vec<String> = found.into_iter().map(|object| object.key).collect();
            if !keys.is_empty() {
                let _ = self.blobs.delete(&keys).await;
            }
        }
        self.rooms.lock().await.remove(slug);
    }

    /// One pass over every open room: write the ones that have gone quiet,
    /// checkpoint the ones that have been quiet long enough to be worth a
    /// mark, and let go of the ones nobody has open. Run on a timer by
    /// `serve`, so a document nobody is watching is still persisted and still
    /// gets its checkpoints -- which is the whole difference between a session
    /// the server holds and a session it relays.
    pub async fn sweep(&self) {
        let open: Vec<(String, Arc<Room>)> = {
            let rooms = self.rooms.lock().await;
            rooms
                .iter()
                .map(|(slug, room)| (slug.clone(), room.clone()))
                .collect()
        };
        let mut idle = Vec::new();
        for (slug, room) in open {
            if room.tick().await {
                idle.push(slug);
            }
        }
        // Kept for a while after the last socket closes: a reader who
        // reloads the page should not pay for a state transfer from storage.
        // The ceiling in `get` is what bounds the map; this only trims it.
        let mut rooms = self.rooms.lock().await;
        if rooms.len() > self.config.session.rooms_max / 2 {
            let now = now_unix();
            for slug in idle {
                let stale = match rooms.get(&slug) {
                    Some(room) => {
                        let state = room.state.lock().await;
                        state.sockets.is_empty() && !state.session.dirty && now - state.touched > 60
                    }
                    None => false,
                };
                if stale {
                    rooms.remove(&slug);
                }
            }
        }
    }

    /// How many documents this server is holding in memory, which is the
    /// number the ceiling is on.
    #[allow(dead_code)] // asked by the tests that check the ceiling holds
    pub async fn open_count(&self) -> usize {
        self.rooms.lock().await.len()
    }

    /// Writes out every open room, for a server on its way down.
    pub async fn flush(&self) {
        let open: Vec<Arc<Room>> = self.rooms.lock().await.values().cloned().collect();
        for room in open {
            if let Err(err) = room.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    room.slug
                );
            }
        }
    }
}

impl Room {
    async fn load(&self) {
        if let Ok(raw) = self.blobs.get(&room_key(&self.slug)).await {
            if let Ok(stored) = serde_json::from_slice::<RoomState_>(&raw) {
                let mut state = self.state.lock().await;
                state.seq = stored.seq;
                state.comments = stored.comments;
                state.comments.sort_by_key(|item| item.seq);
            }
        }
        self.load_session().await;
    }

    /// Brings the document back: the persisted session state if there is one,
    /// and otherwise the source the document was published with, which is the
    /// one time a session is ever seeded.
    ///
    /// This is also the whole of the migration. A document stored the old way
    /// has no `sessions/<slug>` and does have a source -- or, published as
    /// HTML, has the page itself -- and is seeded from it the first time
    /// anybody opens it. Nothing is rewritten until then, so a deployment that
    /// is rolled back loses nothing.
    async fn load_session(&self) {
        let manifest = history::load(self.blobs.as_ref(), &self.slug)
            .await
            .unwrap_or_default();
        let entry = match self.store.get() {
            Some(store) => store.get(&self.slug).await,
            None => None,
        };
        let format = entry
            .as_ref()
            .map(|entry| {
                if entry.source_format.is_empty() {
                    "html".to_string()
                } else {
                    entry.source_format.clone()
                }
            })
            .unwrap_or_default();

        let stored = match self.blobs.get(&session_key(&self.slug)).await {
            Ok(raw) => Some(raw),
            Err(BlobError::NotFound) => None,
            Err(err) => {
                // Storage that cannot be read is not storage to seed over: a
                // session seeded from the published source on top of a state
                // that is merely unreachable would show the document twice.
                eprintln!(
                    "warning: could not read the session for {}: {err}",
                    self.slug
                );
                let mut state = self.state.lock().await;
                state.manifest = manifest;
                state.session.format = format;
                return;
            }
        };

        let seed = match &stored {
            Some(_) => None,
            None => self.published_source(entry.as_ref()).await,
        };

        let mut state = self.state.lock().await;
        state.manifest = manifest;
        state.session.format = format;
        if let Some(point) = state.manifest.latest() {
            state.session.last_checkpoint = point.sha.clone();
        }
        match stored {
            Some(raw) => {
                if let Err(err) = session::apply_update(&state.session.doc, &raw) {
                    eprintln!(
                        "warning: the session for {} is unreadable ({err})",
                        self.slug
                    );
                }
            }
            None => {
                if let Some((source, format)) = seed {
                    state.session.format = format;
                    session::replace_text(&state.session.doc, &source);
                    // Seeded, not edited: what is in the document is what
                    // storage already says, so there is nothing to write back
                    // until somebody types.
                    state.session.dirty = false;
                }
            }
        }
    }

    /// The bytes a session is seeded from, for a document that has one and no
    /// session yet, and the format they are in. A document published as HTML
    /// is its own source.
    ///
    /// A document whose source is gone -- the old layout kept one only for the
    /// version the index named -- falls back to its stored page, and says so:
    /// the format becomes `html`, because that is what the bytes are. Seeding
    /// markdown's slot with HTML and still calling it markdown is how a
    /// migrated document would come back rendered twice.
    async fn published_source(
        &self,
        entry: Option<&crate::store::IndexEntry>,
    ) -> Option<(String, String)> {
        let store = self.store.get()?;
        let entry = entry?;
        if !entry.source_format.is_empty() && entry.source_format != "html" {
            if let Ok(raw) = store.read_source(&self.slug).await {
                return Some((
                    String::from_utf8_lossy(&raw).to_string(),
                    entry.source_format.clone(),
                ));
            }
            eprintln!(
                "warning: {} has no stored {} source; it opens as the page it was published as",
                self.slug, entry.source_format
            );
        }
        let raw = store.read(&self.slug, &entry.sha).await.ok()?;
        Some((
            String::from_utf8_lossy(&raw).to_string(),
            "html".to_string(),
        ))
    }

    /// Rewrites the whole room. Comment volume per document is in the dozens,
    /// so this stays cheaper than any incremental scheme. The in-memory copy
    /// is the source of truth while anyone is connected; writing goes wherever
    /// the documents go, so the server holds nothing that only it has.
    /// Whether another server holds this room, as of the last time we asked.
    pub fn read_only(&self) -> bool {
        self.read_only.load(Ordering::Relaxed)
    }

    /// Says whether this server may still write the room, renewing the lock
    /// when it is old enough to be worth saying so again. Renewal is on the
    /// write path rather than on a timer: a room nobody is writing does not
    /// need holding, and a room being written is asked about often enough.
    ///
    /// A renewal that finds somebody else in the lock makes the room read-only
    /// for good. Their copy is the live one, and continuing to write ours
    /// would put half of each thread in the stored list.
    async fn hold(&self) -> bool {
        if self.read_only() {
            return false;
        }
        let now = now_unix();
        let mut renewed = self.renewed.lock().await;
        if now - *renewed < RENEW_AFTER_SECONDS {
            return true;
        }
        let (held, by) = take_room_lock(self.blobs.as_ref(), &self.slug, &self.holder).await;
        if !held {
            eprintln!(
                "warning: {} was taken by {by}; comments here are read-only from now on",
                self.slug
            );
            self.read_only.store(true, Ordering::Relaxed);
            return false;
        }
        *renewed = now;
        true
    }

    pub async fn save(&self, state: &RoomState) -> Result<(), String> {
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let raw = json!({"seq": state.seq, "comments": to_stored(&state.comments)});
        let body = serde_json::to_vec(&raw).map_err(|err| err.to_string())?;
        self.blobs
            .put(&room_key(&self.slug), body, "application/json")
            .await
            .map_err(|err| err.to_string())
    }

    /// Every comment, for seeding and for the tests that read a room back.
    #[allow(dead_code)]
    pub async fn snapshot(&self) -> Vec<Comment> {
        self.state.lock().await.comments.clone()
    }

    /// The per-caller view of the whole thread: the hello frame and the REST
    /// listing both need this, since deletable differs by who is asking.
    pub async fn snapshot_for(&self, author: &str, is_owner: bool) -> Vec<CommentView> {
        let state = self.state.lock().await;
        state
            .comments
            .iter()
            .map(|item| CommentView {
                comment: item.clone(),
                deletable: deletable(item, author, is_owner),
            })
            .collect()
    }

    /// (total, open)
    pub async fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().await;
        let open = state.comments.iter().filter(|item| !item.resolved).count();
        (state.comments.len(), open)
    }

    pub async fn attach(&self, id: u64, address: String, tx: Sender, may_edit: bool) {
        let mut state = self.state.lock().await;
        state.touched = now_unix();
        state.sockets.insert(
            id,
            Peer {
                address,
                tx,
                may_edit,
                sent: 0,
                acked: 0,
                minute: 0,
                updates: 0,
            },
        );
    }

    pub async fn detach(&self, id: u64) {
        let mut state = self.state.lock().await;
        state.sockets.remove(&id);
        state.touched = now_unix();
    }

    /// How many editors are still connected, which is what says whether the
    /// last one has just left.
    pub async fn editors_connected(&self) -> usize {
        self.state
            .lock()
            .await
            .sockets
            .values()
            .filter(|peer| peer.may_edit)
            .count()
    }

    pub async fn broadcast(&self, payload: &Value) {
        self.broadcast_except(None, payload).await;
    }

    /// Sends to everyone but one socket: an editing update is relayed to the
    /// others, and the one that sent it already has it.
    pub async fn broadcast_except(&self, skip: Option<u64>, payload: &Value) {
        let message = payload.to_string();
        let mut state = self.state.lock().await;
        send_to_all(&mut state, skip, &message);
    }

    /// Counts writes per address per hour, and forgets older hours as it goes
    /// rather than accumulating an entry per address per hour.
    fn rate_ok(&self, state: &mut RoomState, address: &str) -> bool {
        if address.is_empty() {
            return true;
        }
        let hour = now_unix() / 3600;
        let key = format!("{}:{hour}", rate_key(address));
        let suffix = format!(":{hour}");
        state.rate.retain(|existing, _| existing.ends_with(&suffix));
        let count = state.rate.get(&key).copied().unwrap_or(0);
        if count + 1 > self.config.rate_per_hour {
            return false;
        }
        state.rate.insert(key, count + 1);
        true
    }

    /// Validates, persists, and returns the event to broadcast. The second
    /// result is false when the event is an error, which goes only to its
    /// sender. `author` is the caller's own author key, and `is_owner` says
    /// whether the caller owns the document this room belongs to; both come
    /// from the caller's identity and are never taken from the message itself.
    pub async fn apply(
        &self,
        incoming: Message,
        address: &str,
        author: &str,
        is_owner: bool,
    ) -> (Value, bool) {
        let mut state = self.state.lock().await;
        let config = self.config.clone();

        let fail = |text: &str| -> (Value, bool) {
            let mut payload =
                json!({"type": "error", "message": text, "temp_id": incoming.temp_id});
            // Named so the reader knows which optimistic row to roll back.
            if !incoming.comment_id.is_empty() {
                payload["comment_id"] = json!(incoming.comment_id);
            }
            (payload, false)
        };
        const UNSAVED: &str = "could not save that comment; try again";

        // Resolving and deleting cost a slot too, the same as posting: a
        // caller who could resolve or delete without limit could still make a
        // thread unusable, just by different means than flooding it with text.
        if !self.rate_ok(&mut state, address) {
            return fail("too many comments from this address; try later");
        }

        if incoming.kind == "resolve" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            let (was_resolved, was_resolved_at) = (
                state.comments[index].resolved,
                state.comments[index].resolved_at.clone(),
            );
            state.comments[index].resolved = incoming.resolved;
            state.comments[index].resolved_at = incoming.resolved.then(timestamp);
            if self.save(&state).await.is_err() {
                state.comments[index].resolved = was_resolved;
                state.comments[index].resolved_at = was_resolved_at;
                return fail(UNSAVED);
            }
            let target = &state.comments[index];
            return (
                json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                }),
                true,
            );
        }

        if incoming.kind == "delete" {
            let Some(index) = state
                .comments
                .iter()
                .position(|item| item.id == incoming.comment_id)
            else {
                return fail("unknown comment");
            };
            if !deletable(&state.comments[index], author, is_owner) {
                return fail("you may only delete your own comments");
            }
            let removed = state.comments.remove(index);
            if self.save(&state).await.is_err() {
                state.comments.insert(index, removed);
                return fail(UNSAVED);
            }
            return (
                json!({"type": "delete", "comment_id": incoming.comment_id}),
                true,
            );
        }

        let body = clean(&incoming.body, config.caps.body).trim().to_string();
        let motivation = config.allowed_motivation(&incoming.motivation);
        // A highlight is the passage itself: marking something as worth
        // returning to needs no words. Everything else is a remark, and a
        // remark with no words is nothing.
        if body.is_empty() && !(incoming.kind == "comment" && motivation == "highlighting") {
            return fail("comment body is required");
        }
        let mut creator = clean(&incoming.creator, config.caps.creator)
            .trim()
            .to_string();
        if creator.is_empty() {
            creator = "Anonymous".to_string();
        }

        match incoming.kind.as_str() {
            "reply" => {
                let Some(index) = state
                    .comments
                    .iter()
                    .position(|item| item.id == incoming.comment_id)
                else {
                    return fail("unknown comment");
                };
                if state.comments[index].replies.len() >= config.max_replies {
                    return fail("this comment has reached its reply limit");
                }
                let added = Reply {
                    id: new_id(),
                    body,
                    creator,
                    created: timestamp(),
                    author: author.to_string(),
                };
                state.comments[index].replies.push(added.clone());
                if self.save(&state).await.is_err() {
                    state.comments[index].replies.pop();
                    return fail(UNSAVED);
                }
                (
                    json!({
                        "type": "reply", "comment_id": state.comments[index].id,
                        "reply": added, "temp_id": incoming.temp_id,
                    }),
                    true,
                )
            }
            "comment" => {
                if state.comments.len() >= config.max_comments {
                    return fail("this document has reached its comment limit");
                }
                let exact = clean(&incoming.exact, config.caps.exact).trim().to_string();
                let spot = valid_region(incoming.region.as_ref());
                // An annotation is anchored to words or to part of a figure;
                // one or the other, never neither.
                if exact.is_empty() && spot.is_none() {
                    return fail("select some text or part of a figure to comment on");
                }
                state.seq += 1;
                // The selector is the durable anchor. Offsets are recomputed in
                // the reader against whatever version of the document is on
                // screen, so replacing a document needs no migration pass here.
                let added = Comment {
                    id: new_id(),
                    seq: state.seq,
                    motivation,
                    exact,
                    prefix: clean(&incoming.prefix, config.caps.context),
                    suffix: clean(&incoming.suffix, config.caps.context),
                    position: incoming.position.filter(|p| *p >= 0),
                    region: spot,
                    body,
                    tags: clean_tags(&incoming.tags, &config),
                    creator,
                    created: timestamp(),
                    resolved: false,
                    resolved_at: None,
                    replies: Vec::new(),
                    author: author.to_string(),
                };
                state.comments.push(added.clone());
                if self.save(&state).await.is_err() {
                    state.comments.pop();
                    state.seq -= 1;
                    return fail(UNSAVED);
                }
                (
                    json!({"type": "comment", "comment": added, "temp_id": incoming.temp_id}),
                    true,
                )
            }
            _ => fail("unknown message type"),
        }
    }

    /* ---------------------------------------------------------- the document */

    /// The source as it stands, which is what a checkpoint is made of and what
    /// the command line reads.
    pub async fn source(&self) -> String {
        let state = self.state.lock().await;
        session::text_of(&state.session.doc)
    }

    pub async fn format(&self) -> String {
        self.state.lock().await.session.format.clone()
    }

    /// Writes a source into the live document, as an edit rather than as a
    /// substitution: the common prefix and suffix are left alone, so an editor
    /// typing elsewhere at that moment keeps their words and their caret and
    /// sees the rest change under them. This is how a command-line publish, a
    /// `sync` write and a restore all reach the document.
    ///
    /// Returns the update to relay, which is what the sockets are sent.
    pub async fn set_source(&self, source: &str, format: &str) -> Vec<u8> {
        let mut state = self.state.lock().await;
        let before = session::encode_vector(&state.session.doc);
        session::replace_text(&state.session.doc, source);
        if !format.is_empty() {
            state.session.format = format.to_string();
        }
        state.session.dirty = true;
        state.session.updated_at = now_unix();
        session::encode_diff(&state.session.doc, &before)
            .unwrap_or_else(|_| session::encode_state(&state.session.doc))
    }

    /// What a socket is answered with on `y-open`: everything the document
    /// holds, or -- when the socket says what it already has -- only the rest.
    /// The second result is how many people are here.
    pub async fn open_state(&self, vector: Option<&[u8]>) -> (Vec<u8>, usize) {
        let state = self.state.lock().await;
        let update = match vector {
            Some(raw) => session::encode_diff(&state.session.doc, raw)
                .unwrap_or_else(|_| session::encode_state(&state.session.doc)),
            None => session::encode_state(&state.session.doc),
        };
        (update, state.sockets.len())
    }

    /// Applies one update from an editor. The document is the server's, so an
    /// update is applied here before it is relayed, and what is relayed is
    /// what was applied.
    ///
    /// The size ceiling is enforced on the text rather than on the update: an
    /// update that carries the source past `max_html` is trimmed back at once
    /// and the socket that sent it is closed, so the room's text never exceeds
    /// what a publish could have sent and no number of concurrent writers can
    /// talk their way past the quota between them.
    pub async fn receive_update(&self, socket: u64, update: &[u8], seq: i64, by: &str) -> Applied {
        let mut state = self.state.lock().await;
        let now = now_unix();
        {
            let minute = now / 60;
            let Some(peer) = state.sockets.get_mut(&socket) else {
                return Applied::Ignored;
            };
            if !peer.may_edit {
                return Applied::Ignored;
            }
            if peer.minute != minute {
                peer.minute = minute;
                peer.updates = 0;
            }
            peer.updates += 1;
            if peer.updates > self.config.session.updates_per_minute {
                return Applied::Refuse("too many updates");
            }
            if seq > peer.sent {
                peer.sent = seq;
            }
        }
        if session::apply_update(&state.session.doc, update).is_err() {
            return Applied::Ignored;
        }
        state.session.dirty = true;
        state.session.updated_at = now;
        state.session.by = by.to_string();

        let length = session::text_of(&state.session.doc).len();
        if length > self.config.max_html {
            let trimmed = session::truncate_to(&state.session.doc, self.config.max_html);
            if trimmed {
                // The correction is a change like any other and goes to
                // everybody, including the socket about to be closed.
                let whole = session::encode_state(&state.session.doc);
                let payload =
                    json!({"type": "y-update", "update": encode_update(&whole)}).to_string();
                send_to_all(&mut state, None, &payload);
            }
            return Applied::Refuse("this document has reached its size limit");
        }
        Applied::Relay
    }

    /// Writes `sessions/<slug>` when the document has changed, and only then
    /// tells the sockets their updates are durable. Relaying an update is not
    /// an acknowledgment: nothing here says "saved" until storage has said so.
    pub async fn persist(&self) -> Result<bool, String> {
        let mut state = self.state.lock().await;
        if !state.session.dirty {
            return Ok(false);
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let body = session::encode_state(&state.session.doc);
        let size = body.len() as i64;
        let durable: Vec<(u64, i64)> = state
            .sockets
            .iter()
            .map(|(id, peer)| (*id, peer.sent))
            .collect();
        self.blobs
            .put(&session_key(&self.slug), body, "application/octet-stream")
            .await
            .map_err(|err| err.to_string())?;
        state.session.dirty = false;
        for (id, seq) in durable {
            let Some(peer) = state.sockets.get_mut(&id) else {
                continue;
            };
            if seq <= peer.acked {
                continue;
            }
            peer.acked = seq;
            let payload = json!({"type": "y-ack", "seq": seq}).to_string();
            if peer.tx.try_send(Outgoing::Text(payload)).is_err() {
                state.sockets.remove(&id);
            }
        }
        let history = state.manifest.bytes();
        drop(state);
        self.record_size(size + history, None).await;
        Ok(true)
    }

    /// Takes a checkpoint, if the text differs from the last one. Returns the
    /// SHA of the checkpoint that now stands for the current text, or None
    /// when the request was deferred.
    ///
    /// The order of writes is the one `01-SPEC-history.md` sets, so that a
    /// crash leaves nothing worse than an untidy history: the checkpoint
    /// object, then the session state, then the index entry, then the
    /// manifest. A manifest missing its newest entry is repaired by the next
    /// checkpoint, which finds the object present and names it as `parent` --
    /// `repair` below is that.
    pub async fn checkpoint(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        let now = now_unix();
        let (source, format, last, deferred) = {
            let mut state = self.state.lock().await;
            // A deliberate write inside the defer window is not refused; it
            // waits, and is taken when the window passes, if the text still
            // differs. A burst of saves is one mark in the timeline.
            let deferrable = matches!(why, "cli" | "sync" | "restore" | "label");
            if deferrable
                && state.session.last_checkpoint_at > 0
                && now - state.session.last_checkpoint_at < CHECKPOINT_DEFER_SECONDS
            {
                state.session.asked = Some((why.to_string(), by.to_string()));
                (String::new(), String::new(), String::new(), true)
            } else {
                (
                    session::text_of(&state.session.doc),
                    state.session.format.clone(),
                    state.session.last_checkpoint.clone(),
                    false,
                )
            }
        };
        if deferred {
            return Ok(None);
        }

        let sha = crate::store::digest_of(&source);
        // Quiet after quiet costs nothing: the same text is the same
        // checkpoint, and a checkpoint already in the manifest is not written
        // again and adds no entry.
        {
            let mut state = self.state.lock().await;
            state.session.asked = None;
            if state.manifest.has(&sha) {
                state.session.last_checkpoint = sha.clone();
                state.session.last_checkpoint_at = now;
                return Ok(Some(sha));
            }
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }

        // 1. the checkpoint object.
        self.blobs
            .put(
                &checkpoint_key(&self.slug, &sha),
                source.clone().into_bytes(),
                "text/plain; charset=utf-8",
            )
            .await
            .map_err(|err| err.to_string())?;

        // 2. the session state, so a restart comes back at or after the
        //    checkpoint rather than before it.
        let state_bytes = {
            let state = self.state.lock().await;
            session::encode_state(&state.session.doc)
        };
        let session_size = state_bytes.len() as i64;
        self.blobs
            .put(
                &session_key(&self.slug),
                state_bytes,
                "application/octet-stream",
            )
            .await
            .map_err(|err| err.to_string())?;

        // The manifest, in memory: the repair first, then this checkpoint.
        let (manifest, shed, size) = {
            let mut state = self.state.lock().await;
            state.session.dirty = false;
            self.repair(&mut state, &last).await;
            let parent = state
                .manifest
                .latest()
                .map(|point| point.sha.clone())
                .unwrap_or_default();
            state.manifest.checkpoints.push(Checkpoint {
                sha: sha.clone(),
                parent,
                at: timestamp(),
                by: by.to_string(),
                why: why.to_string(),
                source_format: format,
                size: source.len() as i64,
                label: String::new(),
                commit: String::new(),
                dirty: false,
            });
            state.session.last_checkpoint = sha.clone();
            state.session.last_checkpoint_at = now;
            (state.manifest.clone(), Vec::<String>::new(), session_size)
        };
        let mut manifest = manifest;
        let mut shed = shed;

        // A checkpoint is never refused, because refusing it would lose work.
        // What gives instead is the oldest history: the ceilings shed the
        // oldest unlabelled checkpoints, and the oldest labelled ones after
        // them, until the document fits.
        let ceiling = self.allowance(size).await;
        let keep_count = self.config.session.history_max;
        shed.extend(manifest.shed(|manifest| {
            (keep_count == 0 || manifest.checkpoints.len() <= keep_count)
                && (ceiling < 0 || manifest.bytes() <= ceiling)
        }));

        // 3. the index entry, then 4. the manifest.
        self.record_size(size + manifest.bytes(), Some(&sha)).await;
        history::save(self.blobs.as_ref(), &self.slug, &manifest).await?;
        {
            let mut state = self.state.lock().await;
            state.manifest = manifest;
        }
        if !shed.is_empty() {
            let keys: Vec<String> = shed
                .iter()
                .map(|sha| checkpoint_key(&self.slug, sha))
                .collect();
            let _ = self.blobs.delete(&keys).await;
        }
        // The migration's one and only cleanup. A document stored the old way
        // has a rendered page and a source under the old keys; both are copies
        // of what is now a checkpoint, and this is the first moment at which
        // that is true. Before this point nothing has been removed, so a
        // deployment rolled back before its first checkpoint loses nothing.
        if let Some(store) = self.store.get() {
            store.drop_derived(&self.slug).await;
        }
        Ok(Some(sha))
    }

    /// Puts back a checkpoint the manifest lost. The index names the newest
    /// checkpoint, so an index entry naming a SHA the manifest does not have,
    /// whose object is still there, is a write that got as far as step 3 and
    /// no further. It is added as the newest entry rather than guessed at.
    async fn repair(&self, state: &mut RoomState, last: &str) {
        if last.is_empty() || state.manifest.has(last) {
            return;
        }
        if self
            .blobs
            .get(&checkpoint_key(&self.slug, last))
            .await
            .is_err()
        {
            return;
        }
        let size = self
            .blobs
            .get(&checkpoint_key(&self.slug, last))
            .await
            .map(|raw| raw.len() as i64)
            .unwrap_or(0);
        let parent = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();
        state.manifest.checkpoints.push(Checkpoint {
            sha: last.to_string(),
            parent,
            at: timestamp(),
            by: String::new(),
            why: "recovered".to_string(),
            source_format: state.session.format.clone(),
            size,
            label: String::new(),
            commit: String::new(),
            dirty: false,
        });
    }

    /// The most this document's session and history may occupy before it
    /// carries its owner or the deployment over a ceiling, less what the
    /// session state already costs. Negative means "no ceiling here", which is
    /// what a document with no index entry gets.
    async fn allowance(&self, session_size: i64) -> i64 {
        let Some(store) = self.store.get() else {
            return -1;
        };
        match store.room_for(&self.slug).await {
            Some(room) => room - session_size,
            None => -1,
        }
    }

    /// Records what this document now costs, and the checkpoint the index
    /// names. A failure here is a failure of the checkpoint's bookkeeping, not
    /// of the checkpoint: the object and the state are already written, and
    /// the next checkpoint repairs the entry.
    async fn record_size(&self, size: i64, sha: Option<&str>) {
        let Some(store) = self.store.get() else {
            return;
        };
        let format = self.state.lock().await.session.format.clone();
        if let Err(err) = store.record_history(&self.slug, sha, size, &format).await {
            eprintln!(
                "warning: could not record the history of {}: {err}",
                self.slug
            );
        }
    }

    /// The manifest, for the timeline and for the tests.
    #[allow(dead_code)] // the timeline that reads it is step 5
    pub async fn manifest(&self) -> Manifest {
        self.state.lock().await.manifest.clone()
    }

    /// What the sweeper does to one room, once a second: write the document if
    /// it has been quiet long enough to be worth writing, and take a
    /// checkpoint if it has been quiet long enough to be worth a mark.
    ///
    /// Returns whether the room is idle and durable, which is what says it may
    /// be let go of.
    pub async fn tick(&self) -> bool {
        let limits = self.config.session;
        let now = now_unix();
        let (dirty, quiet_for, asked, sockets, since_checkpoint, differs) = {
            let state = self.state.lock().await;
            let text = session::text_of(&state.session.doc);
            (
                state.session.dirty,
                now - state.session.updated_at,
                state.session.asked.clone(),
                state.sockets.len(),
                now - state.session.last_checkpoint_at,
                crate::store::digest_of(&text) != state.session.last_checkpoint,
            )
        };
        if dirty && quiet_for >= limits.write_after_seconds {
            if let Err(err) = self.persist().await {
                eprintln!(
                    "warning: could not write the session for {}: {err}",
                    self.slug
                );
                return false;
            }
        }
        // A deferred request comes due once the window has passed.
        if let Some((why, by)) = asked {
            if since_checkpoint >= CHECKPOINT_DEFER_SECONDS {
                let _ = self.checkpoint(&why, &by).await;
                return false;
            }
        }
        if differs && quiet_for >= limits.checkpoint_seconds {
            let by = self.state.lock().await.session.by.clone();
            let _ = self.checkpoint("quiet", &by).await;
            return false;
        }
        sockets == 0 && !dirty
    }

    /// How many people have this document open, which is worth showing them.
    pub async fn editors(&self) -> usize {
        self.state.lock().await.sockets.len()
    }
}

/// Base64, which is how a binary update travels on a JSON socket.
pub fn encode_update(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn decode_update(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}

/// Sends to every socket but one, dropping any that has fallen too far behind
/// to take another frame. A dropped socket is not a lost edit: the browser
/// reconnects and asks for what it is missing by state vector.
fn send_to_all(state: &mut RoomState, skip: Option<u64>, message: &str) {
    let mut behind = Vec::new();
    for (id, peer) in &state.sockets {
        if Some(*id) == skip {
            continue;
        }
        if peer
            .tx
            .try_send(Outgoing::Text(message.to_string()))
            .is_err()
        {
            behind.push(*id);
        }
    }
    for id in behind {
        state.sockets.remove(&id);
    }
}

/// Lets go of the rooms nobody has open, oldest first, until the map is under
/// its ceiling again. A room is only let go of once its document is durable;
/// one with unwritten changes is kept however long it has been idle, because
/// forgetting it would be losing work.
async fn evict_idle(rooms: &mut HashMap<String, Arc<Room>>, ceiling: usize) {
    let mut idle: Vec<(i64, String)> = Vec::new();
    for (slug, room) in rooms.iter() {
        let state = room.state.lock().await;
        if state.sockets.is_empty() && !state.session.dirty {
            idle.push((state.touched, slug.clone()));
        }
    }
    idle.sort();
    for (_, slug) in idle {
        if rooms.len() < ceiling {
            break;
        }
        rooms.remove(&slug);
    }
}

/// What the rate limiter actually counts against: an IPv4 address used whole,
/// or an IPv6 address reduced to its /64 -- the block an ISP typically hands
/// one customer -- so a rotating address within that prefix does not buy a
/// fresh limit. A value that does not parse as an address is used as given.
pub fn rate_key(address: &str) -> String {
    let Ok(ip) = address.parse::<IpAddr>() else {
        return address.to_string();
    };
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.to_string();
            }
            let segments = v6.segments();
            format!(
                "{:x}:{:x}:{:x}:{:x}",
                segments[0], segments[1], segments[2], segments[3]
            )
        }
    }
}

/// Normalises labels: lowercased, trimmed, deduplicated, capped in both length
/// and number, so filtering by one of them is predictable.
pub fn clean_tags(tags: &[String], config: &Configuration) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let label = clean(tag, config.caps.tag).trim().to_lowercase();
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        if label.is_empty() || out.contains(&label) {
            continue;
        }
        out.push(label);
        if out.len() == config.max_tags {
            break;
        }
    }
    out
}

/// Keeps a rectangle only if it is one: inside the image, with a size worth
/// drawing. Percentages, so it holds at any display width.
pub fn valid_region(spot: Option<&Region>) -> Option<Region> {
    let spot = spot?;
    let inside = |v: f64| (0.0..=100.0).contains(&v);
    if !inside(spot.x) || !inside(spot.y) || !inside(spot.width) || !inside(spot.height) {
        return None;
    }
    if spot.width < 0.5
        || spot.height < 0.5
        || spot.x + spot.width > 100.5
        || spot.y + spot.height > 100.5
    {
        return None;
    }
    if spot.image_index < 0 {
        return None;
    }
    Some(Region {
        image_digest: clean(&spot.image_digest, 64),
        image_index: spot.image_index,
        x: spot.x,
        y: spot.y,
        width: spot.width,
        height: spot.height,
    })
}
