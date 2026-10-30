//! The program Steam itself starts for a game: `config/launch` in Steam's `appcache/appinfo.vdf` (binary VDF).
//! Read-only. Versions 27 to 29 of the file are read; anything else gives no answer and Couchlink guesses instead.

use std::collections::HashMap;
use std::path::Path;

const V27: u32 = 0x0756_4427;
const V28: u32 = 0x0756_4428;
const V29: u32 = 0x0756_4429;

/// One `config/launch` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Launch {
    /// Relative to the game folder, forward slashes, no leading `./`.
    pub executable: String,
    pub arguments: String,
    /// `default`, `option1`, `server`, `editor`... (empty when Steam leaves it out).
    pub kind: String,
    /// `windows`, `linux`, `macos`, comma separated; empty means every OS.
    pub oslist: String,
    pub osarch: String,
    /// Only for a beta branch.
    pub betakey: String,
}

#[derive(Debug)]
enum Node {
    Map(Vec<(String, Node)>),
    Str(String),
    Other,
}

impl Node {
    fn get(&self, key: &str) -> Option<&Node> {
        match self {
            Node::Map(kv) => kv.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self, key: &str) -> String {
        match self.get(key) {
            Some(Node::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn cstr(&mut self) -> Option<String> {
        let rest = self.b.get(self.pos..)?;
        let end = rest.iter().position(|&c| c == 0)?;
        let s = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.pos += end + 1;
        Some(s)
    }
}

/// A binary VDF map up to its end byte. `keys` is the file's string table (version 29), else keys are inline.
fn read_map(r: &mut Reader, keys: Option<&[String]>, depth: u8) -> Option<Node> {
    if depth > 32 {
        return None;
    }
    let mut out = Vec::new();
    loop {
        let t = r.u8()?;
        if t == 0x08 || t == 0x0b {
            return Some(Node::Map(out));
        }
        let key = match keys {
            Some(table) => table.get(r.u32()? as usize)?.clone(),
            None => r.cstr()?,
        };
        let v = match t {
            0x00 => read_map(r, keys, depth + 1)?,
            0x01 => Node::Str(r.cstr()?),
            0x02 | 0x03 | 0x04 | 0x06 => {
                r.take(4)?;
                Node::Other
            }
            0x07 | 0x0a => {
                r.take(8)?;
                Node::Other
            }
            _ => return None,
        };
        out.push((key, v));
    }
}

fn clean_path(p: &str) -> String {
    let p = p.trim().replace('\\', "/");
    let mut p = p.as_str();
    while let Some(rest) = p.strip_prefix("./").or_else(|| p.strip_prefix('/')) {
        p = rest;
    }
    p.to_string()
}

fn launches_of(app: &Node) -> Vec<Launch> {
    let Some(Node::Map(entries)) = app.get("config").and_then(|c| c.get("launch")) else { return Vec::new() };
    entries
        .iter()
        .filter_map(|(_, l)| {
            let exe = clean_path(&l.str("executable"));
            if exe.is_empty() {
                return None;
            }
            let cfg = l.get("config");
            let c = |k: &str| cfg.map(|c| c.str(k)).unwrap_or_default();
            Some(Launch { executable: exe, arguments: l.str("arguments"), kind: l.str("type"), oslist: c("oslist"), osarch: c("osarch"), betakey: c("betakey") })
        })
        .collect()
}

/// The `config/launch` entries of these apps from the bytes of an `appinfo.vdf`. Apps not asked for are skipped
/// without being read.
pub fn parse(bytes: &[u8], wanted: &[u32]) -> HashMap<u32, Vec<Launch>> {
    let mut out = HashMap::new();
    let mut r = Reader { b: bytes, pos: 0 };
    let Some(magic) = r.u32() else { return out };
    if ![V27, V28, V29].contains(&magic) || r.u32().is_none() {
        return out;
    }
    let table: Option<Vec<String>> = if magic == V29 {
        let Some(at) = r.u64() else { return out };
        let mut t = Reader { b: bytes, pos: at as usize };
        let Some(n) = t.u32() else { return out };
        let mut v = Vec::with_capacity(n.min(1 << 20) as usize);
        for _ in 0..n {
            let Some(s) = t.cstr() else { return out };
            v.push(s);
        }
        Some(v)
    } else {
        None
    };
    // After the size: state, last update, PICS token, SHA-1, change number, and from version 28 the binary SHA-1.
    let fixed = if magic == V27 { 40 } else { 60 };
    while let Some(app_id) = r.u32() {
        if app_id == 0 {
            break;
        }
        let Some(size) = r.u32() else { break };
        let end = r.pos + size as usize;
        if end > bytes.len() {
            break;
        }
        if wanted.contains(&app_id) {
            let mut e = Reader { b: &bytes[..end], pos: r.pos + fixed };
            // The entry is one map: a type byte, a key ("appinfo"), then its children.
            let node = (|| {
                if e.u8()? != 0 {
                    return None;
                }
                match table.as_deref() {
                    Some(_) => {
                        e.u32()?;
                    }
                    None => {
                        e.cstr()?;
                    }
                }
                read_map(&mut e, table.as_deref(), 0)
            })();
            if let Some(n) = node {
                out.insert(app_id, launches_of(&n));
            }
        }
        r.pos = end;
    }
    out
}

/// The program Steam starts for this app on `os` (`windows` or `linux`): its main launch option, not a beta's, a
/// server's or an editor's. `None` when Steam lists none for that OS.
pub fn pick(launches: &[Launch], os: &str) -> Option<String> {
    let rank = |l: &Launch| -> Option<u8> {
        let os_ok = l.oslist.is_empty() || l.oslist.split(',').any(|o| o.trim().eq_ignore_ascii_case(os));
        if !os_ok || !l.betakey.is_empty() {
            return None;
        }
        if os == "windows" && !l.executable.to_lowercase().ends_with(".exe") {
            return None;
        }
        let kind = match l.kind.to_lowercase().as_str() {
            "" | "default" => 30,
            "option1" => 20,
            "none" | "option2" | "option3" => 10,
            _ => return None,
        };
        // A 64-bit build first, then one for every OS before one for this OS only.
        Some(kind + if l.osarch == "64" { 5 } else { 0 } + if l.oslist.is_empty() { 0 } else { 1 })
    };
    let mut best: Option<(u8, &Launch)> = None;
    for l in launches {
        if let Some(s) = rank(l) {
            if best.is_none_or(|(b, _)| s > b) {
                best = Some((s, l));
            }
        }
    }
    best.map(|(_, l)| l.executable.clone())
}

/// The program Steam starts for each of these apps on `os`, from the Steam install's `appcache/appinfo.vdf`.
pub fn launch_exes(steam_root: &Path, app_ids: &[u32], os: &str) -> HashMap<u32, String> {
    let Ok(bytes) = std::fs::read(steam_root.join("appcache").join("appinfo.vdf")) else { return HashMap::new() };
    parse(&bytes, app_ids).into_iter().filter_map(|(id, l)| Some((id, pick(&l, os)?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version 29 file with one app, built the way Steam writes it.
    fn sample(app_id: u32) -> Vec<u8> {
        let keys = ["appinfo", "config", "launch", "0", "1", "executable", "type", "oslist", "arguments", "betakey"];
        let k = |s: &str| (keys.iter().position(|x| *x == s).unwrap() as u32).to_le_bytes();
        let mut vdf = Vec::new();
        let map = |v: &mut Vec<u8>, key: &str| {
            v.push(0);
            v.extend(k(key));
        };
        let s = |v: &mut Vec<u8>, key: &str, val: &str| {
            v.push(1);
            v.extend(k(key));
            v.extend(val.as_bytes());
            v.push(0);
        };
        map(&mut vdf, "appinfo");
        map(&mut vdf, "config");
        map(&mut vdf, "launch");
        map(&mut vdf, "0");
        s(&mut vdf, "executable", "tools\\editor.exe");
        s(&mut vdf, "type", "editor");
        vdf.push(8);
        map(&mut vdf, "1");
        s(&mut vdf, "executable", ".\\DyingLightGame.exe");
        s(&mut vdf, "type", "default");
        s(&mut vdf, "arguments", "-nologos");
        map(&mut vdf, "config");
        s(&mut vdf, "oslist", "windows");
        vdf.push(8);
        vdf.push(8);
        vdf.extend([8, 8, 8, 8]);
        let mut entry = Vec::new();
        entry.extend(app_id.to_le_bytes());
        entry.extend(((60 + vdf.len()) as u32).to_le_bytes());
        entry.extend([0u8; 60]);
        entry.extend(&vdf);
        let mut file = Vec::new();
        file.extend(V29.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        let table_at = 16 + entry.len() + 4;
        file.extend((table_at as u64).to_le_bytes());
        file.extend(&entry);
        file.extend(0u32.to_le_bytes());
        file.extend((keys.len() as u32).to_le_bytes());
        for key in keys {
            file.extend(key.as_bytes());
            file.push(0);
        }
        file
    }

    #[test]
    fn reads_the_main_launch_option() {
        let got = parse(&sample(239140), &[239140]);
        let l = &got[&239140];
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].executable, "DyingLightGame.exe");
        assert_eq!(l[1].arguments, "-nologos");
        assert_eq!(pick(l, "windows").as_deref(), Some("DyingLightGame.exe"), "the editor is never the game");
        assert_eq!(pick(l, "linux"), None, "a Windows-only option is no Linux program");
        assert!(parse(&sample(239140), &[10]).is_empty(), "apps not asked for are skipped");
    }

    #[test]
    fn rubbish_gives_nothing() {
        assert!(parse(b"not a vdf at all", &[1]).is_empty());
        let mut cut = sample(5);
        cut.truncate(40);
        assert!(parse(&cut, &[5]).is_empty());
    }

    #[test]
    fn a_beta_or_server_option_is_skipped() {
        let l = |exe: &str, kind: &str, beta: &str| Launch { executable: exe.into(), kind: kind.into(), betakey: beta.into(), ..Default::default() };
        let all = [l("server.exe", "server", ""), l("beta.exe", "default", "public_beta"), l("game.exe", "option1", "")];
        assert_eq!(pick(&all, "windows").as_deref(), Some("game.exe"));
    }

    /// `cargo test -p couchlink-steam -- --ignored this_pc --nocapture` prints what Steam starts for each installed game.
    #[test]
    #[ignore]
    fn this_pc() {
        let root = crate::steam_root().expect("Steam is installed");
        let apps = crate::installed_apps();
        let ids: Vec<u32> = apps.iter().map(|a| a.app_id).collect();
        let exes = launch_exes(&root, &ids, "windows");
        for a in &apps {
            let exe = exes.get(&a.app_id);
            println!("{} {} -> {:?} exists={}", a.app_id, a.name, exe, exe.is_some_and(|e| a.install_dir.join(e).is_file()));
        }
    }
}
