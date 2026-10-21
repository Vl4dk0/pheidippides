use crate::{
    config::Config,
    error::{ApiError, Result},
};
use phi_proto::*;
use rusqlite::{Connection, OptionalExtension, Params, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) struct Db {
    pub conn: Connection,
    pub config: Config,
}
#[derive(Clone)]
pub(crate) struct Store(pub Arc<Mutex<Db>>);
#[derive(Clone)]
pub(crate) struct Auth {
    pub cert: DeviceCertificate,
}
impl Auth {
    pub fn sid(&self) -> SpaceId {
        self.cert.data.space_id
    }
    pub fn person(&self) -> PersonId {
        self.cert.data.person_id
    }
    pub fn device(&self) -> DeviceId {
        self.cert.data.device_id
    }
    pub fn space(&self, sid: SpaceId) -> Result<()> {
        if sid == self.sid() {
            Ok(())
        } else {
            Err(ApiError::missing())
        }
    }
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(crate) fn json(value: &impl Serialize) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}
pub(crate) fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    Ok(bytes)
}
pub(crate) fn token_hash(raw: &str) -> String {
    blake3::hash(raw.as_bytes()).to_hex().to_string()
}
impl Store {
    pub fn open(config: Config) -> Result<Self> {
        std::fs::create_dir_all(config.data_dir.join("blobs"))?;
        let conn = Connection::open(config.data_dir.join("phi.db"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.execute_batch(include_str!("schema.sql"))?;
        let has_closed: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('threads') WHERE name='closed_by')",
            [],
            |row| row.get(0),
        )?;
        if !has_closed {
            conn.execute_batch(
                "ALTER TABLE threads ADD COLUMN closed_by TEXT;
                 ALTER TABLE threads ADD COLUMN closed_at INTEGER;
                 ALTER TABLE threads ADD COLUMN close_json TEXT;",
            )?;
        }
        if conn.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))? < 1 {
            conn.execute_batch("BEGIN IMMEDIATE;
                INSERT OR IGNORE INTO message_mentions
                SELECT m.id,json_extract(j.value,'$.person_id'),COALESCE(json_extract(j.value,'$.handle'),'')
                FROM messages m,json_each(m.json,'$.envelope.mentions') j;
                PRAGMA user_version=1; COMMIT;")?;
        }
        Ok(Self(Arc::new(Mutex::new(Db { conn, config }))))
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let db = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = db.lock().map_err(ApiError::internal)?;
            f(&mut db)
        })
        .await
        .map_err(ApiError::internal)?
    }
}
impl Db {
    pub fn one<T: DeserializeOwned>(&self, sql: &str, args: impl Params) -> Result<T> {
        self.optional(sql, args)?.ok_or_else(ApiError::missing)
    }
    pub fn optional<T: DeserializeOwned>(&self, sql: &str, args: impl Params) -> Result<Option<T>> {
        let raw: Option<String> = self.conn.query_row(sql, args, |r| r.get(0)).optional()?;
        raw.map(|s| serde_json::from_str(&s).map_err(ApiError::from))
            .transpose()
    }
    pub fn list<T: DeserializeOwned>(&self, sql: &str, args: impl Params) -> Result<Vec<T>> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(args, |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn auth(&self, token: Option<&str>) -> Result<Auth> {
        let device: Device = self
            .optional(
                "
            SELECT json FROM devices WHERE token_hash=? AND revoked_at IS NULL AND NOT EXISTS(SELECT 1 FROM revocation_holds h WHERE h.device_id=devices.id)
        ",
                [token_hash(token.ok_or_else(ApiError::unauthorized)?)],
            )?
            .ok_or_else(ApiError::unauthorized)?;
        Ok(Auth { cert: device.cert })
    }
    pub fn touch(&self, a: &Auth) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET json=json_set(json,'$.last_seen_at',?) WHERE id=?
            AND COALESCE(json_extract(json,'$.last_seen_at'),0)<?",
            params![now(), a.device().to_string(), now().saturating_sub(60)],
        )?;
        Ok(())
    }
    pub fn space(&self, sid: SpaceId) -> Result<Space> {
        let mut space: Space = self.one("SELECT json FROM spaces WHERE id=?", [sid.to_string()])?;
        space.limits = ServerLimits {
            max_attachment_bytes: self.config.limits.max_blob_bytes,
            space_quota_bytes: self.config.limits.space_quota_bytes,
        };
        Ok(space)
    }
    pub fn channel(&self, a: &Auth, cid: ChannelId) -> Result<Channel> {
        self.one(
            "
            SELECT c.json FROM channels c JOIN members m ON m.channel_id=c.id
            WHERE c.id=? AND c.space_id=? AND m.person_id=?
        ",
            params![cid.to_string(), a.sid().to_string(), a.person().to_string()],
        )
    }
    pub fn message(&self, a: &Auth, mid: MessageId) -> Result<Message> {
        self.one(
            "
            SELECT m.json FROM messages m JOIN members cm ON
            cm.channel_id=m.channel_id WHERE m.id=? AND m.space_id=? AND
            cm.person_id=?
        ",
            params![mid.to_string(), a.sid().to_string(), a.person().to_string()],
        )
    }
    pub fn trust_pins(&self, sid: SpaceId) -> Result<phi_crypto::PinnedTrustState> {
        self.one(
            "SELECT json FROM trust_pins WHERE space_id=?",
            [sid.to_string()],
        )
    }
    pub fn pin_trust(&self, sid: SpaceId, verified: &phi_crypto::VerifiedDirectory) -> Result<()> {
        self.conn.execute("INSERT INTO trust_pins VALUES(?,?) ON CONFLICT(space_id) DO UPDATE SET json=excluded.json", params![sid.to_string(),json(&verified.pinned_state())?])?;
        Ok(())
    }
    pub fn directory(&self, sid: SpaceId) -> Result<TrustBundle> {
        let root = self.space(sid)?.root_cert;
        let devices: Vec<Device> = self.list(
            "SELECT json FROM devices WHERE space_id=? AND id!=?",
            params![sid.to_string(), root.data.device_id.to_string()],
        )?;
        let grants: Vec<Grant> = self.list(
            "SELECT json FROM grants WHERE space_id=?",
            [sid.to_string()],
        )?;
        Ok(TrustBundle {
            root,
            certificates: devices.into_iter().map(|d| d.cert).collect(),
            grants: grants.into_iter().map(|g| g.record).collect(),
            revocations: self.list(
                "SELECT json FROM revocations WHERE space_id=?",
                [sid.to_string()],
            )?,
        })
    }
    pub fn verify(&self, a: &Auth, bytes: &[u8], signature: &Signature) -> Result<()> {
        Ok(phi_crypto::verify_signature(
            &a.cert.data.sign_pub,
            bytes,
            signature,
        )?)
    }
    pub fn seq(&self, sid: SpaceId) -> Result<u64> {
        Ok(self.conn.query_row(
            "UPDATE spaces SET seq=seq+1 WHERE id=? RETURNING seq",
            [sid.to_string()],
            |r| r.get(0),
        )?)
    }
    pub fn latest(&self, sid: SpaceId) -> Result<u64> {
        Ok(self.conn.query_row(
            "SELECT seq FROM spaces WHERE id=?",
            [sid.to_string()],
            |r| r.get(0),
        )?)
    }
    pub fn event(
        &self,
        sid: SpaceId,
        seq: u64,
        event: EventData,
        cid: Option<ChannelId>,
        target: Option<PersonId>,
        thread: Option<MessageId>,
    ) -> Result<()> {
        let event = Event {
            space_id: sid,
            seq,
            created_at: now(),
            event,
        };
        self.conn.execute(
            "INSERT INTO events VALUES(?,?,?,?,?,?)",
            params![
                sid.to_string(),
                seq,
                cid.map(|v| v.to_string()),
                target.map(|v| v.to_string()),
                thread.map(|v| v.to_string()),
                json(&event)?
            ],
        )?;
        Ok(())
    }
    pub fn transaction<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = f(self);
        match result {
            Ok(value) => match self.conn.execute_batch("COMMIT") {
                Ok(()) => Ok(value),
                Err(error) => {
                    self.rollback();
                    Err(error.into())
                }
            },
            Err(error) => {
                self.rollback();
                Err(error)
            }
        }
    }
    fn rollback(&self) {
        if let Err(error) = self.conn.execute_batch("ROLLBACK") {
            tracing::error!(%error,"transaction rollback failed");
        }
    }

    pub fn backup(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(ApiError::invalid("backup destination must not exist"));
        }
        // A separate reserved writer lock also excludes a live server process while
        // the source connection takes its online snapshot and copies referenced files.
        let locker = Connection::open(self.config.data_dir.join("phi.db"))?;
        locker.busy_timeout(std::time::Duration::from_secs(30))?;
        locker.execute_batch("BEGIN IMMEDIATE")?;
        std::fs::create_dir_all(dest.join("blobs"))?;
        self.conn
            .backup(rusqlite::MAIN_DB, dest.join("phi.db"), None)?;
        let ids: Vec<String> = {
            let mut q = self.conn.prepare("SELECT id FROM blobs")?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        for id in ids {
            std::fs::copy(
                self.config.data_dir.join("blobs").join(&id),
                dest.join("blobs").join(id),
            )?;
        }
        Ok(())
    }
    pub fn sweep(&mut self, at: u64) -> Result<(u64, Vec<String>)> {
        let spaces: Vec<Space> = self.list("SELECT json FROM spaces", [])?;
        let mut deleted = 0;
        for space in spaces {
            if space.settings.retention_days == 0 {
                continue;
            }
            let cutoff = at.saturating_sub(u64::from(space.settings.retention_days) * 86400);
            self.transaction(|db| {
                deleted += db.conn.execute(
                    "DELETE FROM threads WHERE space_id=? AND activity<?",
                    params![space.id.to_string(), cutoff],
                )? as u64;
                let ids: Vec<Blob> = db.list(
                    "
            SELECT json FROM blobs WHERE space_id=? AND created_at<? AND NOT
            EXISTS(SELECT 1 FROM blob_refs r WHERE r.blob_id=blobs.id)
        ",
                    params![space.id.to_string(), cutoff],
                )?;
                for blob in ids {
                    db.conn.execute(
                        "INSERT OR IGNORE INTO blob_gc VALUES(?)",
                        [blob.id.to_string()],
                    )?;
                    db.conn
                        .execute("DELETE FROM blobs WHERE id=?", [blob.id.to_string()])?;
                }
                Ok(())
            })?;
        }
        let garbage: Vec<String> = {
            let mut q = self.conn.prepare("SELECT id FROM blob_gc")?;
            q.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?
        };
        Ok((deleted, garbage))
    }
}
