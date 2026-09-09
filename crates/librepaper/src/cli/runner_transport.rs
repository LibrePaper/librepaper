//! Persistent relay delivery, independent of model turns.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{http::Request, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
const RETRY: Duration = Duration::from_secs(2);

pub(super) enum Event {
    Presence(bool),
    Frame(Value),
    Offline,
    Fatal(String),
}
pub(super) struct Transport {
    pub outgoing: mpsc::Sender<Value>,
    pub incoming: mpsc::Receiver<Event>,
    task: JoinHandle<()>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn send(socket: &mut Socket, value: &Value) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(10),
            socket.send(Message::Text(value.to_string().into()))
        )
        .await,
        Ok(Ok(()))
    )
}
impl Transport {
    pub fn start(request: Request<()>, token: String) -> Self {
        let (outgoing, mut output) = mpsc::channel::<Value>(128);
        let (input, incoming) = mpsc::channel::<Event>(128);
        let task = tokio::spawn(async move {
            let mut attempt = 0u32;
            loop {
                let connect = tokio::time::timeout(
                    Duration::from_secs(15),
                    tokio_tungstenite::connect_async(request.clone()),
                )
                .await;
                let mut socket = match connect {
                    Ok(Ok((socket, _))) => socket,
                    Ok(Err(tokio_tungstenite::tungstenite::Error::Http(response)))
                        if matches!(response.status().as_u16(), 401 | 403 | 404) =>
                    {
                        let _ = input.send(Event::Fatal("Document access is unavailable. Start a new connection with a valid link.".into())).await;
                        return;
                    }
                    _ => {
                        if input.send(Event::Offline).await.is_err() {
                            return;
                        }
                        attempt = (attempt + 1).min(5);
                        tokio::time::sleep(Duration::from_millis(250 * (1 << attempt))).await;
                        continue;
                    }
                };
                if !send(
                    &mut socket,
                    &json!({"type":"join","token":token,"role":"agent"}),
                )
                .await
                {
                    if input.send(Event::Offline).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(RETRY).await;
                    continue;
                }
                let mut browser = false;
                let mut joined = false;
                // One unacknowledged event preserves lifecycle ordering under
                // backpressure. Retries retain the ID and content exactly.
                let mut pending: Option<(Value, tokio::time::Instant)> = None;
                let mut retry = tokio::time::interval(RETRY);
                let handshake = tokio::time::sleep(Duration::from_secs(15));
                tokio::pin!(handshake);
                loop {
                    tokio::select! {
                        _ = &mut handshake, if !joined => break,
                        _ = retry.tick(), if pending.is_some() => {
                            if let Some((value, sent)) = &mut pending {
                                if sent.elapsed() >= RETRY {
                                    if !send(&mut socket, value).await { break; }
                                    *sent = tokio::time::Instant::now();
                                }
                            }
                        }
                        frame = socket.next() => {
                            let value = match frame {
                                Some(Ok(Message::Text(raw))) => match serde_json::from_str::<Value>(&raw) { Ok(value) => value, Err(_) => continue },
                                Some(Ok(Message::Ping(_))) => {
                                    if !matches!(tokio::time::timeout(Duration::from_secs(10),socket.flush()).await,Ok(Ok(()))) { break; }
                                    continue;
                                }
                                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                                _ => continue,
                            };
                            match value["type"].as_str().unwrap_or("") {
                                "ready" | "presence" => {
                                    if value["type"] == "ready" { joined = true; attempt = 0; }
                                    if !joined { continue; }
                                    browser = value["browser"].as_bool().unwrap_or(false);
                                    if !browser { pending = None; }
                                    if input.send(Event::Presence(browser)).await.is_err() { return; }
                                }
                                "error" if !joined => {
                                    if value["status"] == 409 { break; }
                                    let _ = input.send(Event::Fatal(value["message"].as_str().unwrap_or("Assistant channel refused").into())).await;
                                    return;
                                }
                                "ack" => {
                                    if pending.as_ref().is_some_and(|(event,_)|event["id"]==value["id"]) { pending=None; }
                                }
                                "error" if pending.as_ref().is_some_and(|(event,_)|event["id"]==value["id"]) => {
                                    if value["status"] == 409 || value["status"] == 429 { continue; }
                                    pending = None;
                                    if input.send(Event::Frame(value)).await.is_err() { return; }
                                }
                                _ => {
                                    let incoming = matches!(value["type"].as_str(),Some("cancel"|"input"|"preview_result"|"error")) || (value["type"]=="message" && value["message"]["role"]=="user");
                                    if joined && incoming && input.send(Event::Frame(value)).await.is_err() { return; }
                                }
                            }
                        }
                        value = output.recv(), if pending.is_none() => {
                            let Some(value) = value else { return; };
                            // The task ledger is authoritative across disconnections.
                            if !joined || !browser { continue; }
                            if !send(&mut socket, &value).await { break; }
                            pending = Some((value, tokio::time::Instant::now()));
                        }
                    }
                }
                if input.send(Event::Offline).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
        Self {
            outgoing,
            incoming,
            task,
        }
    }
}
