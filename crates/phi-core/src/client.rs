use crate::{
    storage::*,
    transport::{Http, token_string},
    *,
};
use phi_crypto::*;
use phi_proto::*;
use reqwest::Method;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelMeta {
    pub name: String,
    pub space_name: String,
    pub people: BTreeMap<PersonId, String>,
    pub members: Vec<PersonId>,
}
#[derive(Clone, Debug)]
pub struct NamedChannel {
    pub channel: Channel,
    pub meta: ChannelMeta,
}
#[derive(Default)]
pub struct SendOptions {
    pub body: String,
    pub summary: Option<String>,
    pub files: Vec<PathBuf>,
}
pub type KeyStoreFactory = Box<dyn FnOnce(&Profile) -> Result<Box<dyn KeyStore>> + Send>;

pub struct DeviceOptions {
    pub kind: DeviceKind,
    pub name: String,
    pub keystore: Option<KeyStoreFactory>,
}
impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            kind: DeviceKind::Cli,
            name: "cli".into(),
            keystore: None,
        }
    }
}
impl DeviceOptions {
    fn validate(&self) -> Result<()> {
        let _: Handle = self.name.parse()?;
        if self.kind == DeviceKind::Gui && self.keystore.is_none() {
            return Err(user("GUI devices require a keychain KeyStore"));
        }
        Ok(())
    }
    fn storage(self, store: &Store, profile: &Profile) -> Result<Box<dyn KeyStore>> {
        match self.keystore {
            Some(factory) => factory(profile),
            None => Ok(Box::new(FileKeyStore::open(
                &store.profile_dir(profile).join("keys"),
            )?)),
        }
    }
}

pub struct Client {
    pub profile: Profile,
    pub handle: Handle,
    pub channels: Vec<NamedChannel>,
    pub settings: SpaceSettings,
    pub devices: Vec<Device>,
    store: Store,
    keys: Box<dyn KeyStore>,
    identity: Identity,
    cache: Cache,
    http: Http,
    directory: VerifiedDirectory,
}
impl Client {
    pub async fn create(
        store: Store,
        server: &str,
        name: &str,
        display_name: &str,
        admin_token: Option<String>,
    ) -> Result<Self> {
        Self::create_for_device(
            store,
            server,
            name,
            display_name,
            admin_token,
            DeviceOptions::default(),
        )
        .await
    }
    pub async fn create_for_device(
        store: Store,
        server: &str,
        name: &str,
        display_name: &str,
        admin_token: Option<String>,
        device: DeviceOptions,
    ) -> Result<Self> {
        device.validate()?;
        validate_name(name)?;
        let keys = DeviceKeys::generate(&mut entropy)?;
        let sid = space_id(&keys.sign_public());
        let pid = PersonId::from_bytes(random_id(&mut entropy)?);
        let did = DeviceId::from_bytes(random_id(&mut entropy)?);
        let cert = certify_device(
            DeviceCertificateData {
                space_id: sid,
                person_id: pid,
                device_id: did,
                kind: device.kind,
                sign_pub: keys.sign_public(),
                kex_pub: keys.kex_public(),
                created_at: now(),
                issuer: CertificateIssuer::Device { device_id: did },
            },
            &keys,
        )?;
        let bundle = TrustBundle {
            root: cert.clone(),
            certificates: vec![],
            grants: vec![],
            revocations: vec![],
        };
        let directory = verify_trust_bundle(sid, &bundle, now(), None)?;
        let mut people = BTreeMap::new();
        people.insert(pid, unique_slug(display_name, &BTreeMap::new()));
        let meta = ChannelMeta {
            name: "all".into(),
            space_name: name.into(),
            people,
            members: vec![pid],
        };
        let (all, all_key) = channel_request(&cert, &keys, &directory, ChannelKind::All, &meta)?;
        let (own, own_key) = channel_request(
            &cert,
            &keys,
            &directory,
            ChannelKind::SelfChannel,
            &ChannelMeta {
                name: "self".into(),
                ..meta
            },
        )?;
        let http = Http::new(server, admin_token)?;
        let response: CreateSpaceResponse = http
            .send(
                Method::POST,
                "/spaces",
                &CreateSpaceRequest {
                    root_cert: cert.clone(),
                    all_channel: all,
                    self_channel: own,
                },
            )
            .await?;
        let profile = Profile {
            name: name.into(),
            server: http.base.clone(),
            space_id: sid,
            person_id: pid,
            device_id: did,
            device_name: device.name.clone(),
            display_name: unique_slug(display_name, &BTreeMap::new()),
            kind: device.kind,
            summarizer: None,
        };
        let (sign, kex) = keys.export_secrets();
        let identity = Identity {
            sign: KeyBytes::from_bytes(*sign),
            kex: KeyBytes::from_bytes(*kex),
            token: response.device_token,
            certificate: cert,
        };
        let key_store = device.storage(&store, &profile)?;
        store_json(key_store.as_ref(), "identity", &identity)?;
        save_channel_key(key_store.as_ref(), &all_key)?;
        save_channel_key(key_store.as_ref(), &own_key)?;
        let mut cache = Cache::open(&store.cache_dir(&profile).join("cache.db"))?;
        cache.trust(sid, &bundle)?;
        store.save_profile(&profile)?;
        Self::open_with_keystore(store, profile, None, key_store).await
    }
    pub async fn join(store: Store, encoded: &str, name: &str) -> Result<Self> {
        Self::join_for_device(store, encoded, name, DeviceOptions::default()).await
    }
    pub async fn join_for_device(
        store: Store,
        encoded: &str,
        name: &str,
        device: DeviceOptions,
    ) -> Result<Self> {
        device.validate()?;
        let link = GrantLink::parse(encoded)?;
        if store
            .profiles()?
            .iter()
            .any(|p| p.server == link.server_url() && p.space_id == link.space_id())
        {
            return Err(user(
                "a profile for this space already exists on this machine",
            ));
        }
        let grant_keys = link.secret().derive()?;
        let http = Http::new(&link.server_url(), None)?;
        let grant: GetGrantResponse = http.get(&format!("/grants/{}", grant_keys.id())).await?;
        grant.record.validate()?;
        let data = &grant.record.data;
        if data.space_id != link.space_id()
            || data.grant_id != grant_keys.id()
            || data.grant_pub != grant_keys.public()
        {
            return Err(user("grant does not match the link"));
        }
        if data.device_kind != device.kind {
            return Err(user("grant device kind does not match this application"));
        }
        let keys = DeviceKeys::generate(&mut entropy)?;
        let did = DeviceId::from_bytes(random_id(&mut entropy)?);
        let certificate = certify_with_grant(
            DeviceCertificateData {
                space_id: link.space_id(),
                person_id: data.person_id,
                device_id: did,
                kind: data.device_kind,
                sign_pub: keys.sign_public(),
                kex_pub: keys.kex_public(),
                created_at: now(),
                issuer: CertificateIssuer::Grant {
                    grant_id: grant_keys.id(),
                },
            },
            &grant_keys,
        )?;
        let response: RedeemGrantResponse = http
            .send(
                Method::POST,
                &format!("/grants/{}/redeem", grant_keys.id()),
                &RedeemGrantRequest {
                    certificate: certificate.clone(),
                },
            )
            .await?;
        if response.grant.record != grant.record
            || !response.directory.certificates.contains(&certificate)
        {
            return Err(user("redemption changed the signed grant or certificate"));
        }
        let directory = verify_trust_bundle(link.space_id(), &response.directory, now(), None)?;
        directory.device(did)?;
        let (sign, kex) = keys.export_secrets();
        let mut profile = Profile {
            name: link.space_id().to_string(),
            server: http.base,
            space_id: link.space_id(),
            person_id: data.person_id,
            device_id: did,
            device_name: device.name.clone(),
            display_name: unique_slug(name, &BTreeMap::new()),
            kind: device.kind,
            summarizer: None,
        };
        if store
            .profiles()?
            .iter()
            .any(|p| p.server == profile.server && p.space_id == profile.space_id)
        {
            return Err(user(
                "a profile for this space already exists on this machine",
            ));
        }
        let key_store = device.storage(&store, &profile)?;
        let identity = Identity {
            sign: KeyBytes::from_bytes(*sign),
            kex: KeyBytes::from_bytes(*kex),
            token: response.device_token,
            certificate,
        };
        store_json(key_store.as_ref(), "identity", &identity)?;
        let mut cache = Cache::open(&store.cache_dir(&profile).join("cache.db"))?;
        cache.trust(profile.space_id, &response.directory)?;
        let http = Http::new(&profile.server, Some(token_string(&identity.token)?))?;
        let channels: ListChannelsResponse = http
            .get(&format!("/spaces/{}/channels", profile.space_id))
            .await?;
        for sealed in &response.grant.sealed_keys {
            let key = unseal_grant_key(profile.space_id, &grant_keys, sealed)?;
            save_channel_key(key_store.as_ref(), &key)?;
        }
        for sealed in &response.grant.sealed_keys {
            let key = unseal_grant_key(profile.space_id, &grant_keys, sealed)?;
            let channel = channels
                .channels
                .iter()
                .find(|c| c.id == key.channel_id())
                .ok_or_else(|| user("grant channel missing"))?;
            let membership = directory.verify_channel(channel, None)?;
            let membership = if key.epoch() == channel.current_epoch {
                membership
            } else {
                let records: Vec<_> = channel
                    .membership_history
                    .iter()
                    .chain(std::iter::once(&channel.membership))
                    .collect();
                let index = records
                    .iter()
                    .rposition(|r| r.data.epoch == key.epoch())
                    .ok_or_else(|| user("historical epoch has no authenticated roster"))?;
                directory
                    .verify_membership(records[index], index.checked_sub(1).map(|i| records[i]))?
            };
            // Read and check encrypted peer context before giving a device any key.
            let meta_key: KeyBytes = load_json(
                key_store.as_ref(),
                &format!("channel/{}/{}", channel.id, channel.meta_epoch),
            )?;
            let meta_key = ChannelKey::from_bytes(
                profile.space_id,
                channel.id,
                channel.meta_epoch,
                *meta_key.as_bytes(),
            );
            let meta: ChannelMeta =
                serde_json::from_slice(&decrypt_channel_metadata(&meta_key, &channel.meta)?)?;
            validate_meta(channel, &meta, profile.person_id)?;
            let recipient = directory.device(did)?;
            let wrapped = wrap_channel_key(&key, &recipient, &membership, &mut entropy)?;
            let _: GetKeysResponse = http
                .send(
                    Method::PUT,
                    &format!("/channels/{}/keys", channel.id),
                    &PutKeysRequest {
                        membership: None,
                        epoch: key.epoch(),
                        wrapped_keys: vec![wrapped],
                    },
                )
                .await?;
            cache.channel(&directory, channel)?;
            save_channel_key(key_store.as_ref(), &key)?;
            if channel.kind == ChannelKind::All {
                profile.name = meta.space_name;
            }
        }
        store.save_profile(&profile)?;
        let mut client = Self::open_with_keystore(store, profile, None, key_store).await?;
        client.register_name(name).await?;
        if !client
            .channels
            .iter()
            .any(|c| c.channel.kind == ChannelKind::SelfChannel)
        {
            client
                .new_channel(
                    ChannelKind::SelfChannel,
                    "self",
                    vec![client.profile.person_id],
                )
                .await?;
        }
        Ok(client)
    }
    pub async fn open(store: Store, profile: Profile, handle: Option<&str>) -> Result<Self> {
        if profile.kind == DeviceKind::Gui {
            return Err(user(
                "this profile uses the GUI keychain; open it through the desktop app",
            ));
        }
        let keys = FileKeyStore::open(&store.profile_dir(&profile).join("keys"))?;
        Self::open_with_keystore(store, profile, handle, Box::new(keys)).await
    }
    pub async fn open_with_keystore(
        store: Store,
        profile: Profile,
        handle: Option<&str>,
        keys: Box<dyn KeyStore>,
    ) -> Result<Self> {
        let identity: Identity = load_json(keys.as_ref(), "identity")?;
        let cert = &identity.certificate.data;
        if cert.space_id != profile.space_id
            || cert.person_id != profile.person_id
            || cert.device_id != profile.device_id
            || cert.kind != profile.kind
            || cert.sign_pub != identity.keys().sign_public()
            || cert.kex_pub != identity.keys().kex_public()
        {
            return Err(user("profile and private identity do not match"));
        }
        let mut cache = Cache::open(&store.cache_dir(&profile).join("cache.db"))?;
        let http = Http::new(&profile.server, Some(token_string(&identity.token)?))?;
        let devices: ListDevicesResponse = http
            .get(&format!("/spaces/{}/devices", profile.space_id))
            .await?;
        let directory = cache.trust(profile.space_id, &devices.directory)?;
        directory.device(profile.device_id)?;
        let handle = handle.unwrap_or(&profile.device_name).parse()?;
        let mut client = Self {
            profile,
            handle,
            channels: vec![],
            settings: SpaceSettings::default(),
            devices: devices.devices,
            store,
            keys,
            identity,
            cache,
            http,
            directory,
        };
        client.refresh().await?;
        client.prune()?;
        Ok(client)
    }
    pub async fn refresh(&mut self) -> Result<()> {
        let sid = self.profile.space_id;
        let devices: ListDevicesResponse = self.http.get(&format!("/spaces/{sid}/devices")).await?;
        self.directory = self.cache.trust(sid, &devices.directory)?;
        self.directory.device(self.profile.device_id)?;
        self.devices = devices.devices;
        let space: GetSpaceResponse = self.http.get(&format!("/spaces/{sid}")).await?;
        if space.id != sid {
            return Err(user("space response crossed profile boundary"));
        }
        self.settings = space.settings;
        let channels: ListChannelsResponse =
            self.http.get(&format!("/spaces/{sid}/channels")).await?;
        let mut accepted = Vec::new();
        for channel in channels.channels {
            let prior = self
                .cache
                .get::<ChannelMembership>(&format!("roster/{}", channel.id))?;
            self.directory.verify_channel(&channel, prior.as_ref())?;
            let response: GetKeysResponse = self
                .http
                .get(&format!("/channels/{}/keys/mine", channel.id))
                .await?;
            for wrapped in response.wrapped_keys {
                if wrapped.channel_id != channel.id {
                    return Err(user("key response crossed channel boundary"));
                }
                let key = unwrap_channel_key(
                    sid,
                    self.profile.device_id,
                    &self.identity.keys(),
                    &wrapped,
                )?;
                save_channel_key(self.keys.as_ref(), &key)?;
            }
            let key = self.key(channel.id, channel.meta_epoch)?;
            let meta: ChannelMeta =
                serde_json::from_slice(&decrypt_channel_metadata(&key, &channel.meta)?)?;
            validate_meta(&channel, &meta, self.profile.person_id)?;
            self.cache.channel(&self.directory, &channel)?;
            accepted.push(NamedChannel { channel, meta });
        }
        self.channels = accepted;
        Ok(())
    }
    fn prune(&self) -> Result<()> {
        self.cache.prune(self.settings.retention_days)?;
        let blobs = self.store.cache_dir(&self.profile).join("blobs");
        if blobs.exists() {
            let mut stmt = self.cache.db.prepare("SELECT DISTINCT json_extract(value,'$.blob_id') FROM messages,json_each(messages.content,'$.attachments')")?;
            let live: BTreeSet<String> = stmt
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            for entry in std::fs::read_dir(blobs)? {
                let entry = entry?;
                if !live.contains(&entry.file_name().to_string_lossy().to_string()) {
                    std::fs::remove_dir_all(entry.path())?;
                }
            }
        }
        Ok(())
    }
    pub fn names(&self) -> BTreeMap<PersonId, String> {
        self.channels
            .iter()
            .find(|c| c.channel.kind == ChannelKind::All)
            .map(|c| c.meta.people.clone())
            .unwrap_or_default()
    }
    pub fn person_name(&self, id: PersonId) -> String {
        self.names()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }
    pub fn channel_name(&self, id: ChannelId) -> String {
        self.channels
            .iter()
            .find(|c| c.channel.id == id)
            .map(|c| match c.channel.kind {
                ChannelKind::All => "#all".into(),
                ChannelKind::Group => format!("#{}", c.meta.name),
                _ => format!(
                    "@{}",
                    self.person_name(
                        c.channel
                            .members
                            .iter()
                            .find(|p| **p != self.profile.person_id)
                            .copied()
                            .unwrap_or(self.profile.person_id)
                    )
                ),
            })
            .unwrap_or_else(|| id.to_string())
    }
    pub fn person(&self, name: &str) -> Result<PersonId> {
        let name = name.trim_start_matches('@');
        self.names()
            .iter()
            .find(|(id, n)| n.as_str() == name || id.to_string() == name)
            .map(|(id, _)| *id)
            .ok_or_else(|| user(format!("unknown person @{name}; run phi who")))
    }
    async fn register_name(&mut self, name: &str) -> Result<()> {
        let all = self
            .channels
            .iter()
            .find(|c| c.channel.kind == ChannelKind::All)
            .cloned()
            .ok_or_else(|| user("missing #all"))?;
        let mut meta = all.meta.clone();
        if let Some(existing) = meta.people.get(&self.profile.person_id) {
            self.profile.display_name = existing.clone();
        } else {
            let name = unique_slug(name, &meta.people);
            meta.people.insert(self.profile.person_id, name.clone());
            self.profile.display_name = name;
            self.update_channel(&all.channel, meta, all.channel.members.clone())
                .await?;
        }
        self.store.save_profile(&self.profile)?;
        self.refresh().await
    }
    fn key(&self, channel: ChannelId, epoch: Epoch) -> Result<ChannelKey> {
        let key: KeyBytes = load_json(self.keys.as_ref(), &format!("channel/{channel}/{epoch}"))?;
        Ok(ChannelKey::from_bytes(
            self.profile.space_id,
            channel,
            epoch,
            *key.as_bytes(),
        ))
    }
    pub async fn invite(&mut self, enroll: bool, never_expires: bool) -> Result<Zeroizing<String>> {
        self.invite_for_device(enroll, never_expires, self.profile.kind)
            .await
    }
    pub async fn invite_for_device(
        &mut self,
        enroll: bool,
        never_expires: bool,
        kind: DeviceKind,
    ) -> Result<Zeroizing<String>> {
        self.refresh().await?;
        if !enroll && never_expires {
            return Err(user("only enrollment grants may have no expiry"));
        }
        let secret = GrantSecret::generate(&mut entropy)?;
        let grant = secret.derive()?;
        let record = sign_grant(
            GrantRecordData {
                space_id: self.profile.space_id,
                grant_id: grant.id(),
                grant_pub: grant.public(),
                kind: if enroll {
                    GrantKind::Enroll
                } else {
                    GrantKind::Invite
                },
                device_kind: kind,
                person_id: if enroll {
                    self.profile.person_id
                } else {
                    PersonId::from_bytes(random_id(&mut entropy)?)
                },
                created_at: now(),
                expires_at: if never_expires {
                    None
                } else {
                    Some(now() + 7 * 86400)
                },
                issuer_device: self.profile.device_id,
            },
            &self.identity.keys(),
        )?;
        let mut sealed_keys = Vec::new();
        for channel in &self.channels {
            if enroll || channel.channel.kind == ChannelKind::All {
                let membership = self
                    .directory
                    .verify_channel(&channel.channel, Some(&channel.channel.membership))?;
                if !membership.members().contains(&self.profile.person_id) {
                    return Err(user("not a channel member"));
                }
                // Share history as well as the current epoch, including metadata encrypted under an older key.
                for epoch in 0..=channel.channel.current_epoch {
                    if epoch != channel.channel.current_epoch
                        && self
                            .keys
                            .get(&format!("channel/{}/{epoch}", channel.channel.id))?
                            .is_none()
                    {
                        continue;
                    }
                    sealed_keys.push(seal_grant_key(
                        &grant,
                        &self.key(channel.channel.id, epoch)?,
                        &mut entropy,
                    )?);
                }
            }
        }
        let _: CreateGrantResponse = self
            .http
            .send(
                Method::POST,
                &format!("/spaces/{}/grants", self.profile.space_id),
                &CreateGrantRequest {
                    record,
                    sealed_keys,
                },
            )
            .await?;
        let url =
            reqwest::Url::parse(&self.profile.server).map_err(|_| user("invalid server URL"))?;
        let authority = self
            .profile
            .server
            .split_once("://")
            .ok_or_else(|| user("invalid server origin"))?
            .1;
        Ok(GrantLink::new(
            url.scheme() == "https",
            authority.to_owned(),
            self.profile.space_id,
            secret,
        )?
        .encode())
    }
    async fn new_channel(
        &mut self,
        kind: ChannelKind,
        name: &str,
        mut members: Vec<PersonId>,
    ) -> Result<ChannelId> {
        members.sort();
        members.dedup();
        let meta = ChannelMeta {
            name: name.into(),
            space_name: self.profile.name.clone(),
            people: BTreeMap::new(),
            members,
        };
        let (request, key) = channel_request(
            &self.identity.certificate,
            &self.identity.keys(),
            &self.directory,
            kind,
            &meta,
        )?;
        let channel: CreateChannelResponse = self
            .http
            .send(
                Method::POST,
                &format!("/spaces/{}/channels", self.profile.space_id),
                &request,
            )
            .await?;
        self.cache.channel(&self.directory, &channel)?;
        save_channel_key(self.keys.as_ref(), &key)?;
        let id = channel.id;
        self.refresh().await?;
        Ok(id)
    }
    async fn destination(&mut self, to: &str) -> Result<(ChannelId, Vec<Mention>)> {
        if let Some(name) = to.strip_prefix('#') {
            let channel = self
                .channels
                .iter()
                .find(|c| {
                    matches!(c.channel.kind, ChannelKind::All | ChannelKind::Group)
                        && c.meta.name == name
                })
                .ok_or_else(|| user(format!("unknown channel #{name}")))?;
            return Ok((channel.channel.id, vec![]));
        }
        let recipient = to
            .strip_prefix('@')
            .ok_or_else(|| user("address must be @person[/handle] or #group"))?;
        let (name, handle) = recipient
            .split_once('/')
            .map_or((recipient, None), |(n, h)| (n, Some(h)));
        let person = self.person(name)?;
        let mentions = vec![Mention {
            person_id: person,
            handle: handle.map(str::parse).transpose()?,
        }];
        let kind = if person == self.profile.person_id {
            ChannelKind::SelfChannel
        } else {
            ChannelKind::Dm
        };
        let mut members = vec![self.profile.person_id, person];
        members.sort();
        members.dedup();
        if let Some(c) = self.channels.iter().find(|c| {
            c.channel.kind == kind
                && c.channel.members.iter().copied().collect::<BTreeSet<_>>()
                    == members.iter().copied().collect()
        }) {
            return Ok((c.channel.id, mentions));
        }
        Ok((self.new_channel(kind, "dm", members).await?, mentions))
    }
    pub async fn send(&mut self, to: &str, title: &str, options: SendOptions) -> Result<Message> {
        self.refresh().await?;
        reject_links(&options.body)?;
        let (channel, mentions) = self.destination(to).await?;
        self.send_inner(channel, None, Some(title.into()), mentions, options, &[])
            .await
    }
    pub async fn reply(&mut self, thread: MessageId, options: SendOptions) -> Result<Message> {
        self.refresh().await?;
        reject_links(&options.body)?;
        let history = self.load_thread(thread).await?;
        let root = history.first().ok_or_else(|| user("thread not found"))?;
        let channel = root.message.envelope.channel_id;
        self.send_inner(channel, Some(thread), None, vec![], options, &history)
            .await
    }
    async fn send_inner(
        &mut self,
        channel: ChannelId,
        thread: Option<MessageId>,
        title: Option<String>,
        mentions: Vec<Mention>,
        mut options: SendOptions,
        history: &[DecodedMessage],
    ) -> Result<Message> {
        self.ensure_safe_epoch(channel).await?;
        if options.summary.is_none() && history.iter().any(|m| m.verification_error.is_some()) {
            return Err(user(
                "thread contains unverifiable or unavailable messages; supply an explicit whole-thread summary after reviewing the errors",
            ));
        }
        if options.summary.is_none() {
            if let Some(command) = &self.profile.summarizer {
                let mut input = summary_input(history);
                input.push_str(&format!(
                    "\nNew message (included in summary):\n{}\n{}\n",
                    title.as_deref().unwrap_or(""),
                    options.body
                ));
                options.summary = Some(run_summarizer(command, &input).await?);
            } else if self.settings.summary_mode == SummaryMode::Required
                && self.profile.kind == DeviceKind::Cli
            {
                return Err(user(
                    "this space requires --summary of the whole thread, or a summarizer in profile.toml",
                ));
            }
        }
        if let Some(summary) = &options.summary {
            reject_links(summary)?;
        }
        if let Some(title) = &title {
            reject_links(title)?;
        }
        if options.files.len() > MAX_ATTACHMENTS {
            return Err(user("at most 32 attachments"));
        }
        let mut attachments = Vec::new();
        for path in options.files {
            let metadata = std::fs::metadata(&path)?;
            if metadata.len() > DEFAULT_MAX_ATTACHMENT_BYTES {
                return Err(user("attachment exceeds default 25 MiB file limit"));
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| user("attachment filename must be UTF-8"))?
                .to_owned();
            let plaintext = Zeroizing::new(std::fs::read(&path)?);
            let (key, encrypted) = encrypt_attachment(&plaintext, &mut entropy)?;
            let blob: UploadBlobResponse = self
                .http
                .send(
                    Method::POST,
                    "/blobs",
                    &UploadBlobRequest {
                        space_id: self.profile.space_id,
                        ciphertext: encrypted.ciphertext,
                    },
                )
                .await?;
            attachments.push(Attachment {
                blob_id: blob.id,
                mime: mime(&name).into(),
                name,
                size: plaintext.len() as u64,
                key,
                nonce: encrypted.nonce,
            });
        }
        let epoch = self
            .channels
            .iter()
            .find(|c| c.channel.id == channel)
            .ok_or_else(|| user("channel missing"))?
            .channel
            .current_epoch;
        let content = MessageContent {
            title,
            body: options.body,
            summary: options.summary,
            attachments,
            display_name: self.profile.display_name.clone(),
        };
        let id = MessageId::from_bytes(random_id(&mut entropy)?);
        let coverage = content
            .summary
            .as_ref()
            .map(|_| history.last().map_or(0, |m| m.message.seq));
        let (envelope, signature) = encrypt_message(
            &self.key(channel, epoch)?,
            &self.identity.keys(),
            MessageMetadata {
                id,
                summary_covers_seq: coverage,
                thread_id: thread,
                handle: self.handle.clone(),
                mentions,
            },
            &content,
            &mut entropy,
        )?;
        let message: SendMessageResponse = self
            .http
            .send(
                Method::POST,
                &format!("/channels/{channel}/messages"),
                &SendMessageRequest {
                    id,
                    envelope,
                    signature,
                    has_summary: content.summary.is_some(),
                    blob_ids: content.attachments.iter().map(|a| a.blob_id).collect(),
                },
            )
            .await?;
        self.directory.verify_message(&message)?;
        self.cache.message(&message, &content)?;
        Ok(message)
    }
    pub async fn counts(&mut self) -> Result<Counts> {
        self.http
            .get(&format!(
                "/spaces/{}/counts?handle={}",
                self.profile.space_id, self.handle
            ))
            .await
    }
    pub async fn fetch(&mut self) -> Result<String> {
        let counts = self.counts().await?;
        if counts.unread_threads == 0 && counts.mentions == 0 && counts.claimed_by_you == 0 {
            return Ok(String::new());
        }
        let channels = counts
            .channels
            .iter()
            .map(|c| format!("{} {}", self.channel_name(c.channel_id), c.unread_threads))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "phi · {}: {} unread threads ({channels}) · {} mention(s) of you (@{}/{}) · {} claimed by you\n",
            self.profile.name,
            counts.unread_threads,
            counts.mentions,
            self.profile.display_name,
            self.handle,
            counts.claimed_by_you
        ))
    }
    async fn sync_channel(
        &mut self,
        channel: ChannelId,
        thread: Option<MessageId>,
    ) -> Result<Vec<DecodedMessage>> {
        let mut after = 0;
        let mut decoded = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            let page: ListMessagesResponse = self
                .http
                .get(&format!(
                    "/channels/{channel}/messages?after={after}&limit=1000{}",
                    thread.map(|id| format!("&thread={id}")).unwrap_or_default()
                ))
                .await?;
            let count = page.messages.len();
            for message in page.messages {
                if message.envelope.channel_id != channel
                    || message.envelope.space_id != self.profile.space_id
                    || message.seq <= after
                    || !seen.insert(message.id)
                {
                    return Err(user(
                        "message page crossed scope, duplicated an ID, or changed order",
                    ));
                }
                after = message.seq;
                let verified = (|| -> Result<_> {
                    let author = self.directory.verify_message(&message)?;
                    let content = decrypt_message(
                        &self.key(channel, message.envelope.epoch)?,
                        &author,
                        &message.envelope,
                        &message.signature,
                    )?;
                    Ok((author.certificate().data.kind, content))
                })();
                let (author_kind, content, verification_error) = match verified {
                    Ok((kind, content)) => {
                        self.cache.message(&message, &content)?;
                        (kind, content, None)
                    }
                    Err(error) => {
                        let description = error.to_string();
                        (
                            DeviceKind::Cli,
                            MessageContent {
                                title: message
                                    .envelope
                                    .thread_id
                                    .is_none()
                                    .then(|| "[unavailable encrypted title]".into()),
                                body: String::new(),
                                summary: None,
                                attachments: vec![],
                                display_name: String::new(),
                            },
                            Some(description),
                        )
                    }
                };
                decoded.push(DecodedMessage {
                    message,
                    content,
                    attachments: vec![],
                    reactions: vec![],
                    author_kind,
                    verification_error,
                });
            }
            for cursor in page.cursors {
                if cursor.person_id != self.profile.person_id {
                    return Err(user("read cursor crossed person boundary"));
                }
                self.cache.db.execute("INSERT INTO cursors VALUES(?,?) ON CONFLICT(thread) DO UPDATE SET seq=excluded.seq", params![cursor.thread_id.to_string(), cursor.last_read_seq])?;
            }
            // A page contains authoritative current reactions for each returned message, including empty lists.
            for message in decoded.iter().rev().take(count) {
                self.cache.db.execute(
                    "DELETE FROM reactions WHERE message=?",
                    [message.message.id.to_string()],
                )?;
            }
            for reaction in page.reactions {
                if !decoded
                    .iter()
                    .rev()
                    .take(count)
                    .any(|m| m.message.id == reaction.data.message_id)
                {
                    return Err(user("reaction outside message page"));
                }
                self.directory.verify_reaction(&reaction)?;
                self.save_reaction(&reaction)?;
            }
            if count < 1000 {
                break;
            }
        }
        for message in &mut decoded {
            message.reactions = self.reactions(message.message.id)?;
        }
        Ok(decoded)
    }
    async fn sync_all(&mut self) -> Result<Vec<DecodedMessage>> {
        let ids: Vec<_> = self.channels.iter().map(|c| c.channel.id).collect();
        let mut messages = Vec::new();
        for channel in ids {
            messages.extend(self.sync_channel(channel, None).await?);
        }
        messages.sort_by_key(|m| m.message.seq);
        Ok(messages)
    }
    async fn load_thread(&mut self, thread: MessageId) -> Result<Vec<DecodedMessage>> {
        let root: Message = self.http.get(&format!("/messages/{thread}")).await?;
        if root.id != thread || root.envelope.thread_id.is_some() {
            return Err(user("read requires a thread root ID"));
        }
        self.directory.verify_message(&root)?;
        let messages = self
            .sync_channel(root.envelope.channel_id, Some(thread))
            .await?;
        let messages: Vec<_> = messages
            .into_iter()
            .filter(|m| m.message.id == thread || m.message.envelope.thread_id == Some(thread))
            .collect();
        if messages
            .first()
            .is_none_or(|m| m.message.id != thread || m.content.title.is_none())
        {
            return Err(user(
                "thread not found; use the full thread ID from phi inbox",
            ));
        }
        Ok(messages)
    }
    pub async fn read(&mut self, thread: MessageId, options: ReadOptions) -> Result<ThreadView> {
        self.refresh().await?;
        let messages = self.load_thread(thread).await?;
        let latest = messages.last().map_or(0, |m| m.message.seq);
        let mut view = compose_thread(thread, messages, options, &self.names())?;
        for message in &mut view.messages {
            for attachment in &message.content.attachments {
                message.attachments.push(self.download(attachment).await?);
            }
        }
        let _: PutCursorResponse = self
            .http
            .send(
                Method::PUT,
                &format!("/threads/{thread}/cursor"),
                &PutCursorRequest {
                    last_read_seq: latest,
                },
            )
            .await?;
        self.cache.db.execute(
            "INSERT INTO cursors VALUES(?,?) ON CONFLICT(thread) DO UPDATE SET seq=excluded.seq",
            params![thread.to_string(), latest],
        )?;
        Ok(view)
    }
    async fn download(&mut self, attachment: &Attachment) -> Result<PathBuf> {
        attachment.validate()?;
        let mut components = std::path::Path::new(&attachment.name).components();
        if attachment.name.contains(':')
            || !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(user(
                "attachment filename must be a contained, portable path segment",
            ));
        }
        let dir = self
            .store
            .cache_dir(&self.profile)
            .join("blobs")
            .join(attachment.blob_id.to_string());
        private_dir(&dir)?;
        let path = dir.join(&attachment.name);
        // Re-authenticate ciphertext even when a prior plaintext copy exists.
        let blob: DownloadBlobResponse = self
            .http
            .get(&format!("/blobs/{}", attachment.blob_id))
            .await?;
        if blob.blob.id != attachment.blob_id || blob.blob.space_id != self.profile.space_id {
            return Err(user("attachment response crossed scope"));
        }
        let plaintext = decrypt_attachment(
            &attachment.key,
            &Ciphertext {
                nonce: attachment.nonce.clone(),
                ciphertext: blob.ciphertext,
            },
        )?;
        if plaintext.len() as u64 != attachment.size {
            return Err(user("attachment size mismatch"));
        }
        atomic_write(&path, &plaintext)?;
        Ok(path)
    }
    pub async fn inbox(&mut self) -> Result<Vec<InboxEntry>> {
        self.refresh().await?;
        let messages = self.sync_all().await?;
        let mut threads: BTreeMap<MessageId, Vec<DecodedMessage>> = BTreeMap::new();
        for message in messages {
            threads
                .entry(
                    message
                        .message
                        .envelope
                        .thread_id
                        .unwrap_or(message.message.id),
                )
                .or_default()
                .push(message);
        }
        let mut result = Vec::new();
        for (id, thread) in threads {
            let cursor: u64 = self
                .cache
                .db
                .query_row(
                    "SELECT seq FROM cursors WHERE thread=?",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            let latest = thread.last().ok_or_else(|| user("empty thread"))?;
            if latest.message.seq <= cursor {
                continue;
            }
            let root = thread.first().ok_or_else(|| user("empty thread"))?;
            let mentions = thread.iter().filter(|m| m.message.seq > cursor).any(|m| {
                m.message.envelope.mentions.iter().any(|mention| {
                    mention.person_id == self.profile.person_id
                        && mention.handle.as_ref().is_none_or(|h| h == &self.handle)
                })
            });
            result.push(InboxEntry {
                id,
                channel: self.channel_name(root.message.envelope.channel_id),
                title: root.content.title.clone().unwrap_or_default(),
                from: format!(
                    "{}/{}",
                    self.person_name(latest.message.author_person),
                    latest.message.envelope.handle
                ),
                created_at: latest.message.created_at,
                summary: thread
                    .iter()
                    .find_map(|m| {
                        m.verification_error
                            .as_ref()
                            .map(|e| format!("VERIFICATION FAILED / CONTENT UNAVAILABLE: {e}"))
                    })
                    .or_else(|| thread.iter().rev().find_map(|m| m.content.summary.clone())),
                mentioned: mentions,
                reactions: thread.iter().flat_map(|m| m.reactions.clone()).collect(),
                latest_seq: latest.message.seq,
            });
        }
        result.sort_by_key(|e| std::cmp::Reverse(e.latest_seq));
        Ok(result)
    }
    fn reactions(&self, message: MessageId) -> Result<Vec<Reaction>> {
        let mut stmt = self
            .cache
            .db
            .prepare("SELECT raw FROM reactions WHERE message=?")?;
        let raws = stmt
            .query_map([message.to_string()], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        raws.into_iter()
            .map(|s| {
                let reaction: Reaction = serde_json::from_str(&s)?;
                self.directory.verify_reaction(&reaction)?;
                Ok(reaction)
            })
            .collect()
    }
    fn save_reaction(&self, reaction: &Reaction) -> Result<()> {
        let d = &reaction.data;
        let kind = serde_json::to_string(&d.kind)?;
        if d.remove {
            if d.kind == ReactionKind::Claim {
                self.cache.db.execute(
                    "DELETE FROM reactions WHERE message=? AND person=? AND kind=?",
                    params![d.message_id.to_string(), d.person_id.to_string(), kind],
                )?;
            } else {
                self.cache.db.execute(
                    "DELETE FROM reactions WHERE message=? AND person=? AND handle=? AND kind=?",
                    params![
                        d.message_id.to_string(),
                        d.person_id.to_string(),
                        d.handle.as_str(),
                        kind
                    ],
                )?;
            }
        } else {
            self.cache.db.execute("INSERT INTO reactions VALUES(?,?,?,?,?) ON CONFLICT(message,person,handle,kind) DO UPDATE SET raw=excluded.raw", params![d.message_id.to_string(), d.person_id.to_string(), d.handle.as_str(), kind, serde_json::to_string(reaction)?])?;
        }
        let thread: Option<String> = self
            .cache
            .db
            .query_row(
                "SELECT thread FROM messages WHERE id=?",
                [d.message_id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(thread) = thread {
            self.cache.activity(thread.parse()?, d.created_at)?;
        }
        Ok(())
    }
    pub async fn react(
        &mut self,
        message: MessageId,
        kind: ReactionKind,
        remove: bool,
    ) -> Result<Reaction> {
        self.refresh().await?;
        let reaction = sign_reaction(
            ReactionData {
                space_id: self.profile.space_id,
                message_id: message,
                person_id: self.profile.person_id,
                device_id: self.profile.device_id,
                handle: self.handle.clone(),
                kind,
                created_at: now(),
                remove,
            },
            &self.identity.keys(),
        )?;
        let path = if remove {
            format!(
                "/messages/{message}/reactions/{}",
                serde_json::to_value(kind)?
                    .as_str()
                    .ok_or_else(|| user("invalid reaction"))?
            )
        } else {
            format!("/messages/{message}/reactions")
        };
        let response: ReactionResponse = self
            .http
            .send(
                if remove { Method::DELETE } else { Method::POST },
                &path,
                &ReactionRequest {
                    reaction: reaction.clone(),
                },
            )
            .await?;
        self.directory.verify_reaction(&response)?;
        if response != reaction {
            return Err(user("server changed reaction"));
        }
        self.save_reaction(&response)?;
        Ok(response)
    }
    pub async fn unread(&mut self, thread: MessageId) -> Result<()> {
        self.refresh().await?;
        let messages = self.load_thread(thread).await?;
        let latest = messages
            .last()
            .ok_or_else(|| user("thread not found"))?
            .message
            .seq;
        let cursor = latest.saturating_sub(1);
        let _: PutCursorResponse = self
            .http
            .send(
                Method::PUT,
                &format!("/threads/{thread}/cursor"),
                &PutCursorRequest {
                    last_read_seq: cursor,
                },
            )
            .await?;
        self.cache.db.execute(
            "INSERT INTO cursors VALUES(?,?) ON CONFLICT(thread) DO UPDATE SET seq=excluded.seq",
            params![thread.to_string(), cursor],
        )?;
        Ok(())
    }
    pub async fn wait(&mut self, seconds: u16) -> Result<Vec<WaitEntry>> {
        if seconds > 900 {
            return Err(user("wait supports 0–900 seconds"));
        }
        let name = format!("wait/{}", self.handle);
        let after = self.cache.get::<u64>(&name)?.unwrap_or(0);
        let response: EventsResponse = self
            .http
            .get(&format!(
                "/spaces/{}/events?after={after}&timeout={seconds}&handle={}",
                self.profile.space_id, self.handle
            ))
            .await?;
        if response.next_after < after {
            return Err(user("server rolled back event cursor"));
        }
        if response.events.is_empty() {
            self.advance_wait(&name, response.next_after)?;
            return Ok(vec![]);
        }
        self.refresh().await?;
        let mut entries = Vec::new();
        let mut previous = after;
        for event in response.events {
            if event.space_id != self.profile.space_id
                || event.seq <= previous
                || event.seq > response.next_after
            {
                return Err(user("invalid event sequence or scope"));
            }
            previous = event.seq;
            let entry = match event.event {
                EventData::Message(message) => {
                    let author = self.directory.verify_message(&message)?;
                    let content = decrypt_message(
                        &self.key(message.envelope.channel_id, message.envelope.epoch)?,
                        &author,
                        &message.envelope,
                        &message.signature,
                    )?;
                    self.cache.message(&message, &content)?;
                    WaitEntry::Message(InboxEntry {
                        id: message.envelope.thread_id.unwrap_or(message.id),
                        channel: self.channel_name(message.envelope.channel_id),
                        title: content.title.unwrap_or_else(|| {
                            format!(
                                "reply to {}",
                                message
                                    .envelope
                                    .thread_id
                                    .map(|id| id.to_string())
                                    .unwrap_or_default()
                            )
                        }),
                        from: format!(
                            "{}/{}",
                            self.person_name(message.author_person),
                            message.envelope.handle
                        ),
                        created_at: message.created_at,
                        summary: content.summary,
                        mentioned: message.envelope.mentions.iter().any(|m| {
                            m.person_id == self.profile.person_id
                                && m.handle.as_ref().is_none_or(|h| h == &self.handle)
                        }),
                        reactions: vec![],
                        latest_seq: message.seq,
                    })
                }
                EventData::Reaction(reaction) => {
                    self.directory.verify_reaction(&reaction)?;
                    self.save_reaction(&reaction)?;
                    WaitEntry::Reaction(reaction)
                }
                EventData::Settings(change) => {
                    self.directory.verify_settings(&change)?;
                    WaitEntry::Settings(change)
                }
                EventData::Revocation(revocation) => {
                    if !self.directory.revocations().contains(&revocation) {
                        return Err(user("unverified revocation event"));
                    }
                    WaitEntry::Revocation(revocation)
                }
            };
            entries.push(entry);
        }
        // Advance only after the whole response verified. Errors preserve the backlog for inspection.
        self.advance_wait(&name, response.next_after)?;
        Ok(entries)
    }
    fn advance_wait(&self, name: &str, sequence: u64) -> Result<()> {
        self.cache.db.execute("INSERT INTO state VALUES(?,?) ON CONFLICT(name) DO UPDATE SET value=CAST(MAX(CAST(state.value AS INTEGER),CAST(excluded.value AS INTEGER)) AS TEXT)", params![name, sequence.to_string()])?;
        Ok(())
    }
    pub async fn group_create(&mut self, name: &str, persons: &[String]) -> Result<ChannelId> {
        validate_name(name)?;
        self.refresh().await?;
        if name == "all"
            || self
                .channels
                .iter()
                .any(|c| c.channel.kind == ChannelKind::Group && c.meta.name == name)
        {
            return Err(user("group name already exists"));
        }
        let mut members = vec![self.profile.person_id];
        for person in persons {
            members.push(self.person(person)?);
        }
        self.new_channel(ChannelKind::Group, name, members).await
    }
    async fn update_channel(
        &mut self,
        channel: &Channel,
        mut meta: ChannelMeta,
        members: Vec<PersonId>,
    ) -> Result<Channel> {
        meta.members = members.clone();
        let shrinking = channel
            .members
            .iter()
            .any(|person| !members.contains(person));
        let key = if shrinking {
            self.key(channel.id, channel.current_epoch)?
                .rotate(&mut entropy)?
        } else {
            self.key(channel.id, channel.current_epoch)?
        };
        let membership = sign_membership(
            ChannelMembershipData {
                previous: Some(channel.membership.signature.clone()),
                space_id: self.profile.space_id,
                channel_id: channel.id,
                epoch: key.epoch(),
                kind: channel.kind,
                members: members.clone(),
                issuer_device: self.profile.device_id,
            },
            &self.identity.keys(),
        )?;
        let verified = self
            .directory
            .verify_membership(&membership, Some(&channel.membership))?;
        let mut wrapped_keys = Vec::new();
        if shrinking {
            for device in self.directory.active_devices() {
                if members.contains(&device.certificate().data.person_id) {
                    wrapped_keys.push(wrap_channel_key(&key, &device, &verified, &mut entropy)?);
                }
            }
        }
        let meta = encrypt_channel_metadata(&key, &serde_json::to_vec(&meta)?, &mut entropy)?;
        let response: UpdateChannelResponse = self
            .http
            .send(
                Method::PATCH,
                &format!("/channels/{}", channel.id),
                &UpdateChannelRequest {
                    wrapped_keys,
                    membership: membership.clone(),
                    meta_epoch: key.epoch(),
                    meta,
                    members,
                },
            )
            .await?;
        if response.id != channel.id || response.membership != membership {
            return Err(user("server changed membership publication"));
        }
        self.cache.channel(&self.directory, &response)?;
        if shrinking {
            save_channel_key(self.keys.as_ref(), &key)?;
        }
        Ok(response)
    }
    pub async fn group_member(&mut self, name: &str, person: &str, remove: bool) -> Result<()> {
        self.refresh().await?;
        let person = self.person(person)?;
        let channel = self
            .channels
            .iter()
            .find(|c| {
                c.channel.kind == ChannelKind::Group && c.meta.name == name.trim_start_matches('#')
            })
            .cloned()
            .ok_or_else(|| user("group not found"))?;
        let mut members = channel.channel.members.clone();
        if remove {
            members.retain(|p| *p != person);
        } else if !members.contains(&person) {
            members.push(person);
        }
        members.sort();
        if !members.contains(&self.profile.person_id) {
            return Err(user(
                "cannot remove yourself while responsible for the key rotation",
            ));
        }
        let channel = self
            .update_channel(&channel.channel, channel.meta, members)
            .await?;
        if !remove {
            self.rotate(&channel).await?;
        }
        self.refresh().await
    }
    async fn ensure_safe_epoch(&mut self, channel_id: ChannelId) -> Result<()> {
        let named = self
            .channels
            .iter()
            .find(|c| c.channel.id == channel_id)
            .cloned()
            .ok_or_else(|| user("channel missing"))?;
        let protected: BTreeSet<Signature> = self
            .cache
            .get(&format!("protected/{channel_id}"))?
            .unwrap_or_default();
        let relevant = self.directory.revocations().iter().any(|r| {
            if protected.contains(&r.signature) {
                return false;
            }
            match r.data.target {
                RevocationTarget::Person { person_id } => {
                    named.channel.members.contains(&person_id)
                }
                RevocationTarget::Device { device_id } => self.devices.iter().any(|d| {
                    d.cert.data.device_id == device_id
                        && named.channel.members.contains(&d.cert.data.person_id)
                }),
            }
        });
        if !relevant {
            return Ok(());
        }
        let removed: BTreeSet<_> = self
            .directory
            .revocations()
            .iter()
            .filter_map(|r| match r.data.target {
                RevocationTarget::Person { person_id } => Some(person_id),
                _ => None,
            })
            .collect();
        let original_epoch = named.channel.current_epoch;
        let channel = if named.channel.kind == ChannelKind::Group
            && named.channel.members.iter().any(|p| removed.contains(p))
        {
            let members = named
                .channel
                .members
                .iter()
                .filter(|p| !removed.contains(p))
                .copied()
                .collect();
            self.update_channel(&named.channel, named.meta, members)
                .await?
        } else {
            named.channel
        };
        // An issuer may not know another person's private groups. Their remaining members
        // must rekey before sending, even if another client may already have rotated once.
        if channel.current_epoch == original_epoch {
            self.rotate(&channel).await?;
        }
        self.refresh().await
    }
    async fn rotate(&mut self, channel: &Channel) -> Result<()> {
        let key = self
            .key(channel.id, channel.current_epoch)?
            .rotate(&mut entropy)?;
        let proof = sign_membership(
            ChannelMembershipData {
                previous: Some(channel.membership.signature.clone()),
                epoch: key.epoch(),
                members: channel.members.clone(),
                issuer_device: self.profile.device_id,
                ..channel.membership.data.clone()
            },
            &self.identity.keys(),
        )?;
        let membership = self
            .directory
            .verify_membership(&proof, Some(&channel.membership))?;
        let mut wrapped_keys = Vec::new();
        for device in self.directory.active_devices() {
            if membership
                .members()
                .contains(&device.certificate().data.person_id)
            {
                wrapped_keys.push(wrap_channel_key(&key, &device, &membership, &mut entropy)?);
            }
        }
        let _: GetKeysResponse = self
            .http
            .send(
                Method::PUT,
                &format!("/channels/{}/keys", channel.id),
                &PutKeysRequest {
                    membership: Some(proof.clone()),
                    epoch: key.epoch(),
                    wrapped_keys,
                },
            )
            .await?;
        // The server also stores our self-wrap, so refresh can recover a committed key after a crash.
        // Unaccepted proposals must never occupy immutable epoch accounts during concurrent rotations.
        save_channel_key(self.keys.as_ref(), &key)?;
        let meta: ChannelMeta = serde_json::from_slice(&decrypt_channel_metadata(
            &self.key(channel.id, channel.meta_epoch)?,
            &channel.meta,
        )?)?;
        let mut rotated = channel.clone();
        rotated.membership_history.push(rotated.membership.clone());
        rotated.membership = proof;
        rotated.current_epoch = key.epoch();
        self.update_channel(&rotated, meta, channel.members.clone())
            .await?;
        let protected: BTreeSet<Signature> = self
            .directory
            .revocations()
            .iter()
            .map(|r| r.signature.clone())
            .collect();
        self.cache
            .set(&format!("protected/{}", channel.id), &protected)?;
        Ok(())
    }
    pub async fn revoke(&mut self, target: RevocationTarget) -> Result<()> {
        self.refresh().await?;
        let affected_person = match target {
            RevocationTarget::Person { person_id } => person_id,
            RevocationTarget::Device { device_id } => {
                self.devices
                    .iter()
                    .find(|d| d.cert.data.device_id == device_id)
                    .ok_or_else(|| user("device not found"))?
                    .cert
                    .data
                    .person_id
            }
        };
        if matches!(target, RevocationTarget::Device { device_id } if device_id == self.profile.device_id)
            || matches!(target, RevocationTarget::Person { person_id } if person_id == self.profile.person_id)
        {
            return Err(user(
                "use another device/person to revoke this device so it can finish key rotation",
            ));
        }
        let revocation = sign_revocation(
            RevocationData {
                space_id: self.profile.space_id,
                target: target.clone(),
                issuer_device: self.profile.device_id,
                created_at: now(),
            },
            &self.identity.keys(),
        )?;
        let _: CreateRevocationResponse = self
            .http
            .send(
                Method::POST,
                &format!("/spaces/{}/revocations", self.profile.space_id),
                &CreateRevocationRequest { revocation },
            )
            .await?;
        self.refresh().await?;
        let channels: Vec<_> = self
            .channels
            .iter()
            .filter(|c| c.channel.members.contains(&affected_person))
            .map(|c| c.channel.clone())
            .collect();
        for channel in channels {
            let original_epoch = channel.current_epoch;
            let channel = if matches!(target, RevocationTarget::Person { .. })
                && channel.kind == ChannelKind::Group
            {
                let mut members = channel.members.clone();
                members.retain(|p| *p != affected_person);
                let meta: ChannelMeta = serde_json::from_slice(&decrypt_channel_metadata(
                    &self.key(channel.id, channel.meta_epoch)?,
                    &channel.meta,
                )?)?;
                self.update_channel(&channel, meta, members).await?
            } else {
                channel
            };
            if channel.current_epoch == original_epoch {
                self.rotate(&channel).await?;
            }
        }
        self.refresh().await
    }
    pub async fn set_settings(&mut self, key: &str, value: &str) -> Result<SettingsChange> {
        self.refresh().await?;
        let mut settings = self.settings.clone();
        match key {
            "retention" | "retention_days" => {
                settings.retention_days = value
                    .parse()
                    .map_err(|_| user("retention must be a number of days (0 means forever)"))?
            }
            "summary_mode" => {
                settings.summary_mode = match value {
                    "off" => SummaryMode::Off,
                    "required" => SummaryMode::Required,
                    _ => return Err(user("summary_mode must be required or off")),
                }
            }
            _ => return Err(user("setting must be retention_days or summary_mode")),
        }
        let change = sign_settings(
            SettingsChangeData {
                space_id: self.profile.space_id,
                issuer_device: self.profile.device_id,
                created_at: now(),
                settings,
            },
            &self.identity.keys(),
        )?;
        let response: UpdateSettingsResponse = self
            .http
            .send(
                Method::PATCH,
                &format!("/spaces/{}/settings", self.profile.space_id),
                &UpdateSettingsRequest {
                    change: change.clone(),
                },
            )
            .await?;
        self.directory.verify_settings(&response)?;
        if response != change {
            return Err(user("server changed settings event"));
        }
        self.settings = response.data.settings.clone();
        Ok(response)
    }
    pub async fn search(&mut self, text: &str) -> Result<Vec<DecodedMessage>> {
        self.refresh().await?;
        let mut stmt = self.cache.db.prepare(
            "SELECT raw,content FROM messages WHERE instr(lower(COALESCE(json_extract(content,'$.title'),'') || char(10) || json_extract(content,'$.body') || char(10) || COALESCE(json_extract(content,'$.summary'),'')),lower(?))>0 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map([text], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(raw, _)| {
                let message: Message = serde_json::from_str(&raw)?;
                let author = self.directory.verify_message(&message)?;
                let content = decrypt_message(
                    &self.key(message.envelope.channel_id, message.envelope.epoch)?,
                    &author,
                    &message.envelope,
                    &message.signature,
                )?;
                Ok(DecodedMessage {
                    message,
                    content,
                    attachments: vec![],
                    reactions: vec![],
                    author_kind: author.certificate().data.kind,
                    verification_error: None,
                })
            })
            .collect()
    }
    pub async fn who(&mut self) -> Result<String> {
        self.refresh().await?;
        let mut output = String::new();
        for (person, name) in self.names() {
            output.push_str(&format!("@{name} ({person})\n"));
            for device in self
                .devices
                .iter()
                .filter(|d| d.cert.data.person_id == person)
            {
                output.push_str(&format!(
                    "  {} · {:?} · last seen {}{}\n",
                    device.cert.data.device_id,
                    device.cert.data.kind,
                    device
                        .last_seen_at
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "never".into()),
                    if device.revoked_at.is_some() {
                        " · revoked"
                    } else {
                        ""
                    }
                ));
            }
        }
        for channel in &self.channels {
            if matches!(channel.channel.kind, ChannelKind::Group | ChannelKind::All) {
                output.push_str(&format!(
                    "{}: {}\n",
                    self.channel_name(channel.channel.id),
                    channel
                        .channel
                        .members
                        .iter()
                        .map(|p| format!("@{}", self.person_name(*p)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let mut stmt = self.cache.db.prepare("SELECT DISTINCT json_extract(raw,'$.author_person'),json_extract(raw,'$.envelope.handle') FROM messages WHERE created>? ORDER BY 1,2")?;
        for row in stmt.query_map([now().saturating_sub(7 * 86400)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (person, handle) = row?;
            output.push_str(&format!(
                "recent handle: {}/{handle}\n",
                self.person_name(person.parse()?)
            ));
        }
        Ok(output)
    }
}

fn channel_request(
    cert: &DeviceCertificate,
    keys: &DeviceKeys,
    directory: &VerifiedDirectory,
    kind: ChannelKind,
    meta: &ChannelMeta,
) -> Result<(CreateChannelRequest, ChannelKey)> {
    let id = ChannelId::from_bytes(random_id(&mut entropy)?);
    let key = ChannelKey::generate(cert.data.space_id, id, &mut entropy)?;
    let membership = sign_membership(
        ChannelMembershipData {
            previous: None,
            space_id: cert.data.space_id,
            channel_id: id,
            epoch: 0,
            kind,
            members: meta.members.clone(),
            issuer_device: cert.data.device_id,
        },
        keys,
    )?;
    let verified = directory.verify_membership(&membership, None)?;
    let mut wrapped_keys = Vec::new();
    for device in directory.active_devices() {
        if verified
            .members()
            .contains(&device.certificate().data.person_id)
        {
            wrapped_keys.push(wrap_channel_key(&key, &device, &verified, &mut entropy)?);
        }
    }
    let encrypted = encrypt_channel_metadata(&key, &serde_json::to_vec(meta)?, &mut entropy)?;
    Ok((
        CreateChannelRequest {
            membership,
            id,
            space_id: cert.data.space_id,
            kind,
            meta: encrypted,
            members: meta.members.clone(),
            wrapped_keys,
        },
        key,
    ))
}
fn save_channel_key(store: &dyn KeyStore, key: &ChannelKey) -> Result<()> {
    let account = format!("channel/{}/{}", key.channel_id(), key.epoch());
    let secret = KeyBytes::from_bytes(*key.export_secret());
    if let Some(old) = store.get(&account)? {
        let old: KeyBytes = serde_json::from_slice(&old)?;
        if old != secret {
            return Err(user("server changed an accepted channel epoch key"));
        }
        return Ok(());
    }
    store_json(store, &account, &secret)
}
fn validate_meta(channel: &Channel, meta: &ChannelMeta, person: PersonId) -> Result<()> {
    validate_name(&meta.name)?;
    validate_name(&meta.space_name)?;
    if channel.kind != ChannelKind::All
        && meta.members.iter().copied().collect::<BTreeSet<_>>()
            != channel.members.iter().copied().collect()
    {
        return Err(user(
            "encrypted channel peer context differs from signed roster",
        ));
    }
    if !channel.members.contains(&person)
        || (channel.kind == ChannelKind::All && meta.name != "all")
        || (channel.kind == ChannelKind::SelfChannel && channel.members != vec![person])
        || (channel.kind == ChannelKind::Dm && channel.members.len() != 2)
    {
        return Err(user("channel context does not match expected peers"));
    }
    let mut names = BTreeSet::new();
    for name in meta.people.values() {
        validate_name(name)?;
        if !names.insert(name) {
            return Err(user("duplicate encrypted person name"));
        }
    }
    Ok(())
}
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
    {
        return Err(user(
            "name must use lowercase letters, numbers, -, _ or . (1–255 characters)",
        ));
    }
    Ok(())
}
fn unique_slug(name: &str, people: &BTreeMap<PersonId, String>) -> String {
    let base: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(220)
        .collect();
    let base = if base.is_empty() {
        "person".into()
    } else {
        base
    };
    let mut candidate = base.clone();
    let mut suffix = 2;
    while people.values().any(|n| n == &candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}
fn reject_links(body: &str) -> Result<()> {
    let lowercase = body.to_ascii_lowercase();
    if lowercase.contains("phi://") || lowercase.contains("phis://") {
        return Err(user(
            "phi links contain secret grant keys; send the link outside phi",
        ));
    }
    Ok(())
}
fn mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "txt" | "md" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}
async fn run_summarizer(command: &str, input: &str) -> Result<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    #[cfg(windows)]
    let mut process = {
        let mut p = tokio::process::Command::new("cmd");
        p.arg("/C").arg(command);
        p
    };
    #[cfg(not(windows))]
    let mut process = {
        let mut p = tokio::process::Command::new("sh");
        p.arg("-c").arg(command);
        p
    };
    let mut child = process
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| user("summarizer stdin unavailable"))?;
    let writer = async {
        stdin.write_all(input.as_bytes()).await?;
        drop(stdin);
        std::io::Result::Ok(())
    };
    let (written, output) = tokio::join!(writer, child.wait_with_output());
    // Constant commands (explicitly supported by the spec) may successfully close stdin early.
    if let Err(error) = written {
        if error.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(error.into());
        }
    }
    let output = output?;
    if !output.status.success() {
        return Err(user(format!("summarizer failed: {}", output.status)));
    }
    let summary = String::from_utf8(output.stdout)
        .map_err(|_| user("summarizer output is not UTF-8"))?
        .trim()
        .to_owned();
    if summary.is_empty() {
        return Err(user("summarizer returned an empty summary"));
    }
    Ok(summary)
}
