//! clprobe: a tiny command line client for couchlinkd.
//!
//! usage: clprobe [--join DEVICE] [--press DEVICE BUTTON] [--raw JSON] [--catalog] [--watch]
//! Prints the state, runs the actions in order, optionally prints events forever.

use couchlink_core::paths;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

enum Action {
    Join(String),
    Press(String, String),
    Raw(String),
    Catalog,
}

fn usage() -> ! {
    eprintln!("usage: clprobe [--join DEVICE] [--press DEVICE BUTTON] [--raw JSON] [--catalog] [--watch]");
    eprintln!("  --raw sends any request object (the id is filled in), e.g. --raw '{{\"cmd\":\"leave\",\"seat\":0}}'");
    eprintln!("  --catalog prints the game catalog: entry count, fetch time, first 3 titles");
    std::process::exit(2);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut actions = Vec::new();
    let mut watch = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--join" => actions.push(Action::Join(it.next().unwrap_or_else(|| usage()))),
            "--press" => {
                let dev = it.next().unwrap_or_else(|| usage());
                let btn = it.next().unwrap_or_else(|| usage());
                actions.push(Action::Press(dev, btn));
            }
            "--raw" => actions.push(Action::Raw(it.next().unwrap_or_else(|| usage()))),
            "--catalog" => actions.push(Action::Catalog),
            "--watch" => watch = true,
            _ => usage(),
        }
    }
    if let Err(e) = run(actions, watch).await {
        eprintln!("clprobe: {e}");
        std::process::exit(1);
    }
}

async fn run(actions: Vec<Action>, watch: bool) -> Result<(), String> {
    let info = paths::read_daemon_info()
        .ok_or_else(|| format!("no daemon.json in {} (is couchlinkd running?)", paths::state_dir().display()))?;
    let url = format!("ws://127.0.0.1:{}/", info.port);
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.map_err(|e| format!("cannot connect to {url}: {e}"))?;
    let (mut sink, mut source) = ws.split();

    let hello = json!({"id": 1, "cmd": "hello", "token": info.token, "client": "clprobe 0.1.0"});
    sink.send(Message::text(hello.to_string())).await.map_err(|e| e.to_string())?;

    let mut next_id = 2u64;
    let mut pending: Vec<u64> = vec![1];
    let mut sent_actions = false;
    let mut printed_state = false;

    while let Some(frame) = source.next().await {
        let text = match frame.map_err(|e| e.to_string())? {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => return Err("daemon closed the connection".into()),
            _ => continue,
        };
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if v.get("event").is_some() {
            if v["event"] == "state" && !printed_state {
                printed_state = true;
                println!("{}", serde_json::to_string_pretty(&v["state"]).unwrap_or_default());
                if !sent_actions {
                    sent_actions = true;
                    for a in &actions {
                        let id = next_id;
                        next_id += 1;
                        pending.push(id);
                        let req = match a {
                            Action::Join(d) => json!({"id": id, "cmd": "join", "device": d}),
                            Action::Press(d, b) => json!({"id": id, "cmd": "debug_press", "device": d, "button": b}),
                            Action::Catalog => json!({"id": id, "cmd": "get_catalog"}),
                            Action::Raw(text) => {
                                let mut v: Value = serde_json::from_str(text).map_err(|e| format!("bad --raw json: {e}"))?;
                                v["id"] = id.into();
                                v
                            }
                        };
                        sink.send(Message::text(req.to_string())).await.map_err(|e| e.to_string())?;
                    }
                }
            } else if watch {
                println!("{text}");
            }
        } else if let Some(id) = v.get("id").and_then(Value::as_u64) {
            if v.get("catalog").is_some() {
                print_catalog(&v);
            } else if id != 1 {
                println!("{text}");
            }
            pending.retain(|p| *p != id);
        }
        if sent_actions && pending.is_empty() && !watch {
            break;
        }
    }
    let _ = sink.close().await;
    Ok(())
}

/// `--catalog`: a short summary instead of the whole reply.
fn print_catalog(reply: &Value) {
    let cat = &reply["catalog"];
    let entries = cat["entries"].as_array().map(Vec::as_slice).unwrap_or_default();
    println!("catalog: {} entries, snapshot {}, fetched {}, refreshing {}", entries.len(), cat["snapshot"], cat["fetched"], reply["refreshing"]);
    for e in entries.iter().take(3) {
        println!("  {} ({}, {}%)", e["title"].as_str().unwrap_or("?"), e["project"].as_str().unwrap_or("?"), e["coopPct"]);
    }
}
