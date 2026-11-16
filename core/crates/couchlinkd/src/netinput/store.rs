//! `<state>/remote_input.json`: this PC's id, the senders paired with it and the PCs it sends to.

use couchlink_remote::session::Peer;
use couchlink_remote::{hex, unhex};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPeer {
    pub id: String,
    pub name: String,
    pub key: String,
    /// The last address a receiving PC answered from (senders only), to find it without a broadcast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    /// This PC's id (hex), the same whether it sends or receives.
    #[serde(default)]
    pub id: String,
    /// PCs allowed to send input to this one.
    #[serde(default)]
    pub senders: Vec<StoredPeer>,
    /// PCs this one sends its input to.
    #[serde(default)]
    pub hosts: Vec<StoredPeer>,
}

impl StoredPeer {
    pub fn from_peer(p: &Peer, addr: Option<String>) -> StoredPeer {
        StoredPeer { id: hex(&p.id), name: p.name.clone(), key: hex(&p.key), addr }
    }

    pub fn peer(&self) -> Option<Peer> {
        Some(Peer { id: unhex(&self.id)?, name: self.name.clone(), key: unhex(&self.key)? })
    }
}

pub fn file(state_dir: &Path) -> PathBuf {
    state_dir.join("remote_input.json")
}

impl Store {
    /// The store, with an id made (and saved) the first time.
    pub fn load(state_dir: &Path) -> Store {
        let mut s: Store = std::fs::read(file(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if unhex::<8>(&s.id).is_none() {
            s.id = hex(&couchlink_remote::random::<8>());
            let _ = s.save(state_dir);
        }
        s
    }

    pub fn save(&self, state_dir: &Path) -> Result<(), String> {
        let _ = std::fs::create_dir_all(state_dir);
        let path = file(state_dir);
        let tmp = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn id(&self) -> [u8; 8] {
        unhex(&self.id).unwrap_or([0; 8])
    }

    pub fn sender_peers(&self) -> Vec<Peer> {
        self.senders.iter().filter_map(StoredPeer::peer).collect()
    }

    /// Adds or replaces a peer in `list` (by id).
    pub fn upsert(list: &mut Vec<StoredPeer>, p: StoredPeer) {
        list.retain(|o| o.id != p.id);
        list.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_keeps_its_id_and_peers() {
        let dir = std::env::temp_dir().join(format!("couchlink-netinput-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir);
        assert_eq!(s.id.len(), 16);
        let peer = Peer { id: [1; 8], name: "Laptop".into(), key: [9; 32] };
        Store::upsert(&mut s.senders, StoredPeer::from_peer(&peer, None));
        Store::upsert(&mut s.senders, StoredPeer::from_peer(&peer, None));
        s.save(&dir).unwrap();
        let again = Store::load(&dir);
        assert_eq!(again.id, s.id);
        assert_eq!(again.sender_peers(), vec![peer]);
        std::fs::write(file(&dir), "not json").unwrap();
        assert_eq!(Store::load(&dir).senders.len(), 0, "a broken file starts over");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
