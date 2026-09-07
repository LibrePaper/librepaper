//! Private document mailboxes. A chat capability never grants document access.
use std::collections::BTreeMap;
use std::sync::Arc;

use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::blob::{BlobError, BlobStore};

const MAX_BYTES: usize = 4 * 1024 * 1024;
#[derive(Default, Serialize, Deserialize)]
struct Mailboxes {
    conversations: BTreeMap<String, Conversation>,
}
#[derive(Serialize, Deserialize)]
struct Conversation {
    token_hash: String,
    expires_at: i64,
    messages: Vec<Message>,
    #[serde(default)]
    listening_until: i64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub cursor: u64,
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub context: Value,
}
#[derive(Deserialize)]
pub struct Post {
    pub id: String,
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub context: Value,
}
pub enum Action {
    Create,
    Read(u64),
    Post(Post),
    Listen,
    Delete,
}
pub type Error = (u16, &'static str);
fn random() -> String {
    let mut bytes = [0; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn token_matches(stored: &str, token: &str) -> bool {
    let hash = crate::store::digest_of(token);
    let difference = stored
        .bytes()
        .zip(hash.bytes())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b));
    !token.is_empty() && stored.len() == hash.len() && difference == 0
}

/// CAS keeps simultaneous sidebar and agent writes from losing messages.
/// Bounds also prevent holders of a reader link from consuming unlimited storage.
pub async fn request(
    blobs: &Arc<dyn BlobStore>,
    slug: &str,
    id: &str,
    token: &str,
    action: Action,
) -> Result<Value, Error> {
    if let Action::Post(post) = &action {
        if post.id.is_empty()
            || post.id.len() > 128
            || !matches!(post.role.as_str(), "user" | "agent")
            || post.text.trim().is_empty()
            || post.text.len() > 32 * 1024
            || serde_json::to_vec(&post.context)
                .map_err(|_| (400, "bad context"))?
                .len()
                > 16 * 1024
        {
            return Err((
                400,
                "invalid message: bounded id, user/agent role, and nonempty text required",
            ));
        }
    }
    let key = format!("chat/{slug}.json");
    for _ in 0..12 {
        let (mut data, version) = match blobs.get_versioned(&key).await {
            Ok((bytes, version)) => (
                serde_json::from_slice::<Mailboxes>(&bytes)
                    .map_err(|_| (500, "could not read conversation"))?,
                version,
            ),
            Err(BlobError::NotFound) => (Mailboxes::default(), String::new()),
            Err(_) => return Err((500, "could not read conversation")),
        };
        data.conversations
            .retain(|_, conversation| conversation.expires_at > now());
        let result = if matches!(action, Action::Create) {
            if data.conversations.len() >= 16 {
                return Err((
                    409,
                    "document conversation limit reached; delete an old conversation",
                ));
            }
            let id = random();
            let token = random();
            data.conversations.insert(
                id.clone(),
                Conversation {
                    token_hash: crate::store::digest_of(&token),
                    expires_at: now() + 30 * 24 * 60 * 60,
                    messages: Vec::new(),
                    listening_until: 0,
                },
            );
            json!({"id":id,"token":token})
        } else {
            let conversation = data
                .conversations
                .get_mut(id)
                .filter(|c| token_matches(&c.token_hash, token))
                .ok_or((404, "conversation not found"))?;
            match &action {
                Action::Read(after) => {
                    return Ok(json!({
                        "messages":conversation.messages.iter().filter(|m|m.cursor > *after).collect::<Vec<_>>(),
                        "next_cursor":conversation.messages.last().map(|m|m.cursor).unwrap_or(0),
                        "listening":conversation.listening_until > now(),
                    }))
                }
                Action::Post(post) => {
                    if let Some(existing) = conversation.messages.iter().find(|m| m.id == post.id) {
                        if existing.role != post.role
                            || existing.text != post.text
                            || existing.context != post.context
                        {
                            return Err((409, "message id already used for different content"));
                        }
                        return Ok(json!({"message":existing,"next_cursor":existing.cursor}));
                    }
                    if conversation.messages.len() >= 256 {
                        return Err((409, "conversation is full; start a new conversation"));
                    }
                    let message = Message {
                        id: post.id.clone(),
                        cursor: conversation
                            .messages
                            .last()
                            .map(|m| m.cursor + 1)
                            .unwrap_or(1),
                        role: post.role.clone(),
                        text: post.text.clone(),
                        context: post.context.clone(),
                    };
                    conversation.messages.push(message.clone());
                    if serde_json::to_vec(conversation)
                        .map_err(|_| (500, "could not save conversation"))?
                        .len()
                        > 1024 * 1024
                    {
                        return Err((409, "conversation is full; start a new conversation"));
                    }
                    json!({"message":message,"next_cursor":message.cursor})
                }
                Action::Listen => {
                    // Avoid a storage write on every poll; a 30-second lease
                    // is refreshed once half of it has elapsed.
                    if conversation.listening_until > now() + 15 {
                        return Ok(json!({"listening":true}));
                    }
                    conversation.listening_until = now() + 30;
                    json!({"listening":true})
                }
                Action::Delete => {
                    data.conversations.remove(id);
                    json!({"deleted":true})
                }
                Action::Create => unreachable!(),
            }
        };
        let bytes = serde_json::to_vec(&data).map_err(|_| (500, "could not save conversation"))?;
        if bytes.len() > MAX_BYTES {
            return Err((409, "document conversation storage is full"));
        }
        match blobs.swap(&key, bytes, &version).await {
            Ok(_) => return Ok(result),
            Err(BlobError::Conflict) => continue,
            Err(_) => return Err((500, "could not save conversation")),
        }
    }
    Err((409, "conversation changed; retry the request"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::FsStore;

    fn post(id: &str, text: &str) -> Action {
        Action::Post(Post {
            id: id.into(),
            role: "user".into(),
            text: text.into(),
            context: Value::Null,
        })
    }

    #[tokio::test]
    async fn mailbox_persists_and_keeps_capabilities_separate() {
        let dir = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path().to_path_buf()));
        let created = request(&blobs, "paper", "", "", Action::Create)
            .await
            .unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["token"].as_str().unwrap();
        for (slug, secret) in [("other", token), ("paper", ""), ("paper", "wrong")] {
            assert_eq!(
                request(&blobs, slug, id, secret, Action::Read(0))
                    .await
                    .unwrap_err()
                    .0,
                404
            );
        }
        request(&blobs, "paper", id, token, post("one", "Question"))
            .await
            .unwrap();
        let retry = request(&blobs, "paper", id, token, post("one", "Question"))
            .await
            .unwrap();
        assert_eq!(retry["next_cursor"], 1);
        assert_eq!(
            request(&blobs, "paper", id, token, post("one", "Changed"))
                .await
                .unwrap_err()
                .0,
            409
        );
        let reopened: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path().to_path_buf()));
        let read = request(&reopened, "paper", id, token, Action::Read(0))
            .await
            .unwrap();
        assert_eq!(read["messages"].as_array().unwrap().len(), 1);
        assert_eq!(read["listening"], false);
        request(&reopened, "paper", id, token, Action::Listen)
            .await
            .unwrap();
        assert_eq!(
            request(&reopened, "paper", id, token, Action::Read(1))
                .await
                .unwrap()["listening"],
            true
        );
        let stored = blobs.get("chat/paper.json").await.unwrap();
        assert!(!String::from_utf8(stored).unwrap().contains(token));
        request(&reopened, "paper", id, token, Action::Delete)
            .await
            .unwrap();
        assert_eq!(
            request(&blobs, "paper", id, token, Action::Read(0))
                .await
                .unwrap_err()
                .0,
            404
        );
    }

    #[tokio::test]
    async fn simultaneous_posts_do_not_lose_messages() {
        let dir = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path().to_path_buf()));
        let created = request(&blobs, "paper", "", "", Action::Create)
            .await
            .unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["token"].as_str().unwrap();
        let (a, b) = tokio::join!(
            request(&blobs, "paper", id, token, post("a", "A")),
            request(&blobs, "paper", id, token, post("b", "B"))
        );
        a.unwrap();
        b.unwrap();
        let read = request(&blobs, "paper", id, token, Action::Read(0))
            .await
            .unwrap();
        assert_eq!(read["messages"].as_array().unwrap().len(), 2);
        assert_eq!(read["next_cursor"], 2);
        assert_eq!(
            request(
                &blobs,
                "paper",
                id,
                token,
                post("large", &"x".repeat(32769))
            )
            .await
            .unwrap_err()
            .0,
            400
        );
    }

    #[tokio::test]
    async fn expired_conversations_release_capacity_and_refuse_old_handles() {
        let dir = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn BlobStore> = Arc::new(FsStore::new(dir.path().to_path_buf()));
        let mut data = Mailboxes::default();
        for index in 0..16 {
            data.conversations.insert(
                index.to_string(),
                Conversation {
                    token_hash: crate::store::digest_of("secret"),
                    expires_at: now() - 1,
                    messages: Vec::new(),
                    listening_until: 0,
                },
            );
        }
        blobs
            .put(
                "chat/paper.json",
                serde_json::to_vec(&data).unwrap(),
                "application/json",
            )
            .await
            .unwrap();
        assert_eq!(
            request(&blobs, "paper", "0", "secret", Action::Read(0))
                .await
                .unwrap_err()
                .0,
            404
        );
        request(&blobs, "paper", "", "", Action::Create)
            .await
            .unwrap();
        let saved: Mailboxes =
            serde_json::from_slice(&blobs.get("chat/paper.json").await.unwrap()).unwrap();
        assert_eq!(saved.conversations.len(), 1);
    }
}
