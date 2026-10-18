use crate::{
    db::{Auth, Db, json},
    error::{ApiError, Result},
};
use phi_proto::*;
use rusqlite::params;

impl Db {
    fn signed_membership(
        &self,
        a: &Auth,
        proof: &ChannelMembership,
        channel: &Channel,
    ) -> Result<()> {
        let data = &proof.data;
        proof.validate()?;
        if data.space_id != a.sid()
            || data.channel_id != channel.id
            || data.kind != channel.kind
            || data.epoch != channel.current_epoch
            || data.members != channel.members
            || data.issuer_device != a.device()
        {
            return Err(ApiError::invalid(
                "signed membership scope, epoch, members or issuer mismatch",
            ));
        }
        self.verify(
            a,
            &phi_crypto::membership_signing_bytes(data)?,
            &proof.signature,
        )
    }
    fn valid_members(&self, a: &Auth, members: &[PersonId]) -> Result<()> {
        for person in members {
            let active: bool = self.conn.query_row(
                "
            SELECT EXISTS(SELECT 1 FROM persons WHERE id=? AND space_id=? AND
            removed_at IS NULL)
        ",
                params![person.to_string(), a.sid().to_string()],
                |r| r.get(0),
            )?;
            if !active {
                return Err(ApiError::invalid("unknown or removed channel member"));
            }
        }
        Ok(())
    }
    fn valid_keys(&self, a: &Auth, c: &Channel, keys: &[WrappedKey]) -> Result<()> {
        for key in keys {
            if key.channel_id != c.id {
                return Err(ApiError::invalid("wrapped key channel mismatch"));
            }
            let device: Device = self.one(
                "
            SELECT json FROM devices WHERE id=? AND space_id=? AND revoked_at IS
            NULL
        ",
                params![key.device_id.to_string(), a.sid().to_string()],
            )?;
            if !c.members.contains(&device.cert.data.person_id) {
                return Err(ApiError::invalid("key recipient is not a channel member"));
            }
        }
        Ok(())
    }
    fn save_keys(&self, keys: &[WrappedKey]) -> Result<()> {
        for key in keys {
            let old: Option<WrappedKey> = self.optional(
                "
            SELECT json FROM wrapped_keys WHERE channel_id=? AND epoch=? AND
            device_id=?
        ",
                params![
                    key.channel_id.to_string(),
                    key.epoch,
                    key.device_id.to_string()
                ],
            )?;
            if let Some(old) = old {
                if old != *key {
                    return Err(ApiError::conflict("wrapped key already exists"));
                }
            } else {
                self.conn.execute(
                    "INSERT INTO wrapped_keys VALUES(?,?,?,?)",
                    params![
                        key.channel_id.to_string(),
                        key.epoch,
                        key.device_id.to_string(),
                        json(key)?
                    ],
                )?;
            }
        }
        Ok(())
    }
    pub fn create_channel(
        &self,
        a: &Auth,
        r: CreateChannelRequest,
        initial: bool,
    ) -> Result<Channel> {
        r.validate()?;
        a.space(r.space_id)?;
        if !r.members.contains(&a.person()) || (!initial && r.kind == ChannelKind::All) {
            return Err(ApiError::forbidden());
        }
        self.valid_members(a, &r.members)?;
        if matches!(r.kind, ChannelKind::SelfChannel | ChannelKind::Dm) {
            let channels: Vec<Channel> = self.list(
                "
            SELECT c.json FROM channels c JOIN members m ON m.channel_id=c.id
            WHERE c.space_id=? AND m.person_id=?
        ",
                params![a.sid().to_string(), a.person().to_string()],
            )?;
            if channels.iter().any(|c| {
                c.kind == r.kind
                    && c.members.len() == r.members.len()
                    && c.members.iter().all(|m| r.members.contains(m))
            }) {
                return Err(ApiError::conflict(
                    "channel for these persons already exists",
                ));
            }
        }
        let channel = Channel {
            membership: r.membership,
            membership_history: vec![],
            id: r.id,
            space_id: r.space_id,
            kind: r.kind,
            meta: r.meta,
            meta_epoch: 0,
            members: r.members,
            current_epoch: 0,
        };
        self.signed_membership(a, &channel.membership, &channel)?;
        self.valid_keys(a, &channel, &r.wrapped_keys)?;
        self.conn.execute(
            "INSERT INTO channels VALUES(?,?,?,?)",
            params![
                channel.id.to_string(),
                a.sid().to_string(),
                json(&channel.kind)?.trim_matches('"'),
                json(&channel)?
            ],
        )?;
        for person in &channel.members {
            self.conn.execute(
                "INSERT INTO members VALUES(?,?)",
                params![channel.id.to_string(), person.to_string()],
            )?;
        }
        self.save_keys(&r.wrapped_keys)?;
        Ok(channel)
    }
    pub fn update_channel(
        &mut self,
        a: &Auth,
        cid: ChannelId,
        r: UpdateChannelRequest,
    ) -> Result<Channel> {
        r.validate()?;
        self.transaction(|db| {
            let mut c = db.channel(a, cid)?;
            if c.kind != ChannelKind::Group || !r.members.contains(&a.person()) {
                return Err(ApiError::forbidden());
            }
            if r.meta_epoch != c.current_epoch {
                return Err(ApiError::conflict("metadata must use current epoch"));
            }
            db.valid_members(a, &r.members)?;
            if r.membership.data.previous.as_ref() != Some(&c.membership.signature) {
                return Err(ApiError::conflict("membership predecessor mismatch"));
            }
            c.members = r.members;
            c.membership_history.push(c.membership);
            c.membership = r.membership;
            db.signed_membership(a, &c.membership, &c)?;
            c.meta = r.meta;
            c.meta_epoch = r.meta_epoch;
            db.conn.execute(
                "UPDATE channels SET json=? WHERE id=?",
                params![json(&c)?, cid.to_string()],
            )?;
            db.conn
                .execute("DELETE FROM members WHERE channel_id=?", [cid.to_string()])?;
            for p in &c.members {
                db.conn.execute(
                    "INSERT INTO members VALUES(?,?)",
                    params![cid.to_string(), p.to_string()],
                )?;
            }
            Ok(c)
        })
    }
    pub fn put_keys(
        &mut self,
        a: &Auth,
        cid: ChannelId,
        r: PutKeysRequest,
    ) -> Result<GetKeysResponse> {
        r.validate()?;
        self.transaction(|db| {
            let mut channel = db.channel(a, cid)?;
            if r.epoch
                > channel
                    .current_epoch
                    .checked_add(1)
                    .ok_or_else(|| ApiError::conflict("epoch exhausted"))?
            {
                return Err(ApiError::conflict("epoch must advance by one"));
            }
            db.valid_keys(a, &channel, &r.wrapped_keys)?;
            if r.epoch > channel.current_epoch {
                let devices: Vec<Device> = db.list(
                    "
            SELECT d.json FROM devices d JOIN members m ON m.person_id=d.person_id
            WHERE m.channel_id=? AND d.revoked_at IS NULL
        ",
                    [cid.to_string()],
                )?;
                if devices.iter().any(|d| {
                    !r.wrapped_keys
                        .iter()
                        .any(|k| k.device_id == d.cert.data.device_id)
                }) {
                    return Err(ApiError::invalid(
                        "rotation must wrap to every active member device",
                    ));
                }
                channel.current_epoch = r.epoch;
                let proof = r
                    .membership
                    .as_ref()
                    .ok_or_else(|| ApiError::invalid("rotation requires signed membership"))?;
                if proof.data.previous.as_ref() != Some(&channel.membership.signature) {
                    return Err(ApiError::conflict("membership predecessor mismatch"));
                }
                channel.membership_history.push(channel.membership.clone());
                channel.membership = r
                    .membership
                    .ok_or_else(|| ApiError::invalid("rotation requires signed membership"))?;
                db.signed_membership(a, &channel.membership, &channel)?;
                db.conn.execute(
                    "UPDATE channels SET json=? WHERE id=?",
                    params![json(&channel)?, cid.to_string()],
                )?;
            } else if r.membership.is_some()
                || r.wrapped_keys.iter().any(|k| k.device_id != a.device())
            {
                return Err(ApiError::forbidden());
            }
            db.save_keys(&r.wrapped_keys)?;
            Ok(GetKeysResponse {
                wrapped_keys: r.wrapped_keys,
            })
        })
    }
    pub fn keys(&self, a: &Auth, cid: ChannelId) -> Result<GetKeysResponse> {
        self.channel(a, cid)?;
        Ok(GetKeysResponse {
            wrapped_keys: self.list(
                "
            SELECT json FROM wrapped_keys WHERE channel_id=? AND device_id=? ORDER
            BY epoch
        ",
                params![cid.to_string(), a.device().to_string()],
            )?,
        })
    }
}
