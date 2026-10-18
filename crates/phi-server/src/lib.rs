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
use tokio::sync::watch;

#[derive(Clone)]
pub struct Server {
    store: Store,
    notifications: Arc<Mutex<BTreeMap<SpaceId, watch::Sender<u64>>>>,
    max_body: usize,
}
impl Server {
    pub fn open(config: Config) -> anyhow::Result<Self> {
        config.validate()?;
        let max_body = (config.limits.max_blob_bytes as usize)
            .checked_mul(2)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| anyhow::anyhow!("blob limit too large"))?
            .max(1024 * 1024);
        let store = Store::open(config).map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
        Ok(Self {
            store,
            notifications: Arc::new(Mutex::new(BTreeMap::new())),
            max_body,
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
        self.store
            .call(move |db| db.sweep(unix_seconds))
            .await
            .map_err(|e| anyhow::anyhow!("{:?}", e.0))
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
    let max = if path == "/v1/blobs" {
        server.max_body
    } else {
        1024 * 1024
    };
    let body = to_bytes(request.into_body(), max).await.map_err(|_| {
        ApiError(ErrorBody::TooLarge {
            message: "request body limit exceeded".into(),
        })
    })?;
    let is_write = method != Method::GET;
    let notifier = server.clone();
    server
        .store
        .call(move |db| {
            let p: Vec<_> = path.trim_matches('/').split('/').collect();
            let (response, changed) = if let (&Method::POST, ["v1", "spaces"]) =
                (&method, p.as_slice())
            {
                let response = db.create_space(parse(&body)?, token.as_deref())?;
                let sid = response.space.id;
                (value(response)?, Some(sid))
            } else if let (&Method::GET, ["v1", "grants", gid]) = (&method, p.as_slice()) {
                (value(db.grant(id(gid)?)?)?, None)
            } else if let (&Method::POST, ["v1", "grants", gid, "redeem"]) = (&method, p.as_slice())
            {
                let response = db.redeem(id(gid)?, parse(&body)?)?;
                let sid = response.grant.record.data.space_id;
                (value(response)?, Some(sid))
            } else {
                let auth = db.auth(token.as_deref())?;
                (
                    dispatch(db, &auth, &method, &p, &raw_query, &body)?,
                    is_write.then_some(auth.sid()),
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
        (&Method::POST, ["v1", "blobs"]) => {
            let r: UploadBlobRequest = serde_json::from_slice(body).map_err(ApiError::invalid)?;
            value(db.upload(a, r)?)
        }
        (&Method::GET, ["v1", "blobs", bid]) => value(db.download(a, id(bid)?)?),
        _ => Err(ApiError::missing()),
    }
}
