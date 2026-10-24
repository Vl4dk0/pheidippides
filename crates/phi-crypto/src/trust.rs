use crate::{CryptoError, DeviceKeys, GrantKeys, signing_bytes, space_id, verify_signature};
use phi_proto::*;
use std::collections::{BTreeMap, BTreeSet};

pub fn certificate_signing_bytes(data: &DeviceCertificateData) -> Result<Vec<u8>, CryptoError> {
    let issuer = match &data.issuer {
        CertificateIssuer::Device { device_id } => ("device", device_id.to_string()),
        CertificateIssuer::Grant { grant_id } => ("grant", grant_id.to_string()),
    };
    signing_bytes(
        "phi device certificate v2",
        &(
            data.space_id,
            data.person_id,
            data.device_id,
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
        "phi revocation v2",
        &(
            data.space_id,
            target,
            data.issuer_device,
            data.created_at,
            &data.history,
        ),
    )
}
/// BLAKE3 of the signature set. The empty list has one fixed root.
pub fn history_root(signatures: &[Signature]) -> Result<HistoryRoot, CryptoError> {
    if signatures.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(CryptoError::Trust("history must be strictly ascending"));
    }
    let mut hasher = blake3::Hasher::new_derive_key("phi revocation history v1");
    for signature in signatures {
        hasher.update(signature.as_bytes());
    }
    Ok(HistoryRoot::from_bytes(*hasher.finalize().as_bytes()))
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
pub fn thread_state_signing_bytes(data: &ThreadStateData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi thread state v1",
        &(
            data.space_id,
            data.thread_id,
            data.person_id,
            data.device_id,
            &data.handle,
            data.closed,
            data.created_at,
        ),
    )
}
pub fn thread_deletion_signing_bytes(data: &ThreadDeletionData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi thread delete v1",
        &(
            data.space_id,
            data.thread_id,
            data.person_id,
            data.device_id,
            &data.handle,
            data.created_at,
        ),
    )
}
pub fn settings_signing_bytes(data: &SettingsChangeData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi settings v3",
        &(
            data.space_id,
            data.issuer_device,
            data.created_at,
            &data.settings,
        ),
    )
}
pub fn certify_device(
    data: DeviceCertificateData,
    issuer: &DeviceKeys,
) -> Result<DeviceCertificate, CryptoError> {
    if data.issuer
        != (CertificateIssuer::Device {
            device_id: data.device_id,
        })
        || data.sign_pub != issuer.sign_public()
        || data.kex_pub != issuer.kex_public()
        || data.space_id != space_id(&data.sign_pub)
    {
        return Err(CryptoError::Trust(
            "device issuers are reserved for the self-signed root",
        ));
    }
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
pub fn sign_thread_state(
    data: ThreadStateData,
    issuer: &DeviceKeys,
) -> Result<ThreadState, CryptoError> {
    Ok(ThreadState {
        signature: issuer.sign(&thread_state_signing_bytes(&data)?),
        data,
    })
}
pub fn sign_thread_deletion(
    data: ThreadDeletionData,
    issuer: &DeviceKeys,
) -> Result<ThreadDeletion, CryptoError> {
    Ok(ThreadDeletion {
        signature: issuer.sign(&thread_deletion_signing_bytes(&data)?),
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
    pub(crate) context: [u8; 32],
    /// False when the device is revoked and this object is listed in every covering history.
    pub(crate) active: bool,
    certificate: DeviceCertificate,
}
impl VerifiedDevice {
    pub fn certificate(&self) -> &DeviceCertificate {
        &self.certificate
    }
    pub fn active(&self) -> bool {
        self.active
    }
}
/// Opaque local cache state. Deserialize only from the profile's trusted storage.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PinnedTrustState {
    bundle: TrustBundle,
}
impl PinnedTrustState {
    /// Local reads must observe persisted revocations without requiring the host's history.
    pub fn verify(&self, space: SpaceId, now: Timestamp) -> Result<VerifiedDirectory, CryptoError> {
        verify_trust_bundle(space, &self.bundle, now, Some(self))
    }
}

#[derive(Debug)]
pub struct VerifiedDirectory {
    context: [u8; 32],
    space_id: SpaceId,
    certificates: BTreeMap<DeviceId, DeviceCertificate>,
    root: DeviceCertificate,
    grants: BTreeMap<GrantId, GrantRecord>,
    grant_certificates: BTreeMap<GrantId, DeviceCertificate>,
    revocations: Vec<Revocation>,
    histories: BTreeMap<HistoryRoot, BTreeSet<Signature>>,
}
impl VerifiedDirectory {
    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }
    pub fn grant_certificates(&self) -> &BTreeMap<GrantId, DeviceCertificate> {
        &self.grant_certificates
    }
    pub fn revocations(&self) -> &[Revocation] {
        &self.revocations
    }
    /// Persist only after successful verification; reload as trusted prior state.
    pub fn pinned_state(&self) -> PinnedTrustState {
        PinnedTrustState {
            bundle: TrustBundle {
                root: self.root.clone(),
                certificates: self
                    .certificates
                    .values()
                    .filter(|c| c.data.device_id != self.root.data.device_id)
                    .cloned()
                    .collect(),
                grants: self.grants.values().cloned().collect(),
                revocations: self.revocations.clone(),
            },
        }
    }
    /// Revocation affects authorization, never the authenticated device-to-person binding.
    pub fn certificate(&self, id: DeviceId) -> Result<&DeviceCertificate, CryptoError> {
        self.certificates
            .get(&id)
            .ok_or(CryptoError::Trust("unknown device"))
    }
    pub fn trust_bundle(&self) -> TrustBundle {
        self.pinned_state().bundle
    }
    pub fn missing_histories(&self) -> Vec<HistoryRoot> {
        let mut missing = BTreeSet::new();
        for revocation in &self.revocations {
            if !self.histories.contains_key(&revocation.data.history) {
                missing.insert(revocation.data.history);
            }
        }
        missing.into_iter().collect()
    }
    pub fn attach_history(
        &mut self,
        root: HistoryRoot,
        list: &[Signature],
    ) -> Result<(), CryptoError> {
        if history_root(list)? != root {
            return Err(CryptoError::Trust("history root mismatch"));
        }
        self.histories.insert(root, list.iter().cloned().collect());
        Ok(())
    }
    pub fn device(&self, id: DeviceId) -> Result<VerifiedDevice, CryptoError> {
        let certificate = self.certificate(id)?.clone();
        if !self.active_device(id) {
            return Err(CryptoError::Revoked);
        }
        Ok(VerifiedDevice {
            context: self.context,
            active: true,
            certificate,
        })
    }
    pub fn active_devices(&self) -> impl Iterator<Item = VerifiedDevice> + '_ {
        self.certificates
            .values()
            .filter(|c| self.active_device(c.data.device_id))
            .cloned()
            .map(|certificate| VerifiedDevice {
                context: self.context,
                active: true,
                certificate,
            })
    }
    pub fn verify_message(&self, message: &Message) -> Result<VerifiedDevice, CryptoError> {
        let certificate = self.certificate(message.author_device)?.clone();
        if message.envelope.space_id != self.space_id
            || message.author_person != certificate.data.person_id
            || message.id != message.envelope.id
            || message.has_summary != message.envelope.summary_covers_seq.is_some()
            || message.summary_covers_seq != message.envelope.summary_covers_seq
        {
            return Err(CryptoError::Trust("message identity mismatch"));
        }
        crate::verify_message_signature(
            &certificate.data.sign_pub,
            &message.envelope,
            &message.signature,
        )?;
        let active = self.authorize(message.author_device, &message.signature)?;
        Ok(VerifiedDevice {
            context: self.context,
            active,
            certificate,
        })
    }
    /// Revoked reactions never count. A bad signature is an error; absence from a history is not.
    pub fn reaction_counts(&self, reaction: &Reaction) -> Result<bool, CryptoError> {
        let author = self.certificate(reaction.data.device_id)?;
        if reaction.data.space_id != self.space_id
            || reaction.data.person_id != author.data.person_id
        {
            return Err(CryptoError::Trust("reaction identity mismatch"));
        }
        verify_signature(
            &author.data.sign_pub,
            &reaction_signing_bytes(&reaction.data)?,
            &reaction.signature,
        )?;
        Ok(self.active_device(reaction.data.device_id))
    }
    pub fn verify_thread_state(&self, state: &ThreadState) -> Result<(), CryptoError> {
        let author = self.certificate(state.data.device_id)?;
        if state.data.space_id != self.space_id || state.data.person_id != author.data.person_id {
            return Err(CryptoError::Trust("thread state identity mismatch"));
        }
        verify_signature(
            &author.data.sign_pub,
            &thread_state_signing_bytes(&state.data)?,
            &state.signature,
        )?;
        self.authorize(state.data.device_id, &state.signature)?;
        Ok(())
    }
    pub fn verify_thread_deletion(&self, deletion: &ThreadDeletion) -> Result<(), CryptoError> {
        let author = self.certificate(deletion.data.device_id)?;
        if deletion.data.space_id != self.space_id
            || deletion.data.person_id != author.data.person_id
        {
            return Err(CryptoError::Trust("thread deletion identity mismatch"));
        }
        verify_signature(
            &author.data.sign_pub,
            &thread_deletion_signing_bytes(&deletion.data)?,
            &deletion.signature,
        )?;
        self.authorize(deletion.data.device_id, &deletion.signature)?;
        Ok(())
    }
    pub fn verify_settings(&self, change: &SettingsChange) -> Result<(), CryptoError> {
        change.validate()?;
        if change.data.space_id != self.space_id {
            return Err(CryptoError::Trust("settings space mismatch"));
        }
        let author = self.certificate(change.data.issuer_device)?;
        verify_signature(
            &author.data.sign_pub,
            &settings_signing_bytes(&change.data)?,
            &change.signature,
        )?;
        self.authorize(change.data.issuer_device, &change.signature)?;
        Ok(())
    }
    fn covers(revocation: &Revocation, cert: &DeviceCertificateData) -> bool {
        match revocation.data.target {
            RevocationTarget::Device { device_id } => device_id == cert.device_id,
            RevocationTarget::Person { person_id } => person_id == cert.person_id,
        }
    }
    fn revoked_cert(&self, cert: &DeviceCertificateData) -> bool {
        self.revocations
            .iter()
            .any(|revocation| Self::covers(revocation, cert))
    }
    /// A missing list is the empty set, so an unfetched history authorizes nothing.
    fn listed(&self, cert: &DeviceCertificateData, signature: &Signature) -> bool {
        self.revocations
            .iter()
            .filter(|revocation| Self::covers(revocation, cert))
            .all(|revocation| {
                self.histories
                    .get(&revocation.data.history)
                    .is_some_and(|set| set.contains(signature))
            })
    }
    fn chain_ok(&self, mut id: DeviceId) -> bool {
        // A missing certificate or grant fails closed. The length bound stops a cycle
        // the bundle verifier should already have rejected.
        for _ in 0..self.certificates.len().saturating_add(1) {
            let Some(cert) = self.certificates.get(&id) else {
                return false;
            };
            let CertificateIssuer::Grant { grant_id } = cert.data.issuer else {
                return true;
            };
            let signature = cert.signature.clone();
            let Some(grant) = self.grants.get(&grant_id) else {
                return false;
            };
            let issuer_id = grant.data.issuer_device;
            let Some(issuer) = self.certificates.get(&issuer_id).map(|c| c.data.clone()) else {
                return false;
            };
            if self.revoked_cert(&issuer) && !self.listed(&issuer, &signature) {
                return false;
            }
            id = issuer_id;
        }
        false
    }
    fn active_device(&self, id: DeviceId) -> bool {
        self.certificates
            .get(&id)
            .is_some_and(|cert| self.chain_ok(id) && !self.revoked_cert(&cert.data))
    }
    fn authorize(&self, id: DeviceId, signature: &Signature) -> Result<bool, CryptoError> {
        if !self.chain_ok(id) {
            return Err(CryptoError::Revoked);
        }
        let cert = self.certificate(id)?;
        if !self.revoked_cert(&cert.data) {
            return Ok(true);
        }
        if self.listed(&cert.data, signature) {
            Ok(false)
        } else {
            Err(CryptoError::Revoked)
        }
    }
}
fn verify_cert(cert: &DeviceCertificate, issuer: &PublicKey) -> Result<(), CryptoError> {
    verify_signature(
        issuer,
        &certificate_signing_bytes(&cert.data)?,
        &cert.signature,
    )
}

/// Pins are locally trusted previous output, not a second host-supplied directory.
/// Missing/conflicting immutable records fail closed; pinned revocations always merge.
pub fn verify_trust_bundle(
    expected_space: SpaceId,
    bundle: &TrustBundle,
    now: Timestamp,
    pinned: Option<&PinnedTrustState>,
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
    if let Some(prior) = pinned {
        if prior.bundle.root != *root {
            return Err(CryptoError::Trust("pinned root conflict"));
        }
        for cert in &prior.bundle.certificates {
            if !bundle.certificates.contains(cert) {
                return Err(CryptoError::Trust(
                    "pinned certificate missing or conflicting",
                ));
            }
        }
        for grant in &prior.bundle.grants {
            if !bundle.grants.contains(grant) {
                return Err(CryptoError::Trust("pinned grant missing or conflicting"));
            }
        }
    }
    let mut certificates = BTreeMap::from([(root.data.device_id, root.clone())]);
    let mut pending = BTreeMap::new();
    let mut grant_certificates = BTreeMap::new();
    for cert in &bundle.certificates {
        cert.validate()?;
        if cert.data.space_id != expected_space || cert.data.created_at > now {
            return Err(CryptoError::Trust("certificate space or time mismatch"));
        }
        let CertificateIssuer::Grant { grant_id } = cert.data.issuer else {
            return Err(CryptoError::Trust(
                "non-root certificate must be grant-signed",
            ));
        };
        if cert.data.device_id == root.data.device_id
            || pending.insert(cert.data.device_id, cert.clone()).is_some()
        {
            return Err(CryptoError::Trust("duplicate device certificate"));
        }
        if grant_certificates.insert(grant_id, cert.clone()).is_some() {
            return Err(CryptoError::Trust("grant used by multiple certificates"));
        }
    }
    let mut grants = BTreeMap::new();
    let mut pending_grants = BTreeMap::new();
    let mut person_origins = BTreeSet::from([root.data.person_id]);
    for grant in &bundle.grants {
        grant.validate()?;
        if grant.data.space_id != expected_space || grant.data.created_at > now {
            return Err(CryptoError::Trust("grant space or time mismatch"));
        }
        // A person's origin is unique; later devices of that person must enroll.
        if grant.data.kind == GrantKind::Invite && !person_origins.insert(grant.data.person_id) {
            return Err(CryptoError::Trust("invite person already exists"));
        }
        if pending_grants
            .insert(grant.data.grant_id, grant.clone())
            .is_some()
        {
            return Err(CryptoError::Trust("duplicate grant record"));
        }
    }
    // Iterative resolution makes cyclic hostile input bounded by directory size.
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
            let CertificateIssuer::Grant { grant_id } = cert.data.issuer else {
                return Err(CryptoError::Trust("non-root device issuer"));
            };
            if let Some(grant) = grants.get(&grant_id) {
                if cert.data.person_id != grant.data.person_id
                    || cert.data.created_at < grant.data.created_at
                    || grant
                        .data
                        .expires_at
                        .is_some_and(|end| cert.data.created_at >= end)
                {
                    return Err(CryptoError::Trust("grant person or validity mismatch"));
                }
                verify_cert(cert, &grant.data.grant_pub)?;
                certificates.insert(id, cert.clone());
                pending.remove(&id);
            }
        }
        if before == pending.len() + pending_grants.len() {
            return Err(CryptoError::Trust("unanchored or cyclic chain"));
        }
    }
    let mut revocations = bundle.revocations.clone();
    if let Some(prior) = pinned {
        for revocation in &prior.bundle.revocations {
            if !revocations.contains(revocation) {
                revocations.push(revocation.clone());
            }
        }
    }
    revocations.sort_by(|left, right| left.signature.as_bytes().cmp(right.signature.as_bytes()));
    revocations.dedup();
    for revocation in &revocations {
        let data = &revocation.data;
        let issuer = certificates
            .get(&data.issuer_device)
            .ok_or(CryptoError::Trust("unknown revocation issuer"))?;
        if data.space_id != expected_space
            || data.created_at > now
            || data.created_at < issuer.data.created_at
        {
            return Err(CryptoError::Trust("invalid revocation space or time"));
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
    }
    // Every valid signature takes effect. Whether a revoked key may authorize a
    // particular object is decided later, from the history that signature committed to.
    let context = *blake3::hash(&serde_json::to_vec(&(
        &root,
        &certificates,
        &grants,
        &revocations,
    ))?)
    .as_bytes();
    Ok(VerifiedDirectory {
        context,
        space_id: expected_space,
        root: root.clone(),
        certificates,
        grants,
        grant_certificates,
        revocations,
        histories: BTreeMap::new(),
    })
}

pub fn membership_signing_bytes(data: &ChannelMembershipData) -> Result<Vec<u8>, CryptoError> {
    signing_bytes(
        "phi channel membership v2",
        &(
            &data.previous,
            &data.key_commitment,
            data.space_id,
            data.channel_id,
            data.epoch,
            data.kind,
            &data.members,
            data.issuer_device,
        ),
    )
}
pub fn sign_membership(
    data: ChannelMembershipData,
    keys: &DeviceKeys,
) -> Result<ChannelMembership, CryptoError> {
    let result = ChannelMembership {
        signature: keys.sign(&membership_signing_bytes(&data)?),
        data,
    };
    result.validate()?;
    Ok(result)
}
#[derive(Clone, Debug)]
pub struct VerifiedMembership {
    pub(crate) context: [u8; 32],
    pub(crate) data: ChannelMembershipData,
    pub(crate) members: BTreeSet<PersonId>,
}
impl VerifiedMembership {
    pub fn data(&self) -> &ChannelMembershipData {
        &self.data
    }
    pub fn members(&self) -> &BTreeSet<PersonId> {
        &self.members
    }
}
impl VerifiedDirectory {
    /// Roster link checks. The issuer certificate may belong to a revoked device;
    /// authorization of that signature is decided by the channel walk.
    fn check_membership(
        &self,
        membership: &ChannelMembership,
        pinned: Option<&ChannelMembership>,
    ) -> Result<VerifiedMembership, CryptoError> {
        membership.validate()?;
        if membership.data.space_id != self.space_id {
            return Err(CryptoError::Trust("membership space mismatch"));
        }
        let issuer = self.certificate(membership.data.issuer_device)?;
        if let Some(prior) = pinned {
            // The prior roster must come from local accepted state, never this response.
            // The pinned record itself has no link to itself.
            let old = &prior.data;
            if old.space_id != membership.data.space_id
                || old.channel_id != membership.data.channel_id
                || old.kind != membership.data.kind
                || (old.epoch == membership.data.epoch
                    && old.key_commitment != membership.data.key_commitment)
                || old.epoch > membership.data.epoch
                || (membership != prior
                    && membership.data.previous.as_ref() != Some(&prior.signature))
                || (old.kind != ChannelKind::All && !old.members.contains(&issuer.data.person_id))
            {
                return Err(CryptoError::Trust(
                    "membership conflicts with pinned channel authority",
                ));
            }
        }
        if pinned.is_none()
            && membership.data.kind == ChannelKind::All
            && membership.data.issuer_device != self.root.data.device_id
        {
            return Err(CryptoError::Trust(
                "only the space root creates the all channel",
            ));
        }
        if pinned.is_none() && (membership.data.previous.is_some() || membership.data.epoch != 0) {
            return Err(CryptoError::Trust(
                "membership requires its previous authenticated roster",
            ));
        }
        if !membership.data.members.contains(&issuer.data.person_id) {
            return Err(CryptoError::Trust("membership issuer is not a member"));
        }
        verify_signature(
            &issuer.data.sign_pub,
            &membership_signing_bytes(&membership.data)?,
            &membership.signature,
        )?;
        let persons: BTreeSet<_> = self
            .certificates
            .values()
            .map(|c| c.data.person_id)
            .collect();
        if membership.data.members.iter().any(|p| !persons.contains(p)) {
            return Err(CryptoError::Trust("unknown membership person"));
        }
        let members = if membership.data.kind == ChannelKind::All {
            persons
        } else {
            membership.data.members.iter().copied().collect()
        };
        Ok(VerifiedMembership {
            context: self.context,
            data: membership.data.clone(),
            members,
        })
    }
    /// A locally proposed roster record must be signed by a device that is still active.
    pub fn verify_membership(
        &self,
        membership: &ChannelMembership,
        pinned: Option<&ChannelMembership>,
    ) -> Result<VerifiedMembership, CryptoError> {
        self.device(membership.data.issuer_device)?;
        self.check_membership(membership, pinned)
    }
    /// A revoked signer's roster record stands when an active signer extended it,
    /// or when every covering revocation lists that record's signature.
    pub fn verify_channel_history(
        &self,
        channel: &Channel,
        pinned: Option<&ChannelMembership>,
    ) -> Result<Vec<VerifiedMembership>, CryptoError> {
        let records: Vec<_> = channel
            .membership_history
            .iter()
            .chain(std::iter::once(&channel.membership))
            .collect();
        let start = match pinned {
            Some(prior) => {
                records
                    .iter()
                    .position(|record| *record == prior)
                    .ok_or(CryptoError::Trust(
                        "pinned channel membership missing or conflicting",
                    ))?
            }
            None => 0,
        };
        let mut previous = pinned;
        let mut chain = Vec::new();
        let mut unendorsed = Vec::new();
        for record in &records[start..] {
            let record = *record;
            chain.push(self.check_membership(record, previous)?);
            if self.active_device(record.data.issuer_device) {
                unendorsed.clear();
            } else {
                unendorsed.push(record);
            }
            previous = Some(record);
        }
        for record in unendorsed {
            self.authorize(record.data.issuer_device, &record.signature)?;
        }
        let head = chain
            .last()
            .ok_or(CryptoError::Trust("empty membership history"))?;
        if channel.space_id != head.data.space_id
            || channel.id != head.data.channel_id
            || channel.kind != head.data.kind
            || channel.current_epoch != head.data.epoch
            || channel.members.iter().copied().collect::<BTreeSet<_>>() != head.members
            || channel.members.len() != head.members.len()
        {
            return Err(CryptoError::Trust(
                "unsigned channel fields differ from authenticated membership",
            ));
        }
        Ok(chain)
    }
    pub fn verify_channel(
        &self,
        channel: &Channel,
        pinned: Option<&ChannelMembership>,
    ) -> Result<VerifiedMembership, CryptoError> {
        self.verify_channel_history(channel, pinned)?
            .pop()
            .ok_or(CryptoError::Trust("empty membership history"))
    }
}
