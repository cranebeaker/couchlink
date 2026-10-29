//! couchlink-sign: make the recipe signing key, sign a staging folder, check a signed folder.
//!
//!   couchlink-sign keygen [key path]
//!   couchlink-sign sign <staging folder> <out folder> [--key PATH] [--sequence N]
//!   couchlink-sign verify <signed folder> --pubkey HEX
//!
//! The private key lives outside every repository (default %USERPROFILE%\.couchlink-signing\recipes.key)
//! and is never printed.

use std::path::PathBuf;

fn default_key() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default();
    home.join(".couchlink-signing").join("recipes.key")
}

fn usage() -> ! {
    eprintln!("usage: couchlink-sign keygen [KEY] | sign STAGING OUT [--key KEY] [--sequence N] | verify DIR --pubkey HEX");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => {
            let path = args.get(1).filter(|a| !a.starts_with("--")).map(PathBuf::from).unwrap_or_else(default_key);
            couchlink_sign::keygen(&path).map(|public| format!("public key {public}\nprivate key written to {}", path.display()))
        }
        Some("sign") => {
            let (Some(staging), Some(out)) = (args.get(1), args.get(2)) else { usage() };
            let key_path = flag("--key").map(PathBuf::from).unwrap_or_else(default_key);
            let out = PathBuf::from(out);
            // Default sequence: one more than the folder already holds, so republishing never goes backwards.
            let previous = std::fs::read(out.join(couchlink_sign::INDEX_FILE))
                .ok()
                .and_then(|b| serde_json::from_slice::<couchlink_sign::Index>(&b).ok())
                .map_or(0, |i| i.sequence);
            let sequence = flag("--sequence").and_then(|s| s.parse().ok()).unwrap_or(previous + 1);
            let generated = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string();
            couchlink_sign::load_key(&key_path)
                .and_then(|key| couchlink_sign::sign_folder(&PathBuf::from(staging), &out, &key, sequence, &generated))
                .map(|i| format!("signed {} recipes, sequence {}", i.recipes.len(), i.sequence))
        }
        Some("verify") => {
            let Some(dir) = args.get(1) else { usage() };
            let Some(hex) = flag("--pubkey") else { usage() };
            couchlink_sign::parse_public_key(&hex)
                .and_then(|k| couchlink_sign::verify_folder(&PathBuf::from(dir), &k))
                .map(|i| format!("OK: {} recipes, sequence {}", i.recipes.len(), i.sequence))
        }
        _ => usage(),
    };
    match result {
        Ok(msg) => println!("{msg}"),
        Err(e) => {
            eprintln!("couchlink-sign: {e}");
            std::process::exit(1);
        }
    }
}
