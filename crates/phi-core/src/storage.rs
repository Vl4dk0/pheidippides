use crate::{Result, user};
use phi_crypto::{PinnedTrustState, VerifiedDirectory, verify_trust_bundle};
use phi_proto::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub server: String,
    pub space_id: SpaceId,
    pub person_id: PersonId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub display_name: String,
    pub kind: DeviceKind,
    pub summarizer: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub server: String,
    pub space_id: SpaceId,
}
#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
    pub cache_root: PathBuf,
}
impl Store {
    pub fn platform() -> Result<Self> {
        let base = directories::BaseDirs::new()
            .ok_or_else(|| user("cannot find platform config directory"))?;
        Ok(Self {
            root: base.config_dir().join("pheidippides"),
            cache_root: base.cache_dir().join("pheidippides"),
        })
    }
    pub fn new(root: impl Into<PathBuf>, cache_root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cache_root: cache_root.into(),
        }
    }
    pub fn profile_dir(&self, profile: &Profile) -> PathBuf {
        let hash = blake3::hash(profile.server.as_bytes()).to_hex();
        self.root
            .join("profiles")
            .join(format!("{}-{}", &hash[..12], profile.space_id))
    }
    pub fn cache_dir(&self, profile: &Profile) -> PathBuf {
        self.cache_root
            .join(self.profile_dir(profile).file_name().unwrap_or_default())
    }
    pub fn profiles(&self) -> Result<Vec<Profile>> {
        let path = self.root.join("profiles");
        if !path.exists() {
            return Ok(vec![]);
        }
        let mut result: Vec<Profile> = Vec::new();
        for entry in fs::read_dir(path)? {
            let file = entry?.path().join("profile.toml");
            if file.is_file() {
                result.push(toml::from_str(&fs::read_to_string(file)?)?);
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
    pub fn save_profile(&self, profile: &Profile) -> Result<()> {
        let path = self.profile_dir(profile);
        private_dir(&path)?;
        atomic_write(
            &path.join("profile.toml"),
            toml::to_string_pretty(profile)?.as_bytes(),
        )
    }
    pub fn binding(cwd: &Path) -> Result<Option<Binding>> {
        for ancestor in cwd.ancestors() {
            let file = ancestor.join(".phi");
            if file.exists() {
                return Ok(Some(serde_json::from_slice(&fs::read(file)?)?));
            }
        }
        Ok(None)
    }
    pub fn resolve(&self, cwd: &Path, name: Option<&str>) -> Result<Profile> {
        let profiles = self.profiles()?;
        let matching: Vec<_> = if let Some(name) = name {
            profiles
                .iter()
                .filter(|p| p.name == name || p.space_id.to_string() == name)
                .collect()
        } else if let Some(binding) = Self::binding(cwd)? {
            profiles
                .iter()
                .filter(|p| p.space_id == binding.space_id && p.server == binding.server)
                .collect()
        } else {
            profiles.iter().collect()
        };
        if matching.len() == 1 {
            return Ok(matching[0].clone());
        }
        Err(user(format!(
            "select a space with phi init <space> or --space <name>; profiles: {}",
            profiles
                .iter()
                .map(|p| format!("{} ({})", p.name, p.space_id))
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
    pub fn bind(&self, cwd: &Path, name: &str) -> Result<Profile> {
        let profile = self.resolve(cwd, Some(name))?;
        atomic_write(
            &cwd.join(".phi"),
            &serde_json::to_vec_pretty(&Binding {
                server: profile.server.clone(),
                space_id: profile.space_id,
            })?,
        )?;
        Ok(profile)
    }
}

// GUI implementations can put these accounts in the OS keychain; plaintext cache stays separate.
/// Accounts are immutable: identical writes may succeed; conflicting writes must fail atomically.
pub trait KeyStore: Send {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>>;
    fn put(&self, account: &str, secret: &[u8]) -> Result<()>;
}
pub struct FileKeyStore {
    db: Connection,
}
impl FileKeyStore {
    pub fn open(path: &Path) -> Result<Self> {
        let db = private_database(path)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS secrets(account TEXT PRIMARY KEY, value BLOB NOT NULL)",
        )?;
        Ok(Self { db })
    }
}
impl KeyStore for FileKeyStore {
    fn get(&self, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(self
            .db
            .query_row(
                "SELECT value FROM secrets WHERE account=?",
                [account],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(Zeroizing::new))
    }
    fn put(&self, account: &str, secret: &[u8]) -> Result<()> {
        if secret.is_empty() {
            return Err(user("cannot store empty secret"));
        }
        let changed = self.db.execute("INSERT INTO secrets VALUES(?,?) ON CONFLICT(account) DO UPDATE SET value=excluded.value WHERE secrets.value=excluded.value", params![account, secret])?;
        if changed == 0 {
            return Err(user(
                "conflicting private key account; accepted keys cannot be replaced",
            ));
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
pub(crate) struct Identity {
    pub sign: KeyBytes,
    pub kex: KeyBytes,
    pub token: Base64<32>,
    pub certificate: DeviceCertificate,
}
impl Identity {
    pub(crate) fn keys(&self) -> phi_crypto::DeviceKeys {
        phi_crypto::DeviceKeys::from_secrets(*self.sign.as_bytes(), *self.kex.as_bytes())
    }
}
pub(crate) fn store_json<T: Serialize>(
    store: &dyn KeyStore,
    account: &str,
    value: &T,
) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(value)?);
    store.put(account, &bytes)
}
pub(crate) fn load_json<T: DeserializeOwned>(store: &dyn KeyStore, account: &str) -> Result<T> {
    let bytes = store
        .get(account)?
        .ok_or_else(|| user(format!("missing key account {account}; enroll again")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) struct Cache {
    pub db: Connection,
}
impl Cache {
    pub fn open(path: &Path) -> Result<Self> {
        let db = private_database(path)?;
        db.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS state(name TEXT PRIMARY KEY,value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, seq INTEGER UNIQUE NOT NULL, thread TEXT NOT NULL, channel TEXT NOT NULL, created INTEGER NOT NULL, raw TEXT NOT NULL, content TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread,seq);
        CREATE TABLE IF NOT EXISTS message_pins(id TEXT PRIMARY KEY,seq INTEGER UNIQUE NOT NULL,digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS reactions(message TEXT NOT NULL,person TEXT NOT NULL,handle TEXT NOT NULL,kind TEXT NOT NULL,raw TEXT NOT NULL,PRIMARY KEY(message,person,handle,kind));
        CREATE TABLE IF NOT EXISTS cursors(thread TEXT PRIMARY KEY,seq INTEGER NOT NULL);")?;
        Ok(Self { db })
    }
    pub fn get<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        self.db
            .query_row("SELECT value FROM state WHERE name=?", [name], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    pub fn set<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        self.db.execute(
            "INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=excluded.value",
            params![name, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
    pub fn trust(&mut self, sid: SpaceId, bundle: &TrustBundle) -> Result<VerifiedDirectory> {
        // Re-read pins inside the write transaction: concurrent CLI sessions must not overwrite newer trust.
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let prior: Option<String> = tx
            .query_row("SELECT value FROM state WHERE name='trust'", [], |r| {
                r.get(0)
            })
            .optional()?;
        let prior: Option<PinnedTrustState> =
            prior.map(|s| serde_json::from_str(&s)).transpose()?;
        let verified = verify_trust_bundle(sid, bundle, crate::now(), prior.as_ref())?;
        tx.execute("INSERT INTO state VALUES('trust',?) ON CONFLICT(name) DO UPDATE SET value=excluded.value", [serde_json::to_string(&verified.pinned_state())?])?;
        tx.commit()?;
        Ok(verified)
    }
    pub fn channel(&mut self, directory: &VerifiedDirectory, channel: &Channel) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let name = format!("roster/{}", channel.id);
        let prior: Option<String> = tx
            .query_row("SELECT value FROM state WHERE name=?", [&name], |r| {
                r.get(0)
            })
            .optional()?;
        let prior: Option<ChannelMembership> =
            prior.map(|s| serde_json::from_str(&s)).transpose()?;
        directory.verify_channel(channel, prior.as_ref())?;
        tx.execute(
            "INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=excluded.value",
            params![name, serde_json::to_string(&channel.membership)?],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn message(&mut self, message: &Message, content: &MessageContent) -> Result<()> {
        let raw = serde_json::to_string(message)?;
        let digest = blake3::hash(raw.as_bytes()).to_hex().to_string();
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let prior: Option<(String, u64, String)> = tx
            .query_row(
                "SELECT id,seq,digest FROM message_pins WHERE id=? OR seq=?",
                params![message.id.to_string(), message.seq],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if prior.is_some_and(|(id, seq, old)| {
            id != message.id.to_string() || seq != message.seq || old != digest
        }) {
            return Err(user(
                "server changed a previously observed message or sequence",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO message_pins VALUES(?,?,?)",
            params![message.id.to_string(), message.seq, digest],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO messages VALUES(?,?,?,?,?,?,?)",
            params![
                message.id.to_string(),
                message.seq,
                message.envelope.thread_id.unwrap_or(message.id).to_string(),
                message.envelope.channel_id.to_string(),
                message.created_at,
                raw,
                serde_json::to_string(content)?
            ],
        )?;
        tx.commit()?;
        self.activity(
            message.envelope.thread_id.unwrap_or(message.id),
            message.created_at,
        )?;
        Ok(())
    }
    pub fn activity(&self, thread: MessageId, timestamp: u64) -> Result<()> {
        self.db.execute("INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=CAST(MAX(CAST(state.value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)", params![format!("activity/{thread}"), timestamp.to_string()])?;
        Ok(())
    }
    pub fn prune(&self, retention: u32) -> Result<()> {
        if retention == 0 {
            return Ok(());
        }
        let cutoff = crate::now().saturating_sub(u64::from(retention) * 86400);
        self.db.execute("DELETE FROM messages WHERE thread IN (SELECT thread FROM messages GROUP BY thread HAVING MAX(created)<? AND COALESCE((SELECT CAST(value AS INTEGER) FROM state WHERE name='activity/'||thread),0)<?)", params![cutoff, cutoff])?;
        self.db.execute(
            "DELETE FROM reactions WHERE message NOT IN (SELECT id FROM messages)",
            [],
        )?;
        self.db.execute(
            "DELETE FROM cursors WHERE thread NOT IN (SELECT thread FROM messages)",
            [],
        )?;
        Ok(())
    }
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(user("refusing symlink storage directory"));
    }
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn private_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(user("refusing symlink storage file"));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(10))?;
    Ok(db)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut random = [0; 16];
    crate::entropy(&mut random)?;
    let temp = path.with_extension(format!("tmp-{}", MessageId::from_bytes(random)));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Result::Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
