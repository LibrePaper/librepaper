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
    checkpoint_key, room_key, room_lock_key, session_key, take_room_lease, BlobError, BlobStore,
    BlobVersion, Lease, LOCK_STALE_SECONDS,
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
    /// The checkpoint this comment was made on: what the reviewer was actually
    /// looking at. Set by the server, never by the client, from the checkpoint
    /// taken the moment the comment arrived -- so a passage can be looked up
    /// in the text as it was rather than reconstructed from one that has moved
    /// on. Empty on a comment from before the field existed, which is read as
    /// the oldest checkpoint the manifest still has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    /// The checkpoint current when it was resolved, beside `resolved_at`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_in: String,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// Who actually posted this comment: "github:<login>" for a signed-in
    /// caller, "visitor:<sha256 of the visitor token>" for a verified
    /// anonymous browser, or "" for neither (including every seeded example,
    /// which belongs to nobody in particular). Kept out of every client-bound
    /// shape for the same reason as `Reply::author`.
    #[serde(default, skip_serializing)]
    pub author: String,
    /// The digest of the link this comment arrived on, empty for a commenter
    /// by name. It is what lets an owner group a blind reviewer's remarks
    /// without either reviewer having signed anything, and it is not exported:
    /// the export is the reviewer's words, not the mechanics of how they
    /// arrived. Kept off every client-bound shape, as `author` is.
    #[serde(default, skip_serializing)]
    pub via: String,
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
                if !item.via.is_empty() {
                    stored["via"] = json!(item.via);
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
    /// The tree the last checkpoint recorded, so the next one can say which
    /// paths moved without reading it back. Empty on a cold room, and filled
    /// from storage by the first checkpoint that needs it.
    pub last_tree: Option<crate::history::Tree>,
    /// The text digests this server has already written under
    /// `history/<slug>/blobs/`. A chapter untouched between twenty
    /// checkpoints is written once, and a room that was evicted and brought
    /// back writes each of its texts once more -- the same bytes to the same
    /// key, which costs a write and changes nothing.
    pub blobs_written: std::collections::HashSet<String>,
    /// Every asset this document holds, by digest, and what it costs. Read
    /// once when the room is loaded and added to by each upload, because a
    /// checkpoint has to record what a figure weighs and the shared document
    /// carries only its name.
    pub asset_sizes: HashMap<String, i64>,
    /// When each asset was written here, for the grace period. Uploading a
    /// figure and naming it are two requests, and an asset pruned in between
    /// is one somebody had just successfully uploaded. Held in memory only:
    /// the gap it covers is seconds, and a server that restarted in the middle
    /// of it has lost the upload anyway.
    pub asset_written_at: HashMap<String, i64>,
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
    /// The versions this server last saw of the three objects a room owns.
    /// Every write of them is conditional on these, so a write that loses is
    /// proof that another process owns the room -- which is what makes the
    /// lease enforced rather than advisory. Empty means "there was nothing
    /// there", which is how a document with no session or no comments starts.
    pub session_version: BlobVersion,
    pub manifest_version: BlobVersion,
    pub comments_version: BlobVersion,
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
    /// This server's name in the lease, and the lease it holds. A lease is
    /// takeable again once it has gone stale, so holding a room in memory for
    /// longer than that without renewing would let a second server take it and
    /// leave both writing the whole document over each other.
    holder: String,
    lease: Mutex<Lease>,
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
    // The random tail is what makes two of these distinguishable when a pid
    // cannot tell them apart -- a second `RoomSet` in one process, or a pid
    // reused after a restart. A holder that another holder can be mistaken for
    // is a lease that renews when it should have been refused.
    let token = hex::encode(crate::auth::random_bytes(4));
    format!("{host}/{}/{token}", std::process::id())
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
        // bucket finds out it is second rather than interleaving its writes
        // with the first one's.
        let lease = take_room_lease(self.blobs.as_ref(), slug, &self.holder, None).await;
        if !lease.held {
            eprintln!(
                "warning: the lease on {slug} is held by {} at epoch {}; this server serves it \
                 read-only",
                lease.holder, lease.epoch
            );
        }
        let room = Arc::new(Room {
            slug: slug.to_string(),
            blobs: self.blobs.clone(),
            config: self.config.clone(),
            read_only: std::sync::atomic::AtomicBool::new(!lease.held),
            holder: self.holder.clone(),
            lease: Mutex::new(lease),
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
                    last_tree: None,
                    blobs_written: std::collections::HashSet::new(),
                    asset_sizes: HashMap::new(),
                    asset_written_at: HashMap::new(),
                },
                manifest: Manifest::default(),
                touched: now_unix(),
                session_version: BlobVersion::new(),
                manifest_version: BlobVersion::new(),
                comments_version: BlobVersion::new(),
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
        // So do its figures: they are the document's, stored under its slug
        // and referred to by nothing else.
        for prefix in [
            crate::blob::history_prefix(slug),
            crate::blob::asset_prefix(slug),
        ] {
            if let Ok(found) = self.blobs.list(&prefix).await {
                let keys: Vec<String> = found.into_iter().map(|object| object.key).collect();
                if !keys.is_empty() {
                    let _ = self.blobs.delete(&keys).await;
                }
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
        // Read with its version, because every write of it is conditional on
        // the version this server last saw.
        if let Ok((raw, at)) = self.blobs.get_versioned(&room_key(&self.slug)).await {
            if let Ok(stored) = serde_json::from_slice::<RoomState_>(&raw) {
                let mut state = self.state.lock().await;
                state.seq = stored.seq;
                state.comments = stored.comments;
                state.comments.sort_by_key(|item| item.seq);
                state.comments_version = at;
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
        let (manifest, manifest_at) = history::load_versioned(self.blobs.as_ref(), &self.slug)
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

        let stored = match self.blobs.get_versioned(&session_key(&self.slug)).await {
            Ok((raw, at)) => Some((raw, at)),
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
        state.manifest_version = manifest_at;
        state.session.format = format;
        if let Some(point) = state.manifest.latest() {
            state.session.last_checkpoint = point.sha.clone();
        }
        let named = entry
            .as_ref()
            .map(|entry| entry.main.clone())
            .unwrap_or_default();
        match stored {
            Some((raw, at)) => {
                state.session_version = at;
                if let Err(err) = session::apply_update(&state.session.doc, &raw) {
                    eprintln!(
                        "warning: the session for {} is unreadable ({err})",
                        self.slug
                    );
                }
            }
            None => {
                if let Some((source, format)) = seed {
                    state.session.format = format.clone();
                    // A seeded document is a directory of one file from the
                    // start: `replace_text` makes the file, and the migration
                    // below only ever has work to do for a session that was
                    // written before there were directories.
                    session::migrate(&state.session.doc, &main_path_for(&named, &format));
                    session::replace_text(
                        &state.session.doc,
                        &source,
                        &main_path_for(&named, &format),
                    );
                    // Seeded, not edited: what is in the document is what
                    // storage already says, so there is nothing to write back
                    // until somebody types.
                    state.session.dirty = false;
                }
            }
        }
        // The migration, once per document, the first time this code loads a
        // session that was written when a document was one text. It is done
        // here rather than lazily on the first write so that everything after
        // it -- the ceiling, the repair, the checkpoint -- sees a directory
        // and never has to ask which shape it is looking at.
        let format = state.session.format.clone();
        if session::migrate(&state.session.doc, &main_path_for(&named, &format)) {
            state.session.dirty = true;
        }
        drop(state);
        self.load_asset_sizes().await;
    }

    /// What each of this document's figures weighs, read once when the room is
    /// loaded. The shared document carries a figure's name and digest and not
    /// its size, and a checkpoint has to record what the document costs -- so
    /// the answer is read from where the bytes are, which is the store.
    ///
    /// One listing per room rather than one request per figure, and a failure
    /// is not fatal: a size this does not know reads as zero, which
    /// under-counts a quota rather than refusing a document.
    async fn load_asset_sizes(&self) {
        let Ok(found) = self
            .blobs
            .list(&crate::blob::asset_prefix(&self.slug))
            .await
        else {
            return;
        };
        let mut state = self.state.lock().await;
        for object in found {
            if let Some(sha) = object.key.rsplit('/').next() {
                state
                    .session
                    .asset_sizes
                    .insert(sha.to_string(), object.size);
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
        let source = if entry.source_format.is_empty() || entry.source_format == "html" {
            None
        } else {
            store.read_source(&self.slug).await.ok()
        };
        if let Some(raw) = source {
            return Some((
                String::from_utf8_lossy(&raw).to_string(),
                entry.source_format.clone(),
            ));
        }
        // Nothing under the old keys either. That is the ordinary state of a
        // document that has just been created -- the index entry exists and
        // the session is about to be seeded by whoever created it -- so it is
        // not worth a word. Only a document that had a source and lost it
        // falls through to its page, and that is worth saying.
        let page = store.read(&self.slug, &entry.sha).await.ok()?;
        if !entry.source_format.is_empty() && entry.source_format != "html" {
            eprintln!(
                "warning: {} has no stored {} source; it opens as the page it was published as",
                self.slug, entry.source_format
            );
        }
        Some((
            String::from_utf8_lossy(&page).to_string(),
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

    /// Says whether this server may still write the room, renewing the lease
    /// when it is old enough to be worth saying so again. Renewal is on the
    /// write path rather than on a timer: a room nobody is writing does not
    /// need holding, and a room being written is asked about often enough.
    ///
    /// Two things make this a lease rather than a claim. A renewal asserts the
    /// epoch this server believes it holds, so a server that was fenced out
    /// while it was stalled finds out instead of writing over the new holder.
    /// And past `Lease::safe_until` -- a guard's width before the lease could
    /// be taken from us -- writing is refused unless a renewal succeeds first,
    /// so a holder stops strictly before anybody else could start, without
    /// having to trust its own idea of the time against theirs.
    ///
    /// Losing the lease makes the room read-only for good. The other server's
    /// copy is the live one, and continuing to write ours would put half of
    /// each thread in the stored list.
    async fn hold(&self) -> bool {
        if self.read_only() {
            return false;
        }
        let now = now_unix();
        let mut lease = self.lease.lock().await;
        // Fresh, and not near the edge: no need to ask storage anything.
        if now - lease.taken_at < RENEW_AFTER_SECONDS && now < lease.safe_until() {
            return true;
        }
        let renewed = take_room_lease(
            self.blobs.as_ref(),
            &self.slug,
            &self.holder,
            Some(lease.epoch),
        )
        .await;
        if !renewed.held {
            eprintln!(
                "warning: the lease on {} is held by {} at epoch {}; this server is read-only \
                 for it from now on",
                self.slug, renewed.holder, renewed.epoch
            );
            self.read_only.store(true, Ordering::Relaxed);
            return false;
        }
        *lease = renewed;
        true
    }

    /// Writes an object this room owns, and only over the version this server
    /// last saw. This is the enforcement behind the lease: a write that loses
    /// the compare-and-swap is proof that another process owns the room, and
    /// this one stops writing rather than finding out later. `version` is
    /// updated in place on success.
    async fn write_owned(
        &self,
        key: &str,
        body: Vec<u8>,
        version: &mut BlobVersion,
    ) -> Result<(), String> {
        match self.blobs.swap(key, body, version).await {
            Ok(at) => {
                *version = at;
                Ok(())
            }
            Err(BlobError::Conflict) => {
                eprintln!(
                    "warning: {key} was written by another server; this one is read-only for {} \
                     from now on",
                    self.slug
                );
                self.read_only.store(true, Ordering::Relaxed);
                Err("this room is written by another server".into())
            }
            Err(err) => Err(err.to_string()),
        }
    }

    pub async fn save(&self, state: &mut RoomState) -> Result<(), String> {
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let raw = json!({"seq": state.seq, "comments": to_stored(&state.comments)});
        let body = serde_json::to_vec(&raw).map_err(|err| err.to_string())?;
        let mut version = std::mem::take(&mut state.comments_version);
        let result = self
            .write_owned(&room_key(&self.slug), body, &mut version)
            .await;
        state.comments_version = version;
        result
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
        via: &str,
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

        // What the document says at this moment, by name. The socket takes a
        // checkpoint before a comment reaches here, so for a comment this is
        // the text the reviewer was looking at; for a resolve it is the text
        // the author was looking at when they called it done. Either way it is
        // the server's to record and never the client's to send.
        let current = state
            .manifest
            .latest()
            .map(|point| point.sha.clone())
            .unwrap_or_default();

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
            let (was_resolved, was_resolved_at, was_resolved_in) = (
                state.comments[index].resolved,
                state.comments[index].resolved_at.clone(),
                state.comments[index].resolved_in.clone(),
            );
            state.comments[index].resolved = incoming.resolved;
            state.comments[index].resolved_at = incoming.resolved.then(timestamp);
            // Which text it was resolved against. Cleared when a comment is
            // reopened, because it is no longer resolved in anything.
            state.comments[index].resolved_in = if incoming.resolved {
                current.clone()
            } else {
                String::new()
            };
            if self.save(&mut state).await.is_err() {
                state.comments[index].resolved = was_resolved;
                state.comments[index].resolved_at = was_resolved_at;
                state.comments[index].resolved_in = was_resolved_in;
                return fail(UNSAVED);
            }
            let target = &state.comments[index];
            return (
                json!({
                    "type": "resolve", "comment_id": target.id,
                    "resolved": target.resolved, "resolved_at": target.resolved_at,
                    "resolved_in": target.resolved_in,
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
            if self.save(&mut state).await.is_err() {
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
                if self.save(&mut state).await.is_err() {
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
                    revision: current,
                    resolved_in: String::new(),
                    replies: Vec::new(),
                    author: author.to_string(),
                    via: via.to_string(),
                };
                state.comments.push(added.clone());
                if self.save(&mut state).await.is_err() {
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
        self.set_main_file(source, format, "").await
    }

    /// The same, naming the main file. A directory publish knows what its
    /// document is called; a one-file publish does not and takes the name its
    /// format implies.
    pub async fn set_main_file(&self, source: &str, format: &str, named: &str) -> Vec<u8> {
        let mut state = self.state.lock().await;
        let before = session::encode_vector(&state.session.doc);
        // What the main file is called, for the one case where there is not
        // one yet: a document being published for the first time. It follows
        // from what the document is written in, which is the same name the
        // migration gives a document that predates directories.
        let implied = if format.is_empty() {
            state.session.format.clone()
        } else {
            format.to_string()
        };
        session::replace_text(&state.session.doc, source, &main_path_for(named, &implied));
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
    /// The size ceiling is decided before the document is touched. An update
    /// that would carry the source past `max_html` is never applied and never
    /// relayed, and the socket that sent it is closed with the reason; the
    /// text every other peer is looking at does not move, not even for the
    /// instant a trim-afterwards would have taken. `session::admit_update`
    /// answers by a bound in the ordinary case and by rehearsing the update on
    /// a scratch copy only when the bound cannot decide, so no number of
    /// concurrent writers can talk their way past the quota between them and
    /// the common path costs a comparison.
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
        }
        match session::admit_update(
            &state.session.doc,
            update,
            self.config.max_document,
            self.config.max_files,
        ) {
            session::Admission::Malformed => return Applied::Ignored,
            session::Admission::TooLarge => {
                return Applied::Refuse("this document has reached its size limit")
            }
            session::Admission::TooMany => {
                return Applied::Refuse("this document has reached its file limit")
            }
            session::Admission::Fits => {}
        }
        if session::apply_update(&state.session.doc, update).is_err() {
            return Applied::Ignored;
        }
        // Taken after the peer's update rather than before it, so that what is
        // relayed below is the correction alone and not the peer's own work
        // sent back to it a second time.
        let before = session::encode_vector(&state.session.doc);
        // Every key in the shared document is a string an editor can set, so
        // what one wrote is checked before anybody else is shown it. A fault
        // here is put right rather than refused: bytes are the bill and a
        // socket that spends them is closed, but a path the rules refuse is a
        // mistake a person can see, and closing their socket over it would
        // lose the rest of what they typed. What the repair changed is relayed
        // as the server's own update, after the peer's, so every browser --
        // the one that wrote the bad path included -- ends at the same
        // document.
        let put_right = session::repair(&state.session.doc, &self.config.paths());
        if !put_right.is_empty() {
            if let Ok(correction) = session::encode_diff(&state.session.doc, &before) {
                let payload =
                    json!({"type": "y-update", "update": encode_update(&correction)}).to_string();
                send_to_all(&mut state, None, &payload);
            }
        }
        // Counted only once the update is one this document actually took. A
        // refused update must not be acknowledged by the next write, and it
        // would be if it had moved this socket's high-water mark.
        if let Some(peer) = state.sockets.get_mut(&socket) {
            if seq > peer.sent {
                peer.sent = seq;
            }
        }
        state.session.dirty = true;
        state.session.updated_at = now;
        state.session.by = by.to_string();
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
        let mut version = std::mem::take(&mut state.session_version);
        let written = self
            .write_owned(&session_key(&self.slug), body, &mut version)
            .await;
        state.session_version = version;
        written?;
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
    /// The order of writes is the one `docs/specs/history.md` sets, with the text
    /// blobs in front of it, so that a crash leaves nothing worse than an
    /// untidy history: the blobs, then the tree, then the session state, then
    /// the index entry, then the manifest. Nothing ever names an object that
    /// is not there; what a crash can leave is an object nothing names, which
    /// costs storage and loses nothing. A manifest missing its newest entry is
    /// repaired by the next checkpoint, which finds the object present and
    /// names it as `parent` -- `repair` below is that.
    pub async fn checkpoint(&self, why: &str, by: &str) -> Result<Option<String>, String> {
        let now = now_unix();
        let (tree, bodies, format, last, deferred) = {
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
                (
                    crate::history::Tree::default(),
                    HashMap::new(),
                    String::new(),
                    String::new(),
                    true,
                )
            } else {
                let (tree, bodies) = tree_of(&state.session.doc, &state.session.asset_sizes);
                (
                    tree,
                    bodies,
                    state.session.format.clone(),
                    state.session.last_checkpoint.clone(),
                    false,
                )
            }
        };
        if deferred {
            return Ok(None);
        }

        let sha = tree.digest();
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

        // 1. the text blobs, before anything names them. A digest already
        //    written is not written again, which is what makes a chapter
        //    untouched between twenty checkpoints cost one object.
        let unwritten: Vec<(String, String)> = {
            let state = self.state.lock().await;
            bodies
                .iter()
                .filter(|(digest, _)| !state.session.blobs_written.contains(*digest))
                .map(|(digest, body)| (digest.clone(), body.clone()))
                .collect()
        };
        for (digest, body) in &unwritten {
            self.blobs
                .put(
                    &crate::blob::blob_key(&self.slug, digest),
                    body.clone().into_bytes(),
                    "text/plain; charset=utf-8",
                )
                .await
                .map_err(|err| err.to_string())?;
        }
        {
            let mut state = self.state.lock().await;
            for (digest, _) in &unwritten {
                state.session.blobs_written.insert(digest.clone());
            }
        }

        // 2. the tree, which names them.
        self.blobs
            .put(
                &checkpoint_key(&self.slug, &sha),
                tree.to_bytes(),
                "application/json; charset=utf-8",
            )
            .await
            .map_err(|err| err.to_string())?;

        // 3. the session state, so a restart comes back at or after the
        //    checkpoint rather than before it.
        let state_bytes = {
            let state = self.state.lock().await;
            session::encode_state(&state.session.doc)
        };
        let session_size = state_bytes.len() as i64;
        {
            let mut state = self.state.lock().await;
            let mut version = std::mem::take(&mut state.session_version);
            let written = self
                .write_owned(&session_key(&self.slug), state_bytes, &mut version)
                .await;
            state.session_version = version;
            written?;
        }

        // What the parent recorded, so this entry can say which paths moved.
        // Held in memory from one checkpoint to the next; read back only on
        // the first checkpoint after a cold start, which is the only time
        // this server has not seen the parent itself.
        let parent_tree = self.parent_tree().await;

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
                size: tree.size(),
                label: String::new(),
                commit: String::new(),
                dirty: false,
                tree: true,
                changed: tree.changed_from(parent_tree.as_ref()),
            });
            state.session.last_tree = Some(tree.clone());
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

        // 4. the index entry, then 5. the manifest.
        // What the document costs: the live session, its history, and its
        // figures. The quota counts each object once -- a text blob and an
        // asset are each charged where they are stored, and the tree that
        // names them is bookkeeping rather than a third copy.
        let assets = self.assets_bytes().await;
        self.record_size(size + manifest.bytes() + assets, Some(&sha))
            .await;
        {
            let mut state = self.state.lock().await;
            let body = serde_json::to_vec(&manifest).map_err(|err| err.to_string())?;
            let mut version = std::mem::take(&mut state.manifest_version);
            let written = self
                .write_owned(
                    &crate::blob::history_index_key(&self.slug),
                    body,
                    &mut version,
                )
                .await;
            state.manifest_version = version;
            written?;
            state.manifest = manifest;
        }
        if !shed.is_empty() {
            let keys: Vec<String> = shed
                .iter()
                .map(|sha| checkpoint_key(&self.slug, sha))
                .collect();
            let _ = self.blobs.delete(&keys).await;
        }
        // The figures nothing refers to any more, once the tree and the
        // manifest that name what is kept are both written. This order is
        // what makes a crash leave an unreferenced object rather than a tree
        // pointing at one that is gone.
        self.prune_assets().await;
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
        let raw = self
            .blobs
            .get(&checkpoint_key(&self.slug, last))
            .await
            .unwrap_or_default();
        // Whether what was recovered is a tree is answered by the object
        // itself, since a checkpoint written by this code and one written
        // before there were directories sit under the same key. Reading it as
        // a tree is the test: a source that happens to parse as this exact
        // JSON shape is not a source anybody wrote.
        let recovered: Option<crate::history::Tree> = serde_json::from_slice(&raw).ok();
        let size = match &recovered {
            Some(tree) => tree.size(),
            None => raw.len() as i64,
        };
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
            tree: recovered.is_some(),
            changed: Vec::new(),
        });
    }

    /// The tree the newest checkpoint recorded, for the sake of the `changed`
    /// list on the next one. Kept in memory between checkpoints; read back
    /// only after a cold start, and read as a one-file tree when the entry is
    /// from before a document was a directory.
    async fn parent_tree(&self) -> Option<crate::history::Tree> {
        let (held, point, path, id) = {
            let state = self.state.lock().await;
            (
                state.session.last_tree.clone(),
                state.manifest.latest().cloned(),
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        if held.is_some() {
            return held;
        }
        let point = point?;
        crate::history::load_tree(self.blobs.as_ref(), &self.slug, &point, &path, &id)
            .await
            .ok()
    }

    /// What one checkpoint said: its tree, and the text of every file in it by
    /// digest. Read whole, before anything acts on it, because a caller that
    /// got half the files would be worse off than one that was refused -- a
    /// restore would leave a chapter and the file that includes it out of
    /// step, and the timeline would show a document that never existed.
    pub async fn checkpoint_texts(
        &self,
        point: &Checkpoint,
    ) -> Result<(crate::history::Tree, HashMap<String, String>), String> {
        let (path, id) = {
            let state = self.state.lock().await;
            (
                session::main_path(&state.session.doc),
                session::main_id(&state.session.doc),
            )
        };
        let tree =
            crate::history::load_tree(self.blobs.as_ref(), &self.slug, point, &path, &id).await?;
        let mut bodies = HashMap::new();
        for entry in tree.files.values() {
            if entry.kind != "text" || bodies.contains_key(&entry.sha) {
                continue;
            }
            let raw = if point.tree {
                self.blobs
                    .get(&crate::blob::blob_key(&self.slug, &entry.sha))
                    .await
            } else {
                // A checkpoint from before directories is its own text.
                self.blobs
                    .get(&checkpoint_key(&self.slug, point.sha.as_str()))
                    .await
            }
            .map_err(|err| err.to_string())?;
            bodies.insert(entry.sha.clone(), String::from_utf8_lossy(&raw).to_string());
        }
        Ok((tree, bodies))
    }

    /// Names a checkpoint, or takes its name away when `label` is empty.
    ///
    /// This is the one write that changes a manifest entry after it is made,
    /// and it changes exactly one field. Nothing else about a checkpoint is
    /// ever rewritten: what it recorded is what it recorded, and a label is
    /// somebody's remark about it rather than a claim about the text.
    ///
    /// `Ok(false)` means the manifest has no such checkpoint, which is a
    /// 404 for the caller rather than a failure here.
    pub async fn label(&self, sha: &str, label: &str) -> Result<bool, String> {
        {
            let state = self.state.lock().await;
            if !state.manifest.has(sha) {
                return Ok(false);
            }
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        let manifest = {
            let mut state = self.state.lock().await;
            for point in state.manifest.checkpoints.iter_mut() {
                if point.sha == sha {
                    point.label = label.to_string();
                }
            }
            state.manifest.clone()
        };
        let body = serde_json::to_vec(&manifest).map_err(|err| err.to_string())?;
        let mut state = self.state.lock().await;
        let mut version = std::mem::take(&mut state.manifest_version);
        let written = self
            .write_owned(
                &crate::blob::history_index_key(&self.slug),
                body,
                &mut version,
            )
            .await;
        state.manifest_version = version;
        written?;
        Ok(true)
    }

    /// Puts the document back to what a checkpoint recorded: every text and
    /// every asset at once, so a chapter and the file that includes it can
    /// never come back out of step. Returns the update to relay.
    #[allow(dead_code)] // the route that offers it to a reader is step 9
    pub async fn restore(&self, point: &Checkpoint) -> Result<Vec<u8>, String> {
        let (tree, bodies) = self.checkpoint_texts(point).await?;
        let mut state = self.state.lock().await;
        let before = session::encode_vector(&state.session.doc);
        session::restore(&state.session.doc, &tree, &bodies);
        state.session.dirty = true;
        state.session.updated_at = now_unix();
        Ok(session::encode_diff(&state.session.doc, &before)
            .unwrap_or_else(|_| session::encode_state(&state.session.doc)))
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
        let (format, main) = {
            let state = self.state.lock().await;
            (
                state.session.format.clone(),
                session::main_path(&state.session.doc),
            )
        };
        if let Err(err) = store
            .record_history(&self.slug, sha, size, &format, &main)
            .await
        {
            eprintln!(
                "warning: could not record the history of {}: {err}",
                self.slug
            );
        }
    }

    /// The document's directory as a checkpoint would record it. What the
    /// timeline reads, and what a test asks when it wants to know the name the
    /// next checkpoint will have.
    #[allow(dead_code)] // the timeline that reads it is step 6; the tests ask now
    pub async fn tree(&self) -> crate::history::Tree {
        let state = self.state.lock().await;
        tree_of(&state.session.doc, &state.session.asset_sizes).0
    }

    /// Puts a text at a path in the document, beside whatever is already
    /// there. What a directory publish adds each of its chapters with.
    pub async fn add_text(&self, path: &str, body: &str) {
        let mut state = self.state.lock().await;
        session::put_text(&state.session.doc, path, body);
        state.session.dirty = true;
        state.session.updated_at = now_unix();
    }

    /// Names a figure in the document, at a path. The bytes are already in the
    /// store; this is what makes them a figure of this document.
    pub async fn name_asset(&self, path: &str, sha: &str) {
        let mut state = self.state.lock().await;
        session::put_asset(&state.session.doc, path, sha);
        state.session.dirty = true;
        state.session.updated_at = now_unix();
    }

    /* ------------------------------------------------------------- assets */

    /// What this document's figures come to, which is what `max_assets` bounds
    /// and what the owner's quota is charged for.
    pub async fn assets_bytes(&self) -> i64 {
        self.state.lock().await.session.asset_sizes.values().sum()
    }

    /// Stores a figure and answers with its digest. The name is the client's
    /// to give -- it sets `assets[path]` in the shared document afterwards --
    /// and the bytes are the server's to keep.
    ///
    /// Bytes the document already has are not written again: the same figure
    /// uploaded twice is one object, and the second upload costs a hash.
    pub async fn put_asset(
        &self,
        body: Vec<u8>,
        ceilings: (i64, i64),
    ) -> Result<(String, i64), String> {
        let (max_asset, max_assets) = ceilings;
        let size = body.len() as i64;
        if size == 0 {
            return Err("that file is empty".into());
        }
        if size > max_asset {
            return Err(format!(
                "that figure is larger than the {} MB one file may be",
                max_asset >> 20
            ));
        }
        let sha = crate::store::digest_of_bytes(&body);
        {
            let state = self.state.lock().await;
            if let Some(known) = state.session.asset_sizes.get(&sha) {
                // Already here. Nothing is written and nothing is charged: the
                // same bytes under the same name are the same object.
                return Ok((sha, *known));
            }
            let held: i64 = state.session.asset_sizes.values().sum();
            if held + size > max_assets {
                return Err(format!(
                    "this document has reached the {} MB it may keep in figures",
                    max_assets >> 20
                ));
            }
        }
        if !self.hold().await {
            return Err("this room is held by another server".into());
        }
        self.blobs
            .put(
                &crate::blob::asset_key(&self.slug, &sha),
                body,
                "application/octet-stream",
            )
            .await
            .map_err(|err| err.to_string())?;
        {
            let mut state = self.state.lock().await;
            state.session.asset_sizes.insert(sha.clone(), size);
            state
                .session
                .asset_written_at
                .insert(sha.clone(), now_unix());
        }
        // What the document costs has changed, and the index is what the
        // quota is decided from.
        self.record_size_now().await;
        Ok((sha, size))
    }

    /// A figure's bytes, for whoever may read the document.
    pub async fn read_asset(&self, sha: &str) -> Option<Vec<u8>> {
        self.blobs
            .get(&crate::blob::asset_key(&self.slug, sha))
            .await
            .ok()
    }

    /// Drops the figures nothing refers to any more: neither the live document
    /// nor any checkpoint the manifest still holds.
    ///
    /// Run after the new tree and manifest are written, never before, so that
    /// a crash leaves an object nothing names -- which costs storage and loses
    /// nothing -- rather than a tree naming an object that is gone.
    ///
    /// An object younger than the grace period is kept whatever the document
    /// says about it, because uploading a figure and naming it are two
    /// requests and pruning between them would delete what somebody had just
    /// uploaded.
    async fn prune_assets(&self) {
        let now = now_unix();
        let grace = self.config.asset_grace;
        let (live, trees, written_at) = {
            let state = self.state.lock().await;
            let live: std::collections::HashSet<String> = session::assets_of(&state.session.doc)
                .into_values()
                .collect();
            let trees: Vec<Checkpoint> = state.manifest.checkpoints.clone();
            (live, trees, state.session.asset_written_at.clone())
        };
        // Every digest any surviving checkpoint names. A restore has to find
        // its figures where the tree says they are.
        let mut kept = live;
        for point in &trees {
            if !point.tree {
                continue; // a checkpoint from before directories names none
            }
            let (path, id) = {
                let state = self.state.lock().await;
                (
                    session::main_path(&state.session.doc),
                    session::main_id(&state.session.doc),
                )
            };
            if let Ok(tree) =
                crate::history::load_tree(self.blobs.as_ref(), &self.slug, point, &path, &id).await
            {
                for entry in tree.files.values() {
                    if entry.kind == "asset" {
                        kept.insert(entry.sha.clone());
                    }
                }
            }
        }
        let Ok(found) = self
            .blobs
            .list(&crate::blob::asset_prefix(&self.slug))
            .await
        else {
            return;
        };
        let mut gone = Vec::new();
        for object in found {
            let Some(sha) = object.key.rsplit('/').next() else {
                continue;
            };
            if kept.contains(sha) {
                continue;
            }
            if written_at.get(sha).is_some_and(|at| now - at < grace) {
                continue;
            }
            gone.push((object.key.clone(), sha.to_string()));
        }
        if gone.is_empty() {
            return;
        }
        let keys: Vec<String> = gone.iter().map(|(key, _)| key.clone()).collect();
        if self.blobs.delete(&keys).await.is_ok() {
            let mut state = self.state.lock().await;
            for (_, sha) in gone {
                state.session.asset_sizes.remove(&sha);
                state.session.asset_written_at.remove(&sha);
            }
        }
    }

    /// Records what this document costs as it stands, without a checkpoint:
    /// what an asset upload changes.
    async fn record_size_now(&self) {
        let (session_size, history) = {
            let state = self.state.lock().await;
            (
                session::encode_state(&state.session.doc).len() as i64,
                state.manifest.bytes(),
            )
        };
        let assets = self.assets_bytes().await;
        self.record_size(session_size + history + assets, None)
            .await;
    }

    /// The manifest, for the timeline and for the tests.
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

/// The document's directory as a checkpoint records it, and the bytes of each
/// text by digest -- which is what the blobs are written from, so that two
/// files with the same contents are one object and a file that did not change
/// is not written again.
pub fn tree_of(
    doc: &yrs::Doc,
    asset_sizes: &HashMap<String, i64>,
) -> (crate::history::Tree, HashMap<String, String>) {
    use crate::history::{Tree, TreeEntry};
    let ids = session::paths_of(doc);
    let mut by_path: HashMap<String, String> = HashMap::new();
    for (id, path) in &ids {
        by_path.insert(path.clone(), id.clone());
    }
    let mut files = std::collections::BTreeMap::new();
    let mut bodies = HashMap::new();
    for (path, body) in session::texts_of(doc) {
        let sha = crate::store::digest_of(&body);
        files.insert(
            path.clone(),
            TreeEntry {
                kind: "text".to_string(),
                id: by_path.get(&path).cloned().unwrap_or_default(),
                sha: sha.clone(),
                size: body.len() as i64,
            },
        );
        bodies.insert(sha, body);
    }
    for (path, sha) in session::assets_of(doc) {
        files.insert(
            path,
            TreeEntry {
                kind: "asset".to_string(),
                id: String::new(),
                // What a figure costs is known where its bytes are, not in the
                // shared document, which carries only its name and digest.
                size: asset_sizes.get(&sha).copied().unwrap_or(0),
                sha,
            },
        );
    }
    (
        Tree {
            main: session::main_path(doc),
            files,
        },
        bodies,
    )
}

/// What to call the one file a migrated document turns out to have. The index
/// entry's own `main` if it has one; otherwise the name its format implies,
/// which is what every document published before directories was called on
/// the laptop it came from.
pub fn main_path_for(named: &str, format: &str) -> String {
    if !named.is_empty() {
        return named.to_string();
    }
    match format {
        "typst" => "main.typ",
        "markdown" => "main.md",
        "html" => "main.html",
        _ => "main.txt",
    }
    .to_string()
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
