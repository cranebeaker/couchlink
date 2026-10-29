//! Checks recipe files against the runner's own parser.
//!
//! cargo run -q -p couchlink-nucleus --example check_recipe -- <file.json>...
//!
//! A file may hold a bare recipe or `{"recipe": {...}, ...}`. Prints OK or the error per file and exits
//! non-zero if any failed.

use serde_json::Value;

fn main() {
    let mut bad = 0;
    for path in std::env::args().skip(1) {
        let result = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
            let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            let recipe = v.get("recipe").cloned().unwrap_or(v);
            couchlink_core::Recipe::parse(&recipe.to_string()).map(|_| ())
        });
        match result {
            Ok(()) => println!("OK {path}"),
            Err(e) => {
                bad += 1;
                println!("ERROR {path}: {e}");
            }
        }
    }
    std::process::exit(if bad == 0 { 0 } else { 1 });
}
