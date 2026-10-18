mod blobs;
mod channels;
pub mod config;
mod db;
mod error;
mod identity;
mod messages;

use crate::{
    config::Config,
    db::{Auth, Db, Store},
    error::{ApiError, Result},
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::Method,
    response::{IntoResponse, Response},
};
use phi_proto::*;
use rusqlite::params;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, watch};

#[derive(Clone)]
pub struct Server {
    store: Store,
    notifications: Arc<Mutex<BTreeMap<SpaceId, watch::Sender<u64>>>>,
    max_body: usize,
    config: Arc<Config>,
    uploads: Arc<Semaphore>,
}
impl Server {
    pub fn open(config: Config) -> anyhow::Result<Self> {
        config.validate()?;
        let max_body = (config.limits.max_blob_bytes as usize)
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| anyhow::anyhow!("blob limit too large"))?
            .max(1024 * 1024);
        let config = Arc::new(config);
        let store = Store::open((*config).clone()).map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
        Ok(Self {
            store,
            notifications: Arc::new(Mutex::new(BTreeMap::new())),
            max_body,
            config,
            uploads: Arc::new(Semaphore::new(1)),
        })
    }
    pub fn router(&self) -> Router {
        Router::new().fallback(handle).with_state(self.clone())
    }
    fn watch(&self, sid: SpaceId) -> Result<watch::Receiver<u64>> {
        let mut notifications = self.notifications.lock().map_err(ApiError::internal)?;
        Ok(notifications
            .entry(sid)
            .or_insert_with(|| watch::channel(0).0)
            .subscribe())
    }
    fn notify(&self, sid: SpaceId) -> Result<()> {
        if let Some(sender) = self
            .notifications
            .lock()
            .map_err(ApiError::internal)?
            .get(&sid)
        {
            sender.send_modify(|v| *v = v.wrapping_add(1));
        }
        Ok(())
    }
    pub async fn sweep(&self, unix_seconds: u64) -> anyhow::Result<u64> {
        let (deleted, garbage) = self
            .store
            .call(move |db| db.sweep(unix_seconds))
            .await
            .map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
        // Tombstones commit before slow filesystem work; interruption retries on the next sweep.
        let path = self.config.data_dir.join("blobs");
        let cleaned = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<String>> {
            for id in &garbage {
                match std::fs::remove_file(path.join(id)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            Ok(garbage)
        })
        .await??;
        self.store
            .call(move |db| {
                for id in cleaned {
                    db.conn.execute("DELETE FROM blob_gc WHERE id=?", [id])?;
                }
                Ok(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
        Ok(deleted)
    }
    pub async fn backup(&self, dest: std::path::PathBuf) -> anyhow::Result<()> {
        self.store
            .call(move |db| db.backup(&dest))
            .await
            .map_err(|e| anyhow::anyhow!("{:?}", e.0))
    }
    pub fn start_retention(&self) -> tokio::task::JoinHandle<()> {
        let server = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if let Err(error) = server.sweep(db::now()).await {
                    tracing::error!(%error,"retention failed");
                }
            }
        })
    }
    async fn events(
        &self,
        token: Option<String>,
        sid: SpaceId,
        mut q: EventsQuery,
    ) -> Result<EventsResponse> {
        q.validate()?;
        let auth_token = token.clone();
        self.store
            .call(move |db| db.auth(auth_token.as_deref())?.space(sid))
            .await?;
        let mut changes = self.watch(sid)?;
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(u64::from(q.timeout.unwrap_or(600)));
        loop {
            // Subscribe before reading: commits in the read-to-sleep gap remain observable.
            changes.borrow_and_update();
            let next = q.clone();
            let token = token.clone();
            let response = self
                .store
                .call(move |db| {
                    let auth = db.auth(token.as_deref())?;
                    auth.space(sid)?;
                    db.events(&auth, &next)
                })
                .await?;
            q.after = response.next_after;
            if !response.events.is_empty() || tokio::time::Instant::now() >= deadline {
                return Ok(response);
            }
            if tokio::time::timeout_at(deadline, changes.changed())
                .await
                .is_err()
            {
                return Ok(response);
            }
        }
    }
}
fn parse<T: DeserializeOwned + Validate>(body: &[u8]) -> Result<T> {
    let request: T = serde_json::from_slice(body).map_err(ApiError::invalid)?;
    request.validate()?;
    Ok(request)
}
fn query<T: DeserializeOwned + Validate>(raw: &str) -> Result<T> {
    let q: T = serde_urlencoded::from_str(raw).map_err(ApiError::invalid)?;
    q.validate()?;
    Ok(q)
}
fn value(v: impl Serialize) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(v)?)
}
fn id<T: std::str::FromStr<Err = ValidationError>>(s: &str) -> Result<T> {
    Ok(s.parse()?)
}

async fn handle(State(server): State<Server>, request: Request) -> Response {
    match handle_result(server, request).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}
async fn handle_result(server: Server, request: Request) -> Result<serde_json::Value> {
    let major = request
        .headers()
        .get("Phi-Protocol")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u16>().ok());
    if major != Some(PROTOCOL_MAJOR) {
        return Err(ApiError(ErrorBody::Upgrade {
            required_major: PROTOCOL_MAJOR,
        }));
    }
    let token = request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(str::to_owned);
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let raw_query = request.uri().query().unwrap_or("").to_owned();
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if let (&Method::GET, ["v1", "spaces", sid, "events"]) = (&method, parts.as_slice()) {
        return value(server.events(token, id(sid)?, query(&raw_query)?).await?);
    }
    let preflight_token = token.clone();
    let preflight_path = path.clone();
    let preflight_method = method.clone();
    server
        .store
        .call(move |db| {
            let parts: Vec<_> = preflight_path.trim_matches('/').split('/').collect();
            match (&preflight_method, parts.as_slice()) {
                (&Method::POST, ["v1", "spaces"]) => {
                    db.authorize_creation(preflight_token.as_deref())
                }
                (&Method::GET, ["v1", "grants", _])
                | (&Method::POST, ["v1", "grants", _, "redeem"]) => Ok(()),
                _ => {
                    let auth = db.auth(preflight_token.as_deref())?;
                    if matches!(
                        (&preflight_method, parts.as_slice()),
                        (&Method::POST, ["v1", "channels", _, "messages"])
                    ) {
                        db.rate_limit(&auth)?;
                    }
                    Ok(())
                }
            }
        })
        .await?;
    let uploading = method == Method::POST && path == "/v1/blobs";
    // Reject excess uploads before touching their bodies, bounding aggregate ciphertext buffers.
    let permit = if uploading {
        Some(
            server
                .uploads
                .clone()
                .try_acquire_owned()
                .map_err(|_| ApiError(ErrorBody::RateLimited { retry_after: 1 }))?,
        )
    } else {
        None
    };
    let max = if uploading {
        server.max_body
    } else {
        1024 * 1024
    };
    let body = to_bytes(request.into_body(), max).await.map_err(|_| {
        ApiError(ErrorBody::TooLarge {
            message: "request body limit exceeded".into(),
        })
    })?;
    if let Some(permit) = permit {
        let config = server.config.clone();
        let staged = tokio::task::spawn_blocking(move || {
            let request: UploadBlobRequest =
                serde_json::from_slice(&body).map_err(ApiError::invalid)?;
            blobs::StagedBlob::write(&config, request, permit)
        })
        .await
        .map_err(ApiError::internal)??;
        let (result, staged) = server
            .store
            .call(move |db| {
                let mut staged = staged;
                let result = (|| {
                    let auth = db.auth(token.as_deref())?;
                    db.touch(&auth)?;
                    db.commit_blob(&auth, &staged.blob)?;
                    staged.persist();
                    value(&staged.blob)
                })();
                // Return ownership past the DB guard, even on auth/quota failure.
                Ok((result, staged))
            })
            .await?;
        tokio::task::spawn_blocking(move || drop(staged))
            .await
            .map_err(ApiError::internal)?;
        return result;
    }

    if let (&Method::GET, ["v1", "blobs", bid]) = (&method, parts.as_slice()) {
        let bid = id(bid)?;
        let (blob, mut file) = server
            .store
            .call(move |db| {
                let auth = db.auth(token.as_deref())?;
                db.touch(&auth)?;
                let blob = db.blob(&auth, bid)?;
                // Open while its row is visible; an in-flight download survives retention unlink.
                let file = std::fs::File::open(
                    db.config.data_dir.join("blobs").join(blob.id.to_string()),
                )?;
                Ok((blob, file))
            })
            .await?;
        return tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut bytes)?;
            value(DownloadBlobResponse {
                blob,
                ciphertext: Base64Bytes::new(bytes),
            })
        })
        .await
        .map_err(ApiError::internal)?;
    }
    let notifier = server.clone();
    server
        .store
        .call(move |db| {
            let p: Vec<_> = path.trim_matches('/').split('/').collect();
            let (response, changed) = if let (&Method::POST, ["v1", "spaces"]) =
                (&method, p.as_slice())
            {
                let response = db.create_space(parse(&body)?, token.as_deref())?;
                (value(response)?, None)
            } else if let (&Method::GET, ["v1", "grants", gid]) = (&method, p.as_slice()) {
                (value(db.grant(id(gid)?)?)?, None)
            } else if let (&Method::POST, ["v1", "grants", gid, "redeem"]) = (&method, p.as_slice())
            {
                let response = db.redeem(id(gid)?, parse(&body)?)?;
                (value(response)?, None)
            } else {
                let auth = db.auth(token.as_deref())?;
                let before = db.latest(auth.sid())?;
                let response = dispatch(db, &auth, &method, &p, &raw_query, &body)?;
                db.touch(&auth)?;
                (
                    response,
                    (db.latest(auth.sid())? > before).then_some(auth.sid()),
                )
            };
            // Commit notifications survive cancellation of the HTTP request future.
            if let Some(sid) = changed {
                notifier.notify(sid)?;
            }
            Ok(response)
        })
        .await
}
fn dispatch(
    db: &mut Db,
    a: &Auth,
    method: &Method,
    p: &[&str],
    q: &str,
    body: &[u8],
) -> Result<serde_json::Value> {
    match (method, p) {
        (&Method::GET, ["v1", "spaces", sid]) => {
            let sid = id(sid)?;
            a.space(sid)?;
            value(db.space(sid)?)
        }
        (&Method::PATCH, ["v1", "spaces", sid, "settings"]) => {
            a.space(id(sid)?)?;
            value(db.settings(a, parse(body)?)?)
        }
        (&Method::POST, ["v1", "spaces", sid, "grants"]) => {
            a.space(id(sid)?)?;
            value(db.create_grant(a, parse(body)?)?)
        }
        (&Method::GET, ["v1", "spaces", sid, "devices"]) => {
            a.space(id(sid)?)?;
            value(ListDevicesResponse {
                persons: db.list(
                    "SELECT json FROM persons WHERE space_id=?",
                    [a.sid().to_string()],
                )?,
                devices: db.list(
                    "SELECT json FROM devices WHERE space_id=?",
                    [a.sid().to_string()],
                )?,
                directory: db.directory(a.sid())?,
            })
        }
        (&Method::POST, ["v1", "spaces", sid, "revocations"]) => {
            a.space(id(sid)?)?;
            value(db.revoke(a, parse(body)?)?)
        }
        (&Method::GET, ["v1", "spaces", sid, "channels"]) => {
            a.space(id(sid)?)?;
            value(ListChannelsResponse {
                channels: db.list(
                    "
            SELECT c.json FROM channels c JOIN members m ON m.channel_id=c.id
            WHERE c.space_id=? AND m.person_id=? ORDER BY c.id
        ",
                    params![a.sid().to_string(), a.person().to_string()],
                )?,
            })
        }
        (&Method::POST, ["v1", "spaces", sid, "channels"]) => {
            a.space(id(sid)?)?;
            let r = parse(body)?;
            value(db.transaction(|db| db.create_channel(a, r, false))?)
        }
        (&Method::PATCH, ["v1", "channels", cid]) => {
            value(db.update_channel(a, id(cid)?, parse(body)?)?)
        }
        (&Method::PUT, ["v1", "channels", cid, "keys"]) => {
            value(db.put_keys(a, id(cid)?, parse(body)?)?)
        }
        (&Method::GET, ["v1", "channels", cid, "keys", "mine"]) => value(db.keys(a, id(cid)?)?),
        (&Method::POST, ["v1", "channels", cid, "messages"]) => {
            value(db.send(a, id(cid)?, parse(body)?)?)
        }
        (&Method::GET, ["v1", "channels", cid, "messages"]) => {
            value(db.messages(a, id(cid)?, query(q)?)?)
        }
        (&Method::GET, ["v1", "messages", mid]) => value(db.message(a, id(mid)?)?),
        (&Method::POST, ["v1", "messages", mid, "reactions"]) => {
            value(db.reaction(a, id(mid)?, parse(body)?, None)?)
        }
        (&Method::DELETE, ["v1", "messages", mid, "reactions", kind]) => {
            let kind = serde_json::from_value(serde_json::Value::String(kind.to_string()))
                .map_err(ApiError::invalid)?;
            value(db.reaction(a, id(mid)?, parse(body)?, Some(kind))?)
        }
        (&Method::PUT, ["v1", "threads", tid, "cursor"]) => {
            value(db.cursor(a, id(tid)?, parse(body)?)?)
        }
        (&Method::GET, ["v1", "spaces", sid, "counts"]) => {
            a.space(id(sid)?)?;
            value(db.counts(a, query(q)?)?)
        }
        _ => Err(ApiError::missing()),
    }
}

#[cfg(test)]
mod tests;
