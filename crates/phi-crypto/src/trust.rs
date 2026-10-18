use crate::{CryptoError, DeviceKeys, GrantKeys, signing_bytes, space_id, verify_signature};
use phi_proto::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn certificate_signing_bytes(data: &DeviceCertificateData) -> Result<Vec<u8>, CryptoError> {
    let issuer = match &data.issuer {
        CertificateIssuer::Device { device_id } => ("device", device_id.to_string()),
        CertificateIssuer::Grant { grant_id } => ("grant", grant_id.to_string()),
    };
    signing_bytes(
        "phi device certificate v1",
        &(
            data.space_id,
            data.person_id,
            data.device_id,
            data.kind,
            &data.sign_pub,
            &data.kex_pub,
            data.created_at,
            issuer,
        ),
    )
}
pub fn grant_signing_bytes(data: &GrantRecordData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi grant record v1",
        &(
            data.space_id,
            data.grant_id,
            &data.grant_pub,
            data.kind,
            data.person_id,
            data.created_at,
            data.expires_at,
            data.issuer_device,
        ),
    )
}
pub fn revocation_signing_bytes(data: &RevocationData) -> Result<Vec<u8>, CryptoError> {
    let target = match &data.target {
        RevocationTarget::Device { device_id } => ("device", device_id.to_string()),
        RevocationTarget::Person { person_id } => ("person", person_id.to_string()),
    };
    signing_bytes(
        "phi revocation v1",
        &(data.space_id, target, data.issuer_device, data.created_at),
    )
}
pub fn reaction_signing_bytes(data: &ReactionData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi reaction v1",
        &(
            data.space_id,
            data.message_id,
            data.person_id,
            data.device_id,
            &data.handle,
            data.kind,
            data.created_at,
            data.remove,
        ),
    )
}
pub fn settings_signing_bytes(data: &SettingsChangeData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi settings v1",
        &(
            data.space_id,
            data.issuer_device,
            data.created_at,
            data.settings.retention_days,
            data.settings.summary_mode,
            data.settings.rate_limits.messages_per_minute,
            data.settings.rate_limits.messages_per_hour,
        ),
    )
}
pub fn certify_device(
    data: DeviceCertificateData,
    issuer: &DeviceKeys,
) -> Result<DeviceCertificate, CryptoError> {
    let signature = issuer.sign(&certificate_signing_bytes(&data)?);
    let result = DeviceCertificate { data, signature };
    result.validate()?;
    Ok(result)
}
pub fn certify_with_grant(
    data: DeviceCertificateData,
    issuer: &GrantKeys,
) -> Result<DeviceCertificate, CryptoError> {
    if data.issuer
        != (CertificateIssuer::Grant {
            grant_id: issuer.id(),
        })
    {
        return Err(CryptoError::Trust("grant issuer mismatch"));
    }
    let signature = issuer.sign(&certificate_signing_bytes(&data)?);
    let result = DeviceCertificate { data, signature };
    result.validate()?;
    Ok(result)
}
pub fn sign_grant(data: GrantRecordData, issuer: &DeviceKeys) -> Result<GrantRecord, CryptoError> {
    let signature = issuer.sign(&grant_signing_bytes(&data)?);
    let result = GrantRecord { data, signature };
    result.validate()?;
    Ok(result)
}
pub fn sign_revocation(
    data: RevocationData,
    issuer: &DeviceKeys,
) -> Result<Revocation, CryptoError> {
    Ok(Revocation {
        signature: issuer.sign(&revocation_signing_bytes(&data)?),
        data,
    })
}
pub fn sign_reaction(data: ReactionData, issuer: &DeviceKeys) -> Result<Reaction, CryptoError> {
    Ok(Reaction {
        signature: issuer.sign(&reaction_signing_bytes(&data)?),
        data,
    })
}
pub fn sign_settings(
    data: SettingsChangeData,
    issuer: &DeviceKeys,
) -> Result<SettingsChange, CryptoError> {
    data.settings.validate()?;
    Ok(SettingsChange {
        signature: issuer.sign(&settings_signing_bytes(&data)?),
        data,
    })
}

/// Private construction prevents clients accidentally treating host-supplied keys as trusted.
#[derive(Clone, Debug)]
pub struct VerifiedDevice {
    certificate: DeviceCertificate,
}
impl VerifiedDevice {
    pub fn certificate(&self) -> &DeviceCertificate {
        &self.certificate
    }
}
#[derive(Debug)]
pub struct VerifiedDirectory {
    space_id: SpaceId,
    certificates: BTreeMap<DeviceId, DeviceCertificate>,
    revocations: Vec<Revocation>,
}
impl VerifiedDirectory {
    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }
    pub fn device(&self, id: DeviceId) -> Result<VerifiedDevice, CryptoError> {
        let cert = self
            .certificates
            .get(&id)
            .ok_or(CryptoError::Trust("unknown device"))?;
        if revoked_at(&self.revocations, &cert.data, u64::MAX) {
            return Err(CryptoError::Trust("revoked device"));
        }
        Ok(VerifiedDevice {
            certificate: cert.clone(),
        })
    }
    pub fn active_devices(&self) -> impl Iterator<Item = VerifiedDevice> + '_ {
        self.certificates
            .values()
            .filter(|c| !revoked_at(&self.revocations, &c.data, u64::MAX))
            .cloned()
            .map(|certificate| VerifiedDevice { certificate })
    }
    pub fn verify_message(&self, message: &Message) -> Result<VerifiedDevice, CryptoError> {
        let author = self.device(message.author_device)?;
        if message.envelope.space_id != self.space_id
            || message.author_person != author.certificate.data.person_id
        {
            return Err(CryptoError::Trust("message identity mismatch"));
        }
        crate::verify_message_signature(
            &author.certificate.data.sign_pub,
            &message.envelope,
            &message.signature,
        )?;
        Ok(author)
    }
    pub fn verify_reaction(&self, reaction: &Reaction) -> Result<(), CryptoError> {
        let author = self.device(reaction.data.device_id)?;
        if reaction.data.space_id != self.space_id
            || reaction.data.person_id != author.certificate.data.person_id
        {
            return Err(CryptoError::Trust("reaction identity mismatch"));
        }
        verify_signature(
            &author.certificate.data.sign_pub,
            &reaction_signing_bytes(&reaction.data)?,
            &reaction.signature,
        )
    }
    pub fn verify_settings(&self, change: &SettingsChange) -> Result<(), CryptoError> {
        change.validate()?;
        if change.data.space_id != self.space_id {
            return Err(CryptoError::Trust("settings space mismatch"));
        }
        let author = self.device(change.data.issuer_device)?;
        verify_signature(
            &author.certificate.data.sign_pub,
            &settings_signing_bytes(&change.data)?,
            &change.signature,
        )
    }
}
fn revoked_at(revocations: &[Revocation], cert: &DeviceCertificateData, at: Timestamp) -> bool {
    revocations.iter().any(|r| {
        r.data.created_at <= at
            && match r.data.target {
                RevocationTarget::Device { device_id } => device_id == cert.device_id,
                RevocationTarget::Person { person_id } => person_id == cert.person_id,
            }
    })
}
fn verify_cert(cert: &DeviceCertificate, issuer: &PublicKey) -> Result<(), CryptoError> {
    verify_signature(
        issuer,
        &certificate_signing_bytes(&cert.data)?,
        &cert.signature,
    )
}

/// Historical issuance remains valid after an issuer is revoked; revoked targets
/// cannot receive keys. The host must still supply the complete signed directory.
pub fn verify_trust_bundle(
    expected_space: SpaceId,
    bundle: &TrustBundle,
    now: Timestamp,
) -> Result<VerifiedDirectory, CryptoError> {
    let root = &bundle.root;
    root.validate()?;
    if root.data.space_id != expected_space
        || space_id(&root.data.sign_pub) != expected_space
        || root.data.issuer
            != (CertificateIssuer::Device {
                device_id: root.data.device_id,
            })
        || root.data.created_at > now
    {
        return Err(CryptoError::Trust("invalid space root"));
    }
    verify_cert(root, &root.data.sign_pub)?;
    let mut certificates = BTreeMap::from([(root.data.device_id, root.clone())]);
    let mut pending = BTreeMap::new();
    let mut grant_users = BTreeSet::new();
    for cert in &bundle.certificates {
        cert.validate()?;
        if cert.data.space_id != expected_space || cert.data.created_at > now {
            return Err(CryptoError::Trust("certificate space or time mismatch"));
        }
        if cert.data.device_id == root.data.device_id
            || pending.insert(cert.data.device_id, cert.clone()).is_some()
        {
            return Err(CryptoError::Trust("duplicate device certificate"));
        }
        if let CertificateIssuer::Grant { grant_id } = cert.data.issuer {
            if !grant_users.insert(grant_id) {
                return Err(CryptoError::Trust("grant used by multiple certificates"));
            }
        }
    }
    let mut grants = BTreeMap::new();
    let mut pending_grants = BTreeMap::new();
    for grant in &bundle.grants {
        grant.validate()?;
        if grant.data.space_id != expected_space || grant.data.created_at > now {
            return Err(CryptoError::Trust("grant space or time mismatch"));
        }
        if pending_grants
            .insert(grant.data.grant_id, grant.clone())
            .is_some()
        {
            return Err(CryptoError::Trust("duplicate grant record"));
        }
    }
    // Resolve dependencies iteratively, so hostile cycles cannot exhaust the stack.
    while !pending.is_empty() || !pending_grants.is_empty() {
        let before = pending.len() + pending_grants.len();
        for id in pending_grants.keys().copied().collect::<Vec<_>>() {
            let grant = &pending_grants[&id];
            if let Some(issuer) = certificates.get(&grant.data.issuer_device) {
                if grant.data.kind == GrantKind::Enroll
                    && grant.data.person_id != issuer.data.person_id
                {
                    return Err(CryptoError::Trust(
                        "enrollment must belong to the issuer's person",
                    ));
                }
                if grant.data.kind == GrantKind::Invite
                    && grant.data.person_id == issuer.data.person_id
                {
                    return Err(CryptoError::Trust("invite requires a new person"));
                }
                if grant.data.created_at < issuer.data.created_at {
                    return Err(CryptoError::Trust("grant predates issuer"));
                }
                verify_signature(
                    &issuer.data.sign_pub,
                    &grant_signing_bytes(&grant.data)?,
                    &grant.signature,
                )?;
                grants.insert(id, grant.clone());
                pending_grants.remove(&id);
            }
        }
        for id in pending.keys().copied().collect::<Vec<_>>() {
            let cert = &pending[&id];
            let issuer = match cert.data.issuer {
                CertificateIssuer::Device { device_id } => certificates
                    .get(&device_id)
                    .map(|issuer| (&issuer.data.sign_pub, issuer.data.created_at)),
                CertificateIssuer::Grant { grant_id } => {
                    if let Some(grant) = grants.get(&grant_id) {
                        if cert.data.person_id != grant.data.person_id
                            || grant
                                .data
                                .expires_at
                                .is_some_and(|end| cert.data.created_at >= end)
                        {
                            return Err(CryptoError::Trust("grant person or expiry mismatch"));
                        }
                        Some((&grant.data.grant_pub, grant.data.created_at))
                    } else {
                        None
                    }
                }
            };
            if let Some((issuer, created_at)) = issuer {
                if cert.data.created_at < created_at {
                    return Err(CryptoError::Trust("certificate predates issuer"));
                }
                verify_cert(cert, issuer)?;
                certificates.insert(id, cert.clone());
                pending.remove(&id);
            }
        }
        if before == pending.len() + pending_grants.len() {
            return Err(CryptoError::Trust("unanchored or cyclic chain"));
        }
    }
    let mut revocations = bundle.revocations.clone();
    revocations.sort_by_key(|r| r.data.created_at);
    let mut accepted = Vec::new();
    for revocation in revocations {
        let data = &revocation.data;
        let issuer = certificates
            .get(&data.issuer_device)
            .ok_or(CryptoError::Trust("unknown revocation issuer"))?;
        if data.space_id != expected_space
            || data.created_at > now
            || data.created_at < issuer.data.created_at
            || revoked_at(&accepted, &issuer.data, data.created_at)
        {
            return Err(CryptoError::Trust(
                "invalid revocation issuer, space or time",
            ));
        }
        let target_exists = match data.target {
            RevocationTarget::Device { device_id } => certificates.contains_key(&device_id),
            RevocationTarget::Person { person_id } => {
                certificates.values().any(|c| c.data.person_id == person_id)
            }
        };
        if !target_exists {
            return Err(CryptoError::Trust("unknown revocation target"));
        }
        verify_signature(
            &issuer.data.sign_pub,
            &revocation_signing_bytes(data)?,
            &revocation.signature,
        )?;
        accepted.push(revocation);
    }
    for grant in grants.values() {
        let issuer = &certificates[&grant.data.issuer_device];
        if revoked_at(&accepted, &issuer.data, grant.data.created_at) {
            return Err(CryptoError::Trust("grant issued by revoked device"));
        }
    }
    for cert in certificates
        .values()
        .filter(|cert| cert.data.device_id != root.data.device_id)
    {
        let issuer = match cert.data.issuer {
            CertificateIssuer::Device { device_id } => &certificates[&device_id],
            CertificateIssuer::Grant { grant_id } => {
                &certificates[&grants[&grant_id].data.issuer_device]
            }
        };
        if revoked_at(&accepted, &issuer.data, cert.data.created_at)
            || revoked_at(&accepted, &cert.data, cert.data.created_at)
        {
            return Err(CryptoError::Trust("certificate issued after revocation"));
        }
    }
    Ok(VerifiedDirectory {
        space_id: expected_space,
        certificates,
        revocations: accepted,
    })
}
