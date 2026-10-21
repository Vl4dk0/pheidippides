use crate::{Result, user};
use phi_crypto::{PinnedTrustState, VerifiedDirectory, verify_trust_bundle};
use phi_proto::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeMap;
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
                let profile: Profile = toml::from_str(&fs::read_to_string(file)?)?;
                result.push(profile);
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
        Err(profile_selection_error(&profiles))
    }
    pub fn bind(&self, cwd: &Path, name: Option<&str>) -> Result<Profile> {
        let profile = if let Some(name) = name {
            self.resolve(cwd, Some(name))?
        } else {
            let profiles = self.profiles()?;
            match profiles.as_slice() {
                [profile] => profile.clone(),
                _ => return Err(profile_selection_error(&profiles)),
            }
        };
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

fn profile_selection_error(profiles: &[Profile]) -> crate::Error {
    if profiles.is_empty() {
        return user(
            "no local spaces; run phi space create <name> --server <url> or phi join '<link>' --name <name>",
        );
    }
    user(format!(
        "select a space; available spaces:\n{}",
        profiles
            .iter()
            .map(|profile| format!(
                "  {} ({}) · run: phi init {}",
                profile.name, profile.space_id, profile.space_id
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

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
        CREATE TABLE IF NOT EXISTS message_numbers(id TEXT PRIMARY KEY,thread TEXT NOT NULL,number INTEGER NOT NULL,UNIQUE(thread,number));
        CREATE TABLE IF NOT EXISTS message_pins(id TEXT PRIMARY KEY,seq INTEGER UNIQUE NOT NULL,digest TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS reactions(message TEXT NOT NULL,person TEXT NOT NULL,handle TEXT NOT NULL,kind TEXT NOT NULL,raw TEXT NOT NULL,PRIMARY KEY(message,person,handle,kind));
        CREATE TABLE IF NOT EXISTS observations(evidence TEXT PRIMARY KEY,thread TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS wait_outcomes(handle TEXT NOT NULL,seq INTEGER NOT NULL,digest TEXT NOT NULL,error TEXT,PRIMARY KEY(handle,seq));
        CREATE TABLE IF NOT EXISTS pending_revocations(signature TEXT PRIMARY KEY,raw TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS cursors(thread TEXT PRIMARY KEY,seq INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS histories(root TEXT PRIMARY KEY, json TEXT NOT NULL);")?;
        if db.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))? < 2 {
            let tx = db.unchecked_transaction()?;
            tx.execute(
                "UPDATE state SET value=? WHERE name LIKE 'activity/%'",
                [crate::now().to_string()],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO state SELECT 'activity/'||thread,? FROM messages",
                [crate::now().to_string()],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO observations SELECT 'message/'||id,thread FROM messages",
                [],
            )?;
            tx.execute("INSERT OR IGNORE INTO observations SELECT 'reaction/'||r.raw,m.thread FROM reactions r JOIN messages m ON m.id=r.message", [])?;
            tx.execute_batch("PRAGMA user_version=2")?;
            tx.commit()?;
        }
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
        let mut verified = verify_trust_bundle(sid, bundle, crate::now(), prior.as_ref())?;
        tx.execute("INSERT INTO state VALUES('trust',?) ON CONFLICT(name) DO UPDATE SET value=excluded.value", [serde_json::to_string(&verified.pinned_state())?])?;
        tx.commit()?;
        self.attach_histories(&mut verified)?;
        Ok(verified)
    }
    pub fn save_history(&self, root: &HistoryRoot, signatures: &[Signature]) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO histories(root, json) VALUES(?,?)",
            params![root.to_string(), serde_json::to_string(signatures)?],
        )?;
        Ok(())
    }
    /// Cached lists are not part of the trust pin. Attach every one this directory is missing.
    pub fn attach_histories(&self, directory: &mut VerifiedDirectory) -> Result<()> {
        for root in directory.missing_histories() {
            let json: Option<String> = self
                .db
                .query_row(
                    "SELECT json FROM histories WHERE root=?",
                    [root.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(json) = json {
                let list: Vec<Signature> = serde_json::from_str(&json)?;
                directory.attach_history(root, &list)?;
            }
        }
        Ok(())
    }
    pub fn record_revocation(
        &mut self,
        directory: &VerifiedDirectory,
        revocation: &Revocation,
    ) -> Result<VerifiedDirectory> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let prior: String =
            tx.query_row("SELECT value FROM state WHERE name='trust'", [], |r| {
                r.get(0)
            })?;
        let prior: PinnedTrustState = serde_json::from_str(&prior)?;
        let mut bundle = directory.trust_bundle();
        if !bundle.revocations.contains(revocation) {
            bundle.revocations.push(revocation.clone());
        }
        let mut verified =
            verify_trust_bundle(directory.space_id(), &bundle, crate::now(), Some(&prior))?;
        tx.execute(
            "UPDATE state SET value=? WHERE name='trust'",
            [serde_json::to_string(&verified.pinned_state())?],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO pending_revocations VALUES(?,?)",
            params![
                serde_json::to_string(&revocation.signature)?,
                serde_json::to_string(revocation)?
            ],
        )?;
        tx.commit()?;
        self.attach_histories(&mut verified)?;
        Ok(verified)
    }
    pub fn accept_settings(
        &mut self,
        directory: &VerifiedDirectory,
        change: &SettingsChange,
    ) -> Result<SpaceSettings> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row("SELECT value FROM state WHERE name='settings'", [], |r| {
                r.get(0)
            })
            .optional()?;
        let previous: Option<SettingsChange> =
            previous.map(|v| serde_json::from_str(&v)).transpose()?;
        // The cached copy must not accept a change the directory now rejects.
        directory.verify_settings(change)?;
        if previous.as_ref() == Some(change) {
            return Ok(change.data.settings.clone());
        }
        tx.execute("INSERT INTO state VALUES('settings',?) ON CONFLICT(name) DO UPDATE SET value=excluded.value", [serde_json::to_string(change)?])?;
        tx.commit()?;
        Ok(change.data.settings.clone())
    }
    pub fn trusted_settings(&self) -> Result<SpaceSettings> {
        Ok(self
            .get::<SettingsChange>("settings")?
            .map(|s| s.data.settings)
            .unwrap_or_default())
    }
    pub fn channel_epochs(
        &self,
        directory: &VerifiedDirectory,
        channel: &Channel,
    ) -> Result<BTreeMap<Epoch, PublicKey>> {
        channel_epochs(&self.db, directory, channel)
    }
    pub fn channel(&mut self, directory: &VerifiedDirectory, channel: &Channel) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let epochs = channel_epochs(&tx, directory, channel)?;
        for (name, value) in [
            (
                format!("roster/{}", channel.id),
                serde_json::to_string(&channel.membership)?,
            ),
            (
                format!("epochs/{}", channel.id),
                serde_json::to_string(&epochs)?,
            ),
        ] {
            tx.execute("INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=excluded.value", params![name, value])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn wait_outcome(
        &self,
        handle: &Handle,
        event: &Event,
        error: Option<&str>,
    ) -> Result<bool> {
        let digest = blake3::hash(&serde_json::to_vec(event)?)
            .to_hex()
            .to_string();
        let changed = self.db.execute("INSERT INTO wait_outcomes VALUES(?,?,?,?) ON CONFLICT(handle,seq) DO UPDATE SET error=COALESCE(wait_outcomes.error,excluded.error) WHERE wait_outcomes.digest=excluded.digest", params![handle.as_str(), event.seq, digest, error])?;
        if changed == 0 {
            self.db.execute("UPDATE wait_outcomes SET error='server changed a previously observed event' WHERE handle=? AND seq=?", params![handle.as_str(), event.seq])?;
        }
        Ok(changed > 0)
    }
    pub fn number(&self, message: &Message, number: usize) -> Result<()> {
        let changed = self.db.execute("INSERT INTO message_numbers VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET number=excluded.number WHERE message_numbers.thread=excluded.thread AND message_numbers.number=excluded.number", params![message.id.to_string(), message.envelope.thread_id.unwrap_or(message.id).to_string(), number])?;
        if changed == 0 {
            return Err(user("message thread position changed"));
        }
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
        observe_activity(
            &tx,
            message.envelope.thread_id.unwrap_or(message.id),
            &format!("message/{}", message.id),
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn activity(&mut self, thread: MessageId, evidence: &str) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        observe_activity(&tx, thread, evidence)?;
        tx.commit()?;
        Ok(())
    }
    pub fn prune(&self, retention: u32) -> Result<()> {
        if retention == 0 {
            return Ok(());
        }
        let cutoff = crate::now().saturating_sub(u64::from(retention) * 86400);
        self.db.execute("DELETE FROM messages WHERE thread IN (SELECT thread FROM messages GROUP BY thread HAVING COALESCE((SELECT CAST(value AS INTEGER) FROM state WHERE name='activity/'||thread),0)<?)", [cutoff])?;
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
    pub fn delete_thread(&mut self, thread: MessageId, cache_dir: &Path) -> Result<()> {
        let tid = thread.to_string();
        let contents: Vec<String> = {
            let mut stmt = self
                .db
                .prepare("SELECT content FROM messages WHERE thread=?")?;
            stmt.query_map([&tid], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM reactions WHERE message IN (SELECT id FROM messages WHERE thread=?)",
            [&tid],
        )?;
        tx.execute("DELETE FROM messages WHERE thread=?", [&tid])?;
        tx.execute("DELETE FROM message_numbers WHERE thread=?", [&tid])?;
        tx.execute("DELETE FROM observations WHERE thread=?", [&tid])?;
        tx.execute("DELETE FROM cursors WHERE thread=?", [&tid])?;
        tx.execute(
            "DELETE FROM state WHERE name=? OR name=? OR name LIKE ?",
            params![
                format!("activity/{tid}"),
                format!("discovery/{tid}"),
                format!("read/%/{tid}")
            ],
        )?;
        tx.commit()?;
        for raw in contents {
            if let Ok(content) = serde_json::from_str::<MessageContent>(&raw) {
                for attachment in content.attachments {
                    let _ = fs::remove_dir_all(
                        cache_dir.join("blobs").join(attachment.blob_id.to_string()),
                    );
                }
            }
        }
        Ok(())
    }
}
fn observe_activity(db: &Connection, thread: MessageId, evidence: &str) -> Result<()> {
    let inserted = db.execute(
        "INSERT OR IGNORE INTO observations VALUES(?,?)",
        params![evidence, thread.to_string()],
    )?;
    if inserted > 0 {
        db.execute("INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=CAST(MAX(CAST(state.value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)", params![format!("activity/{thread}"), crate::now().to_string()])?;
    }
    Ok(())
}
fn channel_epochs(
    db: &Connection,
    directory: &VerifiedDirectory,
    channel: &Channel,
) -> Result<BTreeMap<Epoch, PublicKey>> {
    let prior: Option<String> = db
        .query_row(
            "SELECT value FROM state WHERE name=?",
            [format!("roster/{}", channel.id)],
            |r| r.get(0),
        )
        .optional()?;
    let prior: Option<ChannelMembership> = prior.map(|v| serde_json::from_str(&v)).transpose()?;
    directory.verify_channel(channel, prior.as_ref())?;
    let raw: Option<String> = db
        .query_row(
            "SELECT value FROM state WHERE name=?",
            [format!("epochs/{}", channel.id)],
            |r| r.get(0),
        )
        .optional()?;
    let mut epochs: BTreeMap<Epoch, PublicKey> = raw
        .map(|v| serde_json::from_str(&v))
        .transpose()?
        .unwrap_or_default();
    let records: Vec<_> = channel
        .membership_history
        .iter()
        .chain(std::iter::once(&channel.membership))
        .collect();
    let start = prior
        .as_ref()
        .and_then(|p| records.iter().position(|r| *r == p))
        .unwrap_or(0);
    for record in &records[start..] {
        if epochs
            .get(&record.data.epoch)
            .is_some_and(|old| old != &record.data.key_commitment)
        {
            return Err(user("epoch key commitment changed"));
        }
        epochs.insert(record.data.epoch, record.data.key_commitment.clone());
    }
    Ok(epochs)
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    if crate::private_file::is_link(path) {
        return Err(user("refusing symlink storage directory"));
    }
    fs::create_dir_all(path)?;
    crate::private_file::restrict_owner(path)?;
    Ok(())
}
fn private_database(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    if crate::private_file::is_link(path) {
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
    // Tighten a pre-existing file before SQLite writes pages, -wal or -shm
    // beside it; creation mode alone does not touch an older file.
    crate::private_file::restrict_owner(path)?;
    // A filesystem path, not a SQLite URI, so Windows separators stay literal.
    // WAL matches the server: the default rollback journal holds a lock that
    // a second connection (a live client plus a reader) cannot share on Windows.
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(10))?;
    db.execute_batch("PRAGMA journal_mode=WAL;")?;
    Ok(db)
}
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if crate::private_file::is_link(path) {
        return Err(user("refusing symlink storage file"));
    }
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
        crate::private_file::restrict_owner(&temp)?;
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
