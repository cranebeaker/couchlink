//! Converts a folder of pulled hub handlers (couch-research/tools/pull_hub_handlers.py) into draft recipes.
//!
//! cargo run -p couchlink-nucleus --example convert_all -- <hub-handlers dir> <out dir>
//!
//! Writes <out>/<id>.json per handler and <out>/index.json sorted by downloads.

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(src), Some(out)) = (args.next().map(PathBuf::from), args.next().map(PathBuf::from)) else {
        eprintln!("usage: convert_all <hub-handlers dir> <out dir>");
        std::process::exit(2);
    };
    fs::create_dir_all(&out).expect("create out dir");
    let mut index = Vec::new();
    let mut failed = 0;
    let mut invalid = Vec::new();
    for entry in fs::read_dir(&src).expect("read hub-handlers dir").flatten() {
        let dir = entry.path();
        let (Ok(meta), Ok(script)) = (fs::read_to_string(dir.join("handler.json")), fs::read(dir.join("handler.js"))) else {
            continue;
        };
        let script = String::from_utf8_lossy(&script);
        let handler: Value = serde_json::from_str(&meta).expect("handler.json");
        let bundle = json!({ "handler": handler, "script": script });
        let row = &handler["Handlers"][0];
        let id = row["_id"].as_str().unwrap_or_default().to_string();
        match couchlink_nucleus::to_recipe(&bundle.to_string()) {
            Ok(d) => {
                let valid = couchlink_core::Recipe::parse(&d.recipe.to_string());
                if let Err(e) = &valid {
                    invalid.push(format!("{id} {}: {e}", row["gameName"].as_str().unwrap_or("?")));
                }
                let doc = json!({ "recipe": d.recipe, "notes": d.notes, "confidence": d.confidence });
                fs::write(out.join(format!("{id}.json")), serde_json::to_string_pretty(&doc).unwrap()).expect("write draft");
                index.push(json!({
                    "id": id,
                    "game": row["gameName"],
                    "title": row["title"],
                    "downloads": row["downloadCount"],
                    "verified": row["verified"],
                    "confidence": d.confidence,
                    "notes": d.notes.len(),
                    "steam": d.recipe["steam"],
                    "valid": valid.is_ok(),
                }));
            }
            Err(e) => {
                failed += 1;
                eprintln!("{id}: {e:?}");
            }
        }
    }
    index.sort_by_key(|v| std::cmp::Reverse(v["downloads"].as_u64().unwrap_or(0)));
    let high = index.iter().filter(|v| v["confidence"].as_f64().unwrap_or(0.0) >= 0.8).count();
    let mid = index.iter().filter(|v| (0.5..0.8).contains(&v["confidence"].as_f64().unwrap_or(0.0))).count();
    fs::write(out.join("index.json"), serde_json::to_string_pretty(&index).unwrap()).expect("write index");
    for line in invalid.iter().take(15) {
        println!("invalid: {line}");
    }
    println!("{} of {} drafts load as Couchlink recipes", index.len() - invalid.len(), index.len());
    println!("{} converted ({high} high, {mid} medium, {} low), {failed} failed", index.len(), index.len() - high - mid);
}
