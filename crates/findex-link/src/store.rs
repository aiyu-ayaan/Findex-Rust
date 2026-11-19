//! Paired devices, kept in `peers.json` next to the identity (owner-only permissions).

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::identity::write_private;
use crate::sas;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Hex SHA-256 of the peer's certificate; pinned on every connection.
    pub id: String,
    pub name: String,
    pub platform: String,
    /// Last address it was seen at.
    pub host: String,
    pub port: u16,
    /// Bearer token we present to the peer.
    pub token_out: String,
    /// Bearer token the peer presents to us.
    pub token_in: String,
}

impl Peer {
    fn to_json(&self) -> Value {
        json!({"id": self.id, "name": self.name, "platform": self.platform, "host": self.host,
               "port": self.port, "tokenOut": self.token_out, "tokenIn": self.token_in})
    }
    fn from_json(v: &Value) -> Option<Peer> {
        let s = |k: &str| v.get(k)?.as_str().map(str::to_string);
        Some(Peer {
            id: s("id")?,
            name: s("name")?,
            platform: s("platform").unwrap_or_default(),
            host: s("host").unwrap_or_default(),
            port: v.get("port")?.as_u64()? as u16,
            token_out: s("tokenOut")?,
            token_in: s("tokenIn")?,
        })
    }
    pub fn fingerprint(&self) -> Option<[u8; 32]> {
        sas::unhex(&self.id)?.try_into().ok()
    }
}

pub struct Store {
    path: Option<PathBuf>,
    peers: Mutex<Vec<Peer>>,
}

impl Store {
    /// `None` keeps peers in memory only (tests).
    pub fn open(path: Option<PathBuf>) -> Store {
        let peers = path
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| {
                v.as_array()
                    .map(|a| a.iter().filter_map(Peer::from_json).collect())
            })
            .unwrap_or_default();
        Store {
            path,
            peers: Mutex::new(peers),
        }
    }

    pub fn all(&self) -> Vec<Peer> {
        self.peers.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Peer> {
        self.all().into_iter().find(|p| p.id == id)
    }

    pub fn by_token(&self, token: &str) -> Option<Peer> {
        self.all()
            .into_iter()
            .find(|p| sas::same(&p.token_in, token))
    }

    pub fn upsert(&self, peer: Peer) -> io::Result<()> {
        let mut peers = self.peers.lock().unwrap();
        peers.retain(|p| p.id != peer.id);
        peers.push(peer);
        self.save(&peers)
    }

    /// Records the address a peer last talked to us from. No write when it did not change.
    pub fn touch(&self, id: &str, host: &str) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(p) = peers.iter_mut().find(|p| p.id == id && p.host != host) {
            p.host = host.to_string();
            let _ = self.save(&peers);
        }
    }

    pub fn remove(&self, id: &str) -> io::Result<()> {
        let mut peers = self.peers.lock().unwrap();
        peers.retain(|p| p.id != id);
        self.save(&peers)
    }

    fn save(&self, peers: &[Peer]) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let v = Value::Array(peers.iter().map(Peer::to_json).collect());
        write_private(path, serde_json::to_string_pretty(&v)?.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_peers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let peer = Peer {
            id: "ab".repeat(32),
            name: "Pixel".into(),
            platform: "android".into(),
            host: "10.0.0.2".into(),
            port: 47100,
            token_out: "o".into(),
            token_in: "i".into(),
        };
        Store::open(Some(path.clone()))
            .upsert(peer.clone())
            .unwrap();
        let s = Store::open(Some(path));
        assert_eq!(s.all(), vec![peer.clone()]);
        assert_eq!(s.by_token("i"), Some(peer));
        assert!(s.by_token("o").is_none());
    }
}
