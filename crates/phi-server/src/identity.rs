use crate::{
    db::{Auth, Db, json, now, random},
    error::{ApiError, Result},
};
use phi_proto::*;
use rusqlite::params;

impl Db {
    pub fn add_device(&self, cert: DeviceCertificate) -> Result<Base64<32>> {
        let token = Base64::from_bytes(random()?);
        let raw = serde_json::to_value(&token)?
            .as_str()
            .ok_or_else(|| ApiError::internal("token encoding"))?
            .to_owned();
        let d = &cert.data;
        let person = Person {
            id: d.person_id,
            space_id: d.space_id,
            removed_at: None,
        };
        self.conn.execute(
            "INSERT OR IGNORE INTO persons VALUES(?,?,NULL,?)",
            params![
                d.person_id.to_string(),
                d.space_id.to_string(),
                json(&person)?
            ],
        )?;
        let device = Device {
            cert: cert.clone(),
            revoked_at: None,
            last_seen_at: Some(now()),
        };
        self.conn.execute(
            "INSERT INTO devices VALUES(?,?,?,?,NULL,?)",
            params![
                d.device_id.to_string(),
                d.space_id.to_string(),
                d.person_id.to_string(),
                crate::db::token_hash(&raw),
                json(&device)?
            ],
        )?;
        Ok(token)
    }
    pub fn create_space(
        &mut self,
        request: CreateSpaceRequest,
        admin: Option<&str>,
    ) -> Result<CreateSpaceResponse> {
        request.validate()?;
        if self.config.allow_space_creation == crate::config::Creation::Token {
            let expected = self
                .config
                .admin_token
                .as_deref()
                .ok_or_else(ApiError::forbidden)?;
            if admin.map(crate::db::token_hash) != Some(crate::db::token_hash(expected)) {
                return Err(ApiError::forbidden());
            }
        }
        let root = &request.root_cert;
        let bundle = TrustBundle {
            root: root.clone(),
            certificates: vec![],
            grants: vec![],
            revocations: vec![],
        };
        phi_crypto::verify_trust_bundle(root.data.space_id, &bundle, now())?;
        let space = Space {
            id: root.data.space_id,
            root_cert: root.clone(),
            settings: self.config.settings(),
            created_at: now(),
        };
        self.transaction(|db| {
            db.conn.execute(
                "INSERT INTO spaces(id,json) VALUES(?,?)",
                params![space.id.to_string(), json(&space)?],
            )?;
            let token = db.add_device(root.clone())?;
            let auth = Auth { cert: root.clone() };
            db.create_channel(&auth, request.all_channel, true)?;
            db.create_channel(&auth, request.self_channel, true)?;
            Ok(CreateSpaceResponse {
                space,
                device_token: token,
            })
        })
    }
    pub fn grant(&self, id: GrantId) -> Result<Grant> {
        self.one("SELECT json FROM grants WHERE id=?", [id.to_string()])
    }
    pub fn create_grant(&mut self, a: &Auth, r: CreateGrantRequest) -> Result<Grant> {
        r.validate()?;
        let d = &r.record.data;
        a.space(d.space_id)?;
        if d.issuer_device != a.device()
            || d.created_at > now()
            || d.created_at < a.cert.data.created_at
            || d.expires_at.is_some_and(|e| e <= now())
        {
            return Err(ApiError::invalid(
                "invalid grant issuer or validity interval",
            ));
        }
        self.verify(a, &phi_crypto::grant_signing_bytes(d)?, &r.record.signature)?;
        let existing: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM persons WHERE id=?)",
            [d.person_id.to_string()],
            |r| r.get(0),
        )?;
        match d.kind {
            GrantKind::Invite if existing => {
                return Err(ApiError::conflict("invite person already exists"));
            }
            GrantKind::Enroll if d.person_id != a.person() => return Err(ApiError::forbidden()),
            _ => {}
        }
        let channels: Vec<Channel> = self.list(
            "
            SELECT c.json FROM channels c JOIN members m ON m.channel_id=c.id
            WHERE c.space_id=? AND m.person_id=?
        ",
            params![a.sid().to_string(), a.person().to_string()],
        )?;
        for key in &r.sealed_keys {
            if !channels.iter().any(|c| {
                c.id == key.channel_id
                    && key.epoch <= c.current_epoch
                    && (d.kind == GrantKind::Enroll || c.kind == ChannelKind::All)
            }) {
                return Err(ApiError::invalid(
                    "grant key outside permitted channels/epochs",
                ));
            }
        }
        if !channels
            .iter()
            .filter(|c| d.kind == GrantKind::Enroll || c.kind == ChannelKind::All)
            .all(|c| {
                r.sealed_keys
                    .iter()
                    .any(|k| k.channel_id == c.id && k.epoch == c.current_epoch)
            })
        {
            return Err(ApiError::invalid("grant missing current channel keys"));
        }
        let grant = Grant {
            record: r.record,
            sealed_keys: r.sealed_keys,
            used_at: None,
        };
        self.conn.execute(
            "INSERT INTO grants VALUES(?,?,NULL,?)",
            params![
                grant.record.data.grant_id.to_string(),
                a.sid().to_string(),
                json(&grant)?
            ],
        )?;
        Ok(grant)
    }
    pub fn redeem(&mut self, id: GrantId, r: RedeemGrantRequest) -> Result<RedeemGrantResponse> {
        r.validate()?;
        self.transaction(|db| {
            let mut grant = db.grant(id)?;
            let data = &grant.record.data;
            if grant.used_at.is_some() {
                return Err(ApiError::conflict("grant already used"));
            }
            if data.expires_at.is_some_and(|e| e <= now()) {
                return Err(ApiError::forbidden());
            }
            let issuer: Device = db.one(
                "SELECT json FROM devices WHERE id=? AND revoked_at IS NULL",
                [data.issuer_device.to_string()],
            )?;
            if issuer.cert.data.space_id != data.space_id {
                return Err(ApiError::forbidden());
            }
            let cert = &r.certificate;
            if cert.data.space_id != data.space_id
                || cert.data.person_id != data.person_id
                || cert.data.issuer != (CertificateIssuer::Grant { grant_id: id })
            {
                return Err(ApiError::invalid("certificate grant identity mismatch"));
            }
            let mut directory = db.directory(data.space_id)?;
            directory.certificates.push(cert.clone());
            phi_crypto::verify_trust_bundle(data.space_id, &directory, now())?;
            let token = db.add_device(cert.clone())?;
            if data.kind == GrantKind::Invite {
                let mut all: Channel = db.one(
                    "SELECT json FROM channels WHERE space_id=? AND kind='all'",
                    [data.space_id.to_string()],
                )?;
                all.members.push(data.person_id);
                db.conn.execute(
                    "UPDATE channels SET json=? WHERE id=?",
                    params![json(&all)?, all.id.to_string()],
                )?;
                db.conn.execute(
                    "INSERT INTO members VALUES(?,?)",
                    params![all.id.to_string(), data.person_id.to_string()],
                )?;
            }
            grant.used_at = Some(now());
            db.conn.execute(
                "
            UPDATE grants SET used_at=?,json=? WHERE id=? AND used_at IS NULL
        ",
                params![grant.used_at, json(&grant)?, id.to_string()],
            )?;
            Ok(RedeemGrantResponse {
                device_token: token,
                grant,
                directory,
            })
        })
    }
    pub fn revoke(&mut self, a: &Auth, r: CreateRevocationRequest) -> Result<Revocation> {
        r.validate()?;
        let rev = r.revocation;
        a.space(rev.data.space_id)?;
        if rev.data.issuer_device != a.device() || rev.data.created_at > now() {
            return Err(ApiError::invalid("revocation issuer/time mismatch"));
        }
        self.verify(
            a,
            &phi_crypto::revocation_signing_bytes(&rev.data)?,
            &rev.signature,
        )?;
        self.transaction(|db| {
            let mut bundle = db.directory(a.sid())?;
            bundle.revocations.push(rev.clone());
            phi_crypto::verify_trust_bundle(a.sid(), &bundle, now())?;
            let devices: Vec<Device> = match rev.data.target {
                RevocationTarget::Device { device_id } => vec![db.one(
                    "SELECT json FROM devices WHERE id=? AND space_id=?",
                    params![device_id.to_string(), a.sid().to_string()],
                )?],
                RevocationTarget::Person { person_id } => {
                    let mut person: Person = db.one(
                        "SELECT json FROM persons WHERE id=? AND space_id=?",
                        params![person_id.to_string(), a.sid().to_string()],
                    )?;
                    person.removed_at = Some(now());
                    db.conn.execute(
                        "UPDATE persons SET removed_at=?,json=? WHERE id=?",
                        params![person.removed_at, json(&person)?, person_id.to_string()],
                    )?;
                    db.list(
                        "SELECT json FROM devices WHERE person_id=?",
                        [person_id.to_string()],
                    )?
                }
            };
            for mut device in devices {
                device.revoked_at = Some(now());
                db.conn.execute(
                    "UPDATE devices SET revoked_at=?,json=? WHERE id=?",
                    params![
                        device.revoked_at,
                        json(&device)?,
                        device.cert.data.device_id.to_string()
                    ],
                )?;
            }
            db.conn.execute(
                "INSERT INTO revocations(space_id,json) VALUES(?,?)",
                params![a.sid().to_string(), json(&rev)?],
            )?;
            let seq = db.seq(a.sid())?;
            db.event(
                a.sid(),
                seq,
                EventData::Revocation(rev.clone()),
                None,
                None,
                None,
            )?;
            Ok(rev)
        })
    }
    pub fn settings(&mut self, a: &Auth, r: UpdateSettingsRequest) -> Result<SettingsChange> {
        r.validate()?;
        let change = r.change;
        a.space(change.data.space_id)?;
        if change.data.issuer_device != a.device()
            || change.data.created_at > now()
            || change.data.created_at < a.cert.data.created_at
        {
            return Err(ApiError::invalid("settings issuer/time mismatch"));
        }
        self.verify(
            a,
            &phi_crypto::settings_signing_bytes(&change.data)?,
            &change.signature,
        )?;
        self.transaction(|db| {
            let mut space = db.space(a.sid())?;
            space.settings = change.data.settings.clone();
            db.conn.execute(
                "UPDATE spaces SET json=? WHERE id=?",
                params![json(&space)?, a.sid().to_string()],
            )?;
            let seq = db.seq(a.sid())?;
            let all: Channel = db.one(
                "SELECT json FROM channels WHERE space_id=? AND kind='all'",
                [a.sid().to_string()],
            )?;
            db.event(
                a.sid(),
                seq,
                EventData::Settings(change.clone()),
                Some(all.id),
                None,
                None,
            )?;
            Ok(change)
        })
    }
}
