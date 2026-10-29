//! WebSocket server on 127.0.0.1.

use crate::hub::{hello_reply, HubMsg};
use couchlink_core::{Command, Reply, Request};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::Message;

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Accept connections forever.
pub async fn serve(listener: TcpListener, token: Arc<String>, hub: UnboundedSender<HubMsg>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let token = token.clone();
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(e) = client(stream, token, hub).await {
                let _ = e; // handshake failures (port probes) are not interesting
            }
        });
    }
}

async fn client(stream: TcpStream, token: Arc<String>, hub: UnboundedSender<HubMsg>) -> Result<(), String> {
    let _ = stream.set_nodelay(true);
    let ws = tokio_tungstenite::accept_async(stream).await.map_err(|e| e.to_string())?;
    let (mut sink, mut source) = ws.split();

    // First frame must be a hello carrying the right token.
    let first = tokio::time::timeout(HELLO_TIMEOUT, async {
        loop {
            match source.next().await {
                Some(Ok(Message::Text(t))) => return Some(t.to_string()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    let Some(first) = first else {
        let _ = sink.close().await;
        return Err("no hello".into());
    };
    let hello_id = match Request::parse(&first) {
        Ok(Request { id, cmd: Command::Hello { token: t, .. } }) if constant_time_eq(&t, &token) => id,
        Ok(Request { id, .. }) => {
            let _ = sink.send(Message::text(Reply::err(id, "first frame must be hello with the right token").to_json())).await;
            let _ = sink.close().await;
            return Err("bad hello".into());
        }
        Err((id, msg)) => {
            if let Some(id) = id {
                let _ = sink.send(Message::text(Reply::err(id, msg).to_json())).await;
            }
            let _ = sink.close().await;
            return Err("bad hello".into());
        }
    };

    let client_id = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = unbounded_channel::<String>();
    // The hello reply goes through the same channel as everything else so the
    // initial state event is guaranteed to follow it.
    let _ = tx.send(hello_reply(hello_id).to_json());
    let _ = hub.send(HubMsg::Connect { client: client_id, tx: tx.clone() });

    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    while let Some(frame) = source.next().await {
        match frame {
            Ok(Message::Text(t)) => match Request::parse(&t) {
                Ok(req) => {
                    let _ = hub.send(HubMsg::Request { client: client_id, req });
                }
                Err((Some(id), msg)) => {
                    let _ = tx.send(Reply::err(id, msg).to_json());
                }
                Err((None, msg)) => {
                    let _ = tx.send(couchlink_core::Event::log(couchlink_core::protocol::LogLevel::Warn, msg).to_json());
                }
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = hub.send(HubMsg::Disconnect { client: client_id });
    drop(tx);
    writer.abort();
    Ok(())
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
