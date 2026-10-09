//! Paired browser clients, persisted at `<data_dir>/bridge-clients.json`.
//!
//! File format (written atomically, mode 0600 on Unix):
//! ```json
//! { "version": 1,
//!   "clients": [ { "id": "uuid", "name": "Chrome – PC", "tokenSha256": "<64 hex>",
//!                  "createdAt": 1700000000000, "lastSeenAt": 1700000000000 } ] }
//! ```
//! Only the SHA-256 (hex) of the UTF-8 token string is stored; the token
//! itself (32 random bytes, base64url without padding) is handed to the
//! extension once at pairing time.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use data_encoding::{BASE64URL_NOPAD, HEXLOWER, HEXLOWER_PERMISSIVE};
use keystead_core::crypto;
use keystead_core::model::now_ms;
use keystead_core::paths;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::util::{log, write_atomic};

/// File name inside the data directory.
pub const CLIENTS_FILE: &str = "bridge-clients.json";
const FORMAT_VERSION: u32 = 1;
/// `lastSeenAt` is kept current in memory; it is written to disk at most
/// this often (every request touches it).
const LAST_SEEN_PERSIST_INTERVAL: Duration = Duration::from_secs(60);

/// A paired client as shown in the app (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedClient {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredClient {
    id: String,
    name: String,
    token_sha256: String,
    created_at: i64,
    last_seen_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientsFile {
    version: u32,
    #[serde(default)]
    clients: Vec<StoredClient>,
}

/// The paired-client registry. Not internally synchronised – the
/// [`Dispatcher`](crate::dispatcher::Dispatcher) keeps it behind a mutex.
#[derive(Debug)]
pub struct ClientStore {
    path: PathBuf,
    clients: Vec<StoredClient>,
    /// Unsaved `lastSeenAt` changes and when they were last written.
    dirty: bool,
    last_flush: Option<Instant>,
}

impl ClientStore {
    /// `<data_dir>/bridge-clients.json`.
    pub fn default_path() -> PathBuf {
        paths::data_dir().join(CLIENTS_FILE)
    }

    /// Opens the store at [`ClientStore::default_path`].
    pub fn open_default() -> Result<ClientStore> {
        ClientStore::open(ClientStore::default_path())
    }

    /// Opens (or, if missing, starts) the store at `path`. A corrupt file is
    /// moved aside to `<file>.corrupt` and the store starts empty – clients
    /// then simply have to pair again. Other read errors are returned.
    pub fn open(path: impl Into<PathBuf>) -> Result<ClientStore> {
        let path = path.into();
        let clients = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<ClientsFile>(&bytes) {
                Ok(file) => file
                    .clients
                    .into_iter()
                    .filter(|c| decode_hash(&c.token_sha256).is_some() && !c.id.is_empty())
                    .collect(),
                Err(e) => {
                    let mut aside = path.as_os_str().to_owned();
                    aside.push(".corrupt");
                    log(format_args!(
                        "{} is unreadable ({e}); moving it aside, browsers must pair again",
                        path.display()
                    ));
                    fs::rename(&path, PathBuf::from(aside))?;
                    Vec::new()
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(ClientStore {
            path,
            clients,
            dirty: false,
            last_flush: None,
        })
    }

    /// Location of the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All paired clients, oldest first.
    pub fn list(&self) -> Vec<PairedClient> {
        self.clients
            .iter()
            .map(|c| PairedClient {
                id: c.id.clone(),
                name: c.name.clone(),
                created_at: c.created_at,
                last_seen_at: c.last_seen_at,
            })
            .collect()
    }

    /// Registers a new client and persists it. Returns the client and its
    /// token (the only time the token exists outside the extension).
    pub fn add(&mut self, name: &str) -> Result<(PairedClient, String)> {
        let token = BASE64URL_NOPAD.encode(&crypto::random_array::<32>()?);
        let now = now_ms();
        let stored = StoredClient {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_owned(),
            token_sha256: HEXLOWER.encode(&token_hash(&token)),
            created_at: now,
            last_seen_at: now,
        };
        self.clients.push(stored.clone());
        if let Err(e) = self.save() {
            self.clients.pop();
            return Err(e);
        }
        let client = PairedClient {
            id: stored.id,
            name: stored.name,
            created_at: stored.created_at,
            last_seen_at: stored.last_seen_at,
        };
        Ok((client, token))
    }

    /// True if `client_id` exists and `token` matches its stored hash
    /// (constant-time comparison of the SHA-256 digests).
    pub fn verify(&self, client_id: &str, token: &str) -> bool {
        let presented = token_hash(token);
        let Some(stored) = self.clients.iter().find(|c| c.id == client_id) else {
            return false;
        };
        decode_hash(&stored.token_sha256)
            .is_some_and(|expected| crypto::ct_eq(&expected, &presented))
    }

    /// Updates `lastSeenAt` of a client. Written to disk at most once per
    /// minute (see [`ClientStore::flush`]).
    pub fn touch(&mut self, client_id: &str) -> Result<()> {
        let Some(client) = self.clients.iter_mut().find(|c| c.id == client_id) else {
            return Ok(());
        };
        client.last_seen_at = now_ms();
        self.dirty = true;
        let due = self
            .last_flush
            .is_none_or(|t| t.elapsed() >= LAST_SEEN_PERSIST_INTERVAL);
        if due {
            self.save()?;
        }
        Ok(())
    }

    /// Writes pending `lastSeenAt` updates.
    pub fn flush(&mut self) -> Result<()> {
        if self.dirty {
            self.save()?;
        }
        Ok(())
    }

    /// Removes a client; its token stops working immediately. Returns
    /// whether it existed.
    pub fn revoke(&mut self, client_id: &str) -> Result<bool> {
        let Some(pos) = self.clients.iter().position(|c| c.id == client_id) else {
            return Ok(false);
        };
        let removed = self.clients.remove(pos);
        if let Err(e) = self.save() {
            self.clients.insert(pos, removed);
            return Err(e);
        }
        Ok(true)
    }

    fn save(&mut self) -> Result<()> {
        let file = ClientsFile {
            version: FORMAT_VERSION,
            clients: self.clients.clone(),
        };
        let json = serde_json::to_vec_pretty(&file)?;
        write_atomic(&self.path, &json, true)?;
        self.dirty = false;
        self.last_flush = Some(Instant::now());
        Ok(())
    }
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn decode_hash(hex: &str) -> Option<Vec<u8>> {
    HEXLOWER_PERMISSIVE
        .decode(hex.as_bytes())
        .ok()
        .filter(|h| h.len() == 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_verify_persist_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENTS_FILE);
        let mut store = ClientStore::open(&path).unwrap();
        assert!(store.list().is_empty());

        let (client, token) = store.add("Chrome – PC").unwrap();
        assert_eq!(token.len(), 43, "32 bytes base64url without padding");
        assert!(!token.contains('=') && !token.contains('+') && !token.contains('/'));
        assert!(store.verify(&client.id, &token));
        assert!(!store.verify(&client.id, "wrong"));
        assert!(!store.verify("unknown", &token));

        // The file stores only the hash.
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(&token));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["version"], 1);
        let entry = &json["clients"][0];
        assert_eq!(entry["id"], client.id.as_str());
        assert_eq!(entry["name"], "Chrome – PC");
        assert_eq!(entry["tokenSha256"].as_str().unwrap().len(), 64);
        assert_eq!(
            entry["tokenSha256"].as_str().unwrap(),
            HEXLOWER.encode(&Sha256::digest(token.as_bytes()))
        );
        assert!(entry["createdAt"].as_i64().unwrap() > 0);
        assert!(entry["lastSeenAt"].as_i64().unwrap() > 0);

        // Reopen.
        let mut store = ClientStore::open(&path).unwrap();
        assert_eq!(store.list(), vec![client.clone()]);
        assert!(store.verify(&client.id, &token));

        assert!(store.revoke(&client.id).unwrap());
        assert!(!store.revoke(&client.id).unwrap());
        assert!(!store.verify(&client.id, &token));
        assert!(ClientStore::open(&path).unwrap().list().is_empty());
    }

    #[test]
    fn touch_updates_last_seen_and_throttles_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENTS_FILE);
        let mut store = ClientStore::open(&path).unwrap();
        let (client, _) = store.add("x").unwrap();
        let mut stored = store.clients[0].clone();
        stored.last_seen_at = 1;
        store.clients[0] = stored;
        store.last_flush = None;
        store.touch(&client.id).unwrap();
        let seen = store.list()[0].last_seen_at;
        assert!(seen > 1);
        // Within the interval: memory only.
        store.clients[0].last_seen_at = 5;
        store.touch(&client.id).unwrap();
        assert!(store.dirty);
        let on_disk = ClientStore::open(&path).unwrap().list()[0].last_seen_at;
        assert_eq!(on_disk, seen);
        store.flush().unwrap();
        assert!(!store.dirty);
        assert!(ClientStore::open(&path).unwrap().list()[0].last_seen_at >= seen);
        // Unknown ids are ignored.
        store.touch("nope").unwrap();
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENTS_FILE);
        fs::write(&path, b"{ not json").unwrap();
        let store = ClientStore::open(&path).unwrap();
        assert!(store.list().is_empty());
        assert!(dir.path().join("bridge-clients.json.corrupt").exists());
        assert!(!path.exists());
    }

    #[test]
    fn invalid_entries_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLIENTS_FILE);
        let good = "ab".repeat(32);
        fs::write(
            &path,
            format!(
                r#"{{"version":1,"clients":[
                {{"id":"a","name":"ok","tokenSha256":"{good}","createdAt":1,"lastSeenAt":2}},
                {{"id":"b","name":"bad","tokenSha256":"xyz","createdAt":1,"lastSeenAt":2}}]}}"#
            ),
        )
        .unwrap();
        let store = ClientStore::open(&path).unwrap();
        let ids: Vec<_> = store.list().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["a"]);
    }
}
