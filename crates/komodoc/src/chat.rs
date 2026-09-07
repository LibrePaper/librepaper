//! Live private channels. Only socket handles and bounded request digests are
//! retained: message bodies are never stored or replayed.
use crate::room::Outgoing;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{mpsc, Mutex};

const CHANNEL_SECONDS: i64 = 60 * 60;

#[derive(Default)]
pub struct Hub {
    channels: Mutex<HashMap<String, Channel>>,
}

struct Channel {
    slug: String,
    token_hash: String,
    touched_at: i64,
    browser: Option<Peer>,
    agent: Option<Peer>,
    requests: VecDeque<(String, String)>,
    minute: i64,
    sent: u32,
}

struct Peer {
    socket: u64,
    tx: mpsc::Sender<Outgoing>,
    receives: bool,
}

#[derive(Deserialize)]
pub struct Post {
    pub id: String,
    #[serde(default)]
    pub role: String,
    pub text: String,
    #[serde(default)]
    pub context: Value,
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

impl Channel {
    fn listening(&self) -> bool {
        self.agent.as_ref().is_some_and(|peer| peer.receives)
    }

    fn presence(&self) -> Value {
        json!({"type":"presence", "listening":self.listening(), "browser":self.browser.is_some()})
    }

    fn announce(&self) {
        let text = self.presence().to_string();
        for peer in [&self.browser, &self.agent].into_iter().flatten() {
            let _ = peer.tx.try_send(Outgoing::Text(text.clone()));
        }
    }
}

impl Hub {
    pub async fn create(&self, slug: &str) -> Result<Value, Error> {
        let current = now();
        let mut channels = self.channels.lock().await;
        channels.retain(|_, channel| {
            channel.browser.is_some()
                || channel.agent.is_some()
                || current - channel.touched_at < CHANNEL_SECONDS
        });
        if channels
            .values()
            .filter(|channel| channel.slug == slug)
            .count()
            >= 16
            || channels.len() >= 10_000
        {
            return Err((409, "live channel limit reached"));
        }
        let id = random();
        let token = random();
        channels.insert(
            id.clone(),
            Channel {
                slug: slug.into(),
                token_hash: crate::store::digest_of(&token),
                touched_at: current,
                browser: None,
                agent: None,
                requests: VecDeque::new(),
                minute: 0,
                sent: 0,
            },
        );
        Ok(json!({"id":id,"token":token,"ephemeral":true}))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn attach(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        role: &str,
        receives: bool,
        socket: u64,
        tx: mpsc::Sender<Outgoing>,
    ) -> Result<Value, Error> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let participant = match role {
            "user" => &mut channel.browser,
            "agent" => &mut channel.agent,
            _ => return Err((400, "invalid participant role")),
        };
        if participant.is_some() {
            return Err((409, "participant is already connected"));
        }
        *participant = Some(Peer {
            socket,
            tx,
            receives,
        });
        channel.touched_at = now();
        let ready = json!({"type":"ready", "listening":channel.listening(), "browser":channel.browser.is_some()});
        channel.announce();
        Ok(ready)
    }

    pub async fn attached(&self, id: &str, socket: u64) -> bool {
        self.channels.lock().await.get(id).is_some_and(|channel| {
            [&channel.browser, &channel.agent]
                .iter()
                .any(|peer| peer.as_ref().is_some_and(|peer| peer.socket == socket))
        })
    }

    pub async fn detach(&self, id: &str, socket: u64) {
        let mut channels = self.channels.lock().await;
        let Some(channel) = channels.get_mut(id) else {
            return;
        };
        // The originating tab owns the channel's lifetime. Closing it revokes
        // the capability, including outstanding agent connections.
        if channel
            .browser
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            if let Some(peer) = &channel.agent {
                let _ = peer.tx.try_send(Outgoing::Close("browser disconnected"));
            }
            channels.remove(id);
        } else if channel
            .agent
            .as_ref()
            .is_some_and(|peer| peer.socket == socket)
        {
            channel.agent = None;
            channel.touched_at = now();
            channel.announce();
        }
    }

    pub async fn delete(&self, slug: &str, id: &str, token: &str) -> Result<Value, Error> {
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        for peer in [&channel.browser, &channel.agent].into_iter().flatten() {
            let _ = peer.tx.try_send(Outgoing::Close("channel closed"));
        }
        channels.remove(id);
        Ok(json!({"deleted":true}))
    }

    pub async fn purge(&self, slug: &str) {
        let mut channels = self.channels.lock().await;
        channels.retain(|_, channel| {
            if channel.slug != slug {
                return true;
            }
            for peer in [&channel.browser, &channel.agent].into_iter().flatten() {
                let _ = peer.tx.try_send(Outgoing::Close("document removed"));
            }
            false
        });
    }

    /// The socket id binds live writes to the participant established by join.
    /// HTTP replies use the same capability, and require a live agent socket.
    pub async fn post(
        &self,
        slug: &str,
        id: &str,
        token: &str,
        socket: Option<u64>,
        post: Post,
    ) -> Result<Value, Error> {
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
            return Err((400, "invalid bounded message"));
        }
        let mut channels = self.channels.lock().await;
        let channel = channels
            .get_mut(id)
            .filter(|channel| channel.slug == slug && token_matches(&channel.token_hash, token))
            .ok_or((404, "channel not found"))?;
        let (sender, recipient) = if post.role == "user" {
            (&channel.browser, &channel.agent)
        } else {
            (&channel.agent, &channel.browser)
        };
        let sender = sender.as_ref().ok_or((409, "sender is not connected"))?;
        if socket.is_some_and(|socket| socket != sender.socket) {
            return Err((403, "wrong participant"));
        }
        let recipient = recipient.as_ref().ok_or((
            409,
            if post.role == "user" {
                "agent is not connected"
            } else {
                "browser is not connected"
            },
        ))?;
        if !recipient.receives {
            return Err((409, "recipient is not listening"));
        }
        let payload = json!({"type":"message","message":{"id":post.id,"role":post.role,"text":post.text,"context":post.context}});
        let digest = crate::store::digest_of(&payload.to_string());
        let request = format!("{}:{}", post.role, post.id);
        if let Some((_, previous)) = channel.requests.iter().find(|(key, _)| key == &request) {
            return if previous == &digest {
                Ok(json!({"type":"ack","id":post.id}))
            } else {
                Err((409, "message id already used for different content"))
            };
        }
        let minute = now() / 60;
        if channel.minute != minute {
            channel.minute = minute;
            channel.sent = 0;
        }
        if channel.sent >= 60 {
            return Err((429, "too many chat messages; try again shortly"));
        }
        // Reserve both queues first: a slow participant causes a clear refusal,
        // never an offline queue or a half-delivered successful message.
        let recipient_slot = recipient
            .tx
            .try_reserve()
            .map_err(|_| (409, "recipient cannot receive messages"))?;
        let sender_slot = sender
            .tx
            .try_reserve()
            .map_err(|_| (409, "sender cannot receive messages"))?;
        let text = payload.to_string();
        recipient_slot.send(Outgoing::Text(text.clone()));
        sender_slot.send(Outgoing::Text(text));
        channel.requests.push_back((request, digest));
        if channel.requests.len() > 256 {
            channel.requests.pop_front();
        }
        channel.sent += 1;
        channel.touched_at = now();
        Ok(json!({"type":"ack","id":post.id}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn post() -> Post {
        Post {
            id: "one".into(),
            role: "user".into(),
            text: "hi".into(),
            context: Value::Null,
        }
    }
    #[tokio::test]
    async fn reply_only_connection_does_not_accept_browser_instructions() {
        let hub = Hub::default();
        let created = hub.create("paper").await.unwrap();
        let id = created["id"].as_str().unwrap();
        let token = created["token"].as_str().unwrap();
        let (browser, _browser_rx) = mpsc::channel(16);
        let (agent, _agent_rx) = mpsc::channel(16);
        hub.attach("paper", id, token, "user", true, 1, browser)
            .await
            .unwrap();
        let ready = hub
            .attach("paper", id, token, "agent", false, 2, agent)
            .await
            .unwrap();
        assert_eq!(ready["listening"], false);
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        let mut reply = post();
        reply.role = "agent".into();
        assert!(hub.post("paper", id, token, Some(2), reply).await.is_ok());
    }

    #[tokio::test]
    async fn only_live_sockets_receive_no_replay_and_browser_close_revokes() {
        let hub = Hub::default();
        let channel = hub.create("paper").await.unwrap();
        let id = channel["id"].as_str().unwrap();
        let token = channel["token"].as_str().unwrap();
        let (browser, mut browser_rx) = mpsc::channel(16);
        let (agent, mut agent_rx) = mpsc::channel(16);
        assert_eq!(
            hub.attach("paper", id, "wrong", "user", true, 1, browser.clone())
                .await
                .unwrap_err()
                .0,
            404
        );
        hub.attach("paper", id, token, "user", true, 1, browser)
            .await
            .unwrap();
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        hub.attach("paper", id, token, "agent", true, 2, agent)
            .await
            .unwrap();
        hub.post("paper", id, token, Some(1), post()).await.unwrap();
        hub.post("paper", id, token, Some(1), post()).await.unwrap();
        let mut messages = 0;
        while let Ok(frame) = agent_rx.try_recv() {
            if let Outgoing::Text(text) = frame {
                if text.contains("\"type\":\"message\"") {
                    messages += 1;
                }
            }
        }
        assert_eq!(messages, 1, "retries do not duplicate delivery");
        hub.detach(id, 2).await;
        assert_eq!(
            hub.post("paper", id, token, Some(1), post())
                .await
                .unwrap_err()
                .0,
            409
        );
        let (agent, mut agent_rx) = mpsc::channel(16);
        hub.attach("paper", id, token, "agent", true, 3, agent)
            .await
            .unwrap();
        assert!(matches!(agent_rx.try_recv(), Ok(Outgoing::Text(_))));
        assert!(
            agent_rx.try_recv().is_err(),
            "reconnecting never replays messages"
        );
        hub.detach(id, 1).await;
        assert!(!hub.attached(id, 3).await);
        assert_eq!(
            hub.post("paper", id, token, None, post())
                .await
                .unwrap_err()
                .0,
            404
        );
        browser_rx.close();
    }
}
