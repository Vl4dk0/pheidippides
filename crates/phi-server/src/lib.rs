mod blobs;
mod channels;
pub mod config;
mod db;
mod error;
mod identity;
mod messages;
mod routes;

use crate::{
    config::Config,
    db::Store,
    error::{ApiError, Result},
};
use axum::Router;
use phi_proto::*;
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
        routes::router(self.clone())
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
