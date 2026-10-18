use crate::{
    db::{Auth, Db, json, now, random},
    error::{ApiError, Result},
};
use phi_proto::*;
use rusqlite::params;
use std::io::Write;

impl Db {
    pub fn upload(&mut self, a: &Auth, r: UploadBlobRequest) -> Result<Blob> {
        a.space(r.space_id)?;
        r.validate_with_limit(self.config.limits.max_blob_bytes)
            .map_err(|e| {
                ApiError(ErrorBody::TooLarge {
                    message: e.to_string(),
                })
            })?;
        let size = r.ciphertext.len() as u64;
        let used: u64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size),0) FROM blobs WHERE space_id=?",
            [a.sid().to_string()],
            |r| r.get(0),
        )?;
        if used
            .checked_add(size)
            .is_none_or(|v| v > self.config.limits.space_quota_bytes)
        {
            return Err(ApiError(ErrorBody::TooLarge {
                message: "space blob quota exceeded".into(),
            }));
        }
        let blob = Blob {
            id: BlobId::from_bytes(random()?),
            space_id: a.sid(),
            size,
            created_at: now(),
        };
        let path = self.config.data_dir.join("blobs").join(blob.id.to_string());
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        let stored = (|| -> Result<()> {
            file.write_all(r.ciphertext.as_bytes())?;
            file.sync_all()?;
            self.conn.execute(
                "INSERT INTO blobs VALUES(?,?,?,?,?,?)",
                params![
                    blob.id.to_string(),
                    a.sid().to_string(),
                    a.device().to_string(),
                    size,
                    blob.created_at,
                    json(&blob)?
                ],
            )?;
            Ok(())
        })();
        if let Err(error) = stored {
            if let Err(cleanup) = std::fs::remove_file(path) {
                tracing::error!(error=%cleanup,"failed to remove incomplete blob");
            }
            return Err(error);
        }
        Ok(blob)
    }
    pub fn download(&self, a: &Auth, id: BlobId) -> Result<DownloadBlobResponse> {
        let blob: Blob = self.one(
            "
            SELECT b.json FROM blobs b WHERE b.id=? AND b.space_id=? AND
            (b.uploader=? OR EXISTS(SELECT 1 FROM blob_refs br JOIN threads t ON
            t.id=br.thread_id JOIN members m ON m.channel_id=t.channel_id WHERE
            br.blob_id=b.id AND m.person_id=?))
        ",
            params![
                id.to_string(),
                a.sid().to_string(),
                a.device().to_string(),
                a.person().to_string()
            ],
        )?;
        let ciphertext = Base64Bytes::new(std::fs::read(
            self.config.data_dir.join("blobs").join(id.to_string()),
        )?);
        Ok(DownloadBlobResponse { blob, ciphertext })
    }
    pub fn rate_limit(&self, a: &Auth) -> Result<()> {
        let limits = self.space(a.sid())?.settings.rate_limits;
        let minute = f64::from(
            limits
                .messages_per_minute
                .min(self.config.limits.messages_per_min),
        );
        let hour = f64::from(
            limits
                .messages_per_hour
                .min(self.config.limits.messages_per_hour),
        );
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let (m, h, updated) = match self.conn.query_row(
            "SELECT minute,hour,updated FROM rate_buckets WHERE device_id=?",
            [a.device().to_string()],
            |r| {
                Ok((
                    r.get::<_, f64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            },
        ) {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => (minute, hour, at),
            Err(e) => return Err(e.into()),
        };
        let elapsed = (at - updated).max(0.0);
        let m = (m + elapsed * minute / 60.0).min(minute);
        let h = (h + elapsed * hour / 3600.0).min(hour);
        if m < 1.0 || h < 1.0 {
            let retry = ((1.0 - m).max(0.0) * 60.0 / minute)
                .max((1.0 - h).max(0.0) * 3600.0 / hour)
                .ceil() as u64;
            return Err(ApiError(ErrorBody::RateLimited {
                retry_after: retry.max(1),
            }));
        }
        self.conn.execute(
            "
            INSERT INTO rate_buckets VALUES(?,?,?,?) ON CONFLICT(device_id) DO
            UPDATE SET
            minute=excluded.minute,hour=excluded.hour,updated=excluded.updated
        ",
            params![a.device().to_string(), m - 1.0, h - 1.0, at],
        )?;
        Ok(())
    }
}
