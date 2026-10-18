use anyhow::{Result, ensure};
use phi_core::*;
use phi_proto::*;
use phi_server::{
    Server,
    config::{Config, Creation},
};
use std::{path::PathBuf, time::Duration};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    url: String,
    server: Server,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn new() -> Result<Self> {
        Self::configured(|_| {}).await
    }
    async fn configured(update: impl FnOnce(&mut Config)) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let mut config = Config {
            data_dir: dir.path().join("server"),
            allow_space_creation: Creation::Anyone,
            ..Config::default()
        };
        update(&mut config);
        let server = Server::open(config)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = server.router();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self {
            dir,
            url,
            server,
            task,
        })
    }
    fn store(&self, name: &str) -> Store {
        Store::new(
            self.dir.path().join(name).join("config"),
            self.dir.path().join(name).join("cache"),
        )
    }
    async fn alice(&self) -> Result<Client> {
        Ok(Client::create(self.store("alice"), &self.url, "game", "alice", None).await?)
    }
    async fn pair(&self) -> Result<(Client, Client)> {
        let mut alice = self.alice().await?;
        let link = alice.invite(false, false).await?;
        let bob = Client::join(self.store("bob"), &link, "bob").await?;
        alice.refresh().await?;
        Ok((alice, bob))
    }
}
fn options(body: &str) -> SendOptions {
    SendOptions {
        body: body.into(),
        summary: Some(format!("Summary: {body}")),
        files: vec![],
    }
}

#[tokio::test]
async fn create_invite_join_attachment_fetch_read_reply() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let path = f.dir.path().join("shot.png");
    std::fs::write(&path, b"image bytes")?;
    let mut message = options("Please review repo foo at abc123.");
    message.files.push(path);
    let root = alice.send("@bob/cli", "Review PR", message).await?;
    ensure!(bob.fetch().await?.contains("1 unread threads"));
    ensure!(bob.fetch().await?.contains("1 mention"));
    let inbox = bob.inbox().await?;
    ensure!(inbox.len() == 1 && inbox[0].mentioned && inbox[0].title == "Review PR");
    let view = bob.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("UNTRUSTED") && view.text().contains("alice/cli (agent)"));
    ensure!(std::fs::read(&view.messages[0].attachments[0])? == b"image bytes");
    ensure!(bob.fetch().await?.is_empty());
    let reply = bob.reply(root.id, options("Looks good")).await?;
    let view = alice
        .read(
            root.id,
            ReadOptions {
                all: true,
                last: None,
            },
        )
        .await?;
    ensure!(view.messages.len() == 2 && view.summary_covers_seq == Some(root.seq));
    ensure!(view.messages[1].message.id == reply.id && view.newer_messages == 0);
    Ok(())
}
#[tokio::test]
async fn claim_exclusivity_reactions_unread_and_person_cursor() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let invite = alice.invite(false, false).await?;
    let mut carol = Client::join(f.store("carol"), &invite, "carol").await?;
    let root = alice.send("#all", "Task", options("review")).await?;
    carol.wait(0).await?;
    bob.react(root.id, ReactionKind::Claim, false).await?;
    ensure!(bob.fetch().await?.contains("1 claimed by you"));
    let events = carol.wait(0).await?;
    ensure!(events.iter().any(|e|matches!(e,WaitEntry::Reaction(r) if r.data.message_id==root.id && r.data.kind==ReactionKind::Claim)));
    let view = carol.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("claimed by bob/cli"));
    ensure!(
        alice
            .react(root.id, ReactionKind::Claim, false)
            .await
            .is_err()
    );
    alice.react(root.id, ReactionKind::Ack, false).await?;
    let view = alice.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("claimed by bob/cli") && view.text().contains("ack×1"));
    bob.react(root.id, ReactionKind::Claim, true).await?;
    ensure!(carol.wait(0).await?.iter().any(
        |e| matches!(e,WaitEntry::Reaction(r) if r.data.kind==ReactionKind::Claim && r.data.remove)
    ));
    ensure!(
        !carol
            .read(root.id, ReadOptions::default())
            .await?
            .text()
            .contains("claimed by bob/cli")
    );

    alice.react(root.id, ReactionKind::Claim, false).await?;
    bob.read(root.id, ReadOptions::default()).await?;
    ensure!(bob.counts().await?.unread_threads == 0);
    bob.unread(root.id).await?;
    ensure!(bob.counts().await?.unread_threads == 1);
    let mut second = Client::open(f.store("bob"), bob.profile.clone(), Some("other")).await?;
    second.read(root.id, ReadOptions::default()).await?;
    ensure!(bob.counts().await?.unread_threads == 0);
    Ok(())
}
#[tokio::test]
async fn wait_backlog_wakes_on_reply_and_keeps_per_handle_cursor() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let root = alice.send("@bob", "Task", options("hello")).await?;
    ensure!(
        bob.wait(0)
            .await?
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(_)))
    );
    ensure!(bob.wait(0).await?.is_empty());
    let mut parallel = Client::open(f.store("bob"), bob.profile.clone(), None).await?;
    let second_waiter = parallel.wait(3);
    let waiter = bob.wait(3);
    let sender = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        alice.reply(root.id, options("new reply")).await
    };
    let (events, second_events, message) = tokio::join!(waiter, second_waiter, sender);
    ensure!(
        second_events?
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(_)))
    );
    message?;
    ensure!(events?.iter().any(|e| matches!(e, WaitEntry::Message(_))));
    ensure!(bob.wait(0).await?.is_empty());
    let mut second = Client::open(f.store("bob"), bob.profile.clone(), Some("second")).await?;
    ensure!(!second.wait(0).await?.is_empty());
    let mut reopened = Client::open(f.store("bob"), bob.profile.clone(), None).await?;
    ensure!(reopened.wait(0).await?.is_empty());
    Ok(())
}
#[tokio::test]
async fn summaries_required_off_explicit_and_local_summarizer() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    ensure!(
        alice
            .send("#all", "No summary", SendOptions::default())
            .await
            .is_err()
    );
    alice.profile.summarizer = Some("echo local-summary".into());
    let root = alice
        .send(
            "#all",
            "Local summary",
            SendOptions {
                body: "x".repeat(100_000),
                ..Default::default()
            },
        )
        .await?;
    ensure!(
        alice
            .read(root.id, ReadOptions::default())
            .await?
            .summary
            .as_deref()
            == Some("local-summary")
    );
    let explicit = alice
        .send("#all", "Explicit", options("explicit wins"))
        .await?;
    ensure!(
        alice
            .read(explicit.id, ReadOptions::default())
            .await?
            .summary
            .as_deref()
            == Some("Summary: explicit wins")
    );
    alice.profile.summarizer = Some("exit 9".into());
    ensure!(
        alice
            .send("#all", "Failure", SendOptions::default())
            .await
            .is_err()
    );
    alice.profile.summarizer = None;
    alice.set_settings("summary_mode", "off").await?;
    alice
        .send("#all", "No summary needed", SendOptions::default())
        .await?;
    Ok(())
}
#[tokio::test]
async fn read_composes_summary_coverage_tail_and_all() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let root = alice.send("#all", "Long thread", options("root")).await?;
    for n in 0..7 {
        alice.reply(root.id, options(&format!("reply {n}"))).await?;
    }
    alice.set_settings("summary_mode", "off").await?;
    for n in 0..2 {
        alice
            .reply(
                root.id,
                SendOptions {
                    body: format!("human-like {n}"),
                    ..Default::default()
                },
            )
            .await?;
    }
    let view = alice.read(root.id, ReadOptions::default()).await?;
    ensure!(view.messages.len() == 5 && view.newer_messages == 2);
    let view = alice
        .read(
            root.id,
            ReadOptions {
                last: Some(1),
                all: false,
            },
        )
        .await?;
    ensure!(view.messages.len() == 2);
    let view = alice
        .read(
            root.id,
            ReadOptions {
                last: None,
                all: true,
            },
        )
        .await?;
    ensure!(view.messages.len() == 10);
    Ok(())
}
#[tokio::test]
async fn enrollment_history_revoke_device_rotates_and_invalidates_token() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let root = alice.send("#all", "Old history", options("old")).await?;
    let link = alice.invite(true, true).await?;
    let mut laptop = Client::join(f.store("laptop"), &link, "ignored-enroll-name").await?;
    ensure!(
        laptop.profile.person_id == alice.profile.person_id
            && laptop.profile.display_name == "alice"
    );
    ensure!(
        laptop
            .read(root.id, ReadOptions::default())
            .await?
            .messages
            .len()
            == 1
    );
    alice
        .revoke(RevocationTarget::Device {
            device_id: laptop.profile.device_id,
        })
        .await?;
    ensure!(matches!(laptop.counts().await, Err(Error::Auth)));
    let new = alice.send("#all", "New key", options("private")).await?;
    ensure!(new.envelope.epoch == 1);
    ensure!(
        alice.read(new.id, ReadOptions::default()).await?.messages[0]
            .content
            .body
            == "private"
    );
    Ok(())
}
#[tokio::test]
async fn removal_rotates_all_dm_and_group_and_isolates_removed_person() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    alice.group_create("backend", &["@bob".into()]).await?;
    alice.send("@bob", "DM", options("before removal")).await?;
    alice
        .revoke(RevocationTarget::Person {
            person_id: bob.profile.person_id,
        })
        .await?;
    ensure!(matches!(bob.counts().await, Err(Error::Auth)));
    ensure!(
        alice
            .channels
            .iter()
            .filter(|c| c.channel.kind != ChannelKind::SelfChannel)
            .all(|c| c.channel.current_epoch == 1)
    );
    let root = alice
        .send("#all", "After removal", options("future secret"))
        .await?;
    ensure!(root.envelope.epoch == 1);
    alice.read(root.id, ReadOptions::default()).await?;
    Ok(())
}
#[tokio::test]
async fn groups_add_remove_rotate_and_enroll_after_rotation() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    alice.group_create("backend", &[]).await?;
    alice
        .send(
            "#backend",
            "Before Bob joined",
            options("private old history"),
        )
        .await?;
    alice.group_member("backend", "@bob", false).await?;
    let root = alice.send("#backend", "Group", options("shared")).await?;
    ensure!(
        bob.read(root.id, ReadOptions::default()).await?.messages[0]
            .content
            .body
            == "shared"
    );
    ensure!(
        bob.inbox()
            .await?
            .iter()
            .any(|e| e.title == "[unavailable encrypted title]")
    );
    let enrollment = bob.invite(true, false).await?;
    let mut bob_laptop = Client::join(f.store("bob-laptop"), &enrollment, "bob").await?;
    ensure!(
        bob_laptop
            .read(root.id, ReadOptions::default())
            .await?
            .messages[0]
            .content
            .body
            == "shared"
    );
    alice.group_member("backend", "@bob", true).await?;
    bob.refresh().await?;
    ensure!(!bob.channels.iter().any(|c| c.meta.name == "backend"));
    let link = alice.invite(true, false).await?;
    let mut laptop = Client::join(f.store("laptop"), &link, "alice").await?;
    let root = alice
        .send("#backend", "After rotation", options("new group key"))
        .await?;
    ensure!(
        laptop.read(root.id, ReadOptions::default()).await?.messages[0]
            .content
            .body
            == "new group key"
    );
    Ok(())
}
#[tokio::test]
async fn profile_resolution_binding_ambiguity_and_space_isolation() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let other = Client::create(f.store("alice"), &f.url, "company", "alice", None).await?;
    let store = f.store("alice");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            std::fs::metadata(store.profile_dir(&alice.profile).join("keys"))?
                .permissions()
                .mode()
                & 0o777
                == 0o600
        );
        ensure!(
            std::fs::metadata(store.profile_dir(&alice.profile))?
                .permissions()
                .mode()
                & 0o777
                == 0o700
        );
    }
    let file_keys = FileKeyStore::open(&store.profile_dir(&alice.profile).join("keys"))?;
    let prior = file_keys.get("identity")?;
    ensure!(file_keys.put("identity", b"conflicting key").is_err());
    ensure!(file_keys.get("identity")? == prior);
    ensure!(store.resolve(f.dir.path(), None).is_err());
    let repo = f.dir.path().join("repo");
    let nested = repo.join("deep");
    std::fs::create_dir_all(&nested)?;
    store.bind(&repo, "game")?;
    ensure!(store.resolve(&nested, None)?.space_id == alice.profile.space_id);
    ensure!(store.resolve(&nested, Some("company"))?.space_id == other.profile.space_id);
    let message = alice.send("#all", "Isolated", options("space A")).await?;
    let mut other = Client::open(store, other.profile, None).await?;
    ensure!(
        other
            .read(message.id, ReadOptions::default())
            .await
            .is_err()
    );
    Ok(())
}
#[tokio::test]
async fn names_slug_collisions_self_channel_and_secret_link_refusal() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let link = alice.invite(false, false).await?;
    let bob = Client::join(f.store("bob"), &link, "Alice").await?;
    ensure!(bob.profile.display_name == "alice-2");
    alice.refresh().await?;
    let root = alice
        .send(
            "@alice/second",
            "Self task",
            options("message to another session"),
        )
        .await?;
    ensure!(alice.counts().await?.unread_threads == 1);
    let mut other_session =
        Client::open(f.store("alice"), alice.profile.clone(), Some("second")).await?;
    ensure!(!other_session.fetch().await?.is_empty());
    other_session.read(root.id, ReadOptions::default()).await?;
    ensure!(alice.counts().await?.unread_threads == 0);
    ensure!(
        alice.read(root.id, ReadOptions::default()).await?.messages[0]
            .message
            .envelope
            .mentions[0]
            .handle
            .as_ref()
            .is_some_and(|h| h.as_str() == "second")
    );
    for body in ["phi://host/space#secret", "PHIS://host/space#secret"] {
        ensure!(alice.send("#all", "Unsafe", options(body)).await.is_err());
    }
    ensure!(
        alice
            .send(
                "#all",
                "Unsafe summary",
                SendOptions {
                    body: "fine".into(),
                    summary: Some("phi://secret".into()),
                    files: vec![]
                }
            )
            .await
            .is_err()
    );
    Ok(())
}
#[tokio::test]
async fn local_cache_search_and_retention_prune_on_startup() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let root = alice
        .send("#all", "Needle", options("searchable plaintext"))
        .await?;
    ensure!(alice.search("searchable").await?.len() == 1);
    ensure!(alice.search("absent").await?.is_empty());
    let cache = f.store("alice").cache_dir(&alice.profile).join("cache.db");
    let conn = rusqlite::Connection::open(cache)?;
    conn.execute("UPDATE messages SET created=1", [])?;
    conn.execute(
        "UPDATE state SET value='1' WHERE name LIKE 'activity/%'",
        [],
    )?;
    let mut reopened = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(reopened.search("searchable").await?.is_empty());
    reopened.read(root.id, ReadOptions::default()).await?;
    ensure!(reopened.search("searchable").await?.len() == 1);
    Ok(())
}
#[tokio::test]
async fn http_errors_admin_token_rate_limit_protocol_and_auth() -> Result<()> {
    let f = Fixture::configured(|c| {
        c.allow_space_creation = Creation::Token;
        c.admin_token = Some("admin-secret".into());
        c.limits.messages_per_min = 1;
    })
    .await?;
    ensure!(
        Client::create(f.store("bad"), &f.url, "game", "alice", None)
            .await
            .is_err()
    );
    let mut alice = Client::create(
        f.store("alice"),
        &f.url,
        "game",
        "alice",
        Some("admin-secret".into()),
    )
    .await?;
    alice.send("#all", "First", options("one")).await?;
    let error = alice
        .send("#all", "Second", options("two"))
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected rate limit"))?;
    ensure!(matches!(error, Error::RateLimited(_)) && error.exit_code() == 2);
    let response = reqwest::Client::new()
        .get(format!("{}/v1/spaces/{}", f.url, alice.profile.space_id))
        .header("Phi-Protocol", 2)
        .send()
        .await?;
    ensure!(response.status().as_u16() == 426);
    ensure!(Error::Upgrade(2).exit_code() == 4 && Error::Auth.exit_code() == 3);
    Ok(())
}
#[tokio::test]
async fn persisted_trust_rejects_omitted_certificate_and_roster_rollback() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, bob) = f.pair().await?;
    alice.group_create("backend", &["@bob".into()]).await?;
    alice.group_member("backend", "@bob", true).await?;
    let connection = rusqlite::Connection::open(f.dir.path().join("server/phi.db"))?;
    let channel = alice
        .channels
        .iter()
        .find(|c| c.meta.name == "backend")
        .ok_or_else(|| anyhow::anyhow!("missing group"))?
        .channel
        .clone();
    let mut rollback = channel.clone();
    rollback.membership = rollback
        .membership_history
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing history"))?
        .clone();
    rollback.membership_history.clear();
    rollback.current_epoch = 0;
    rollback.members = rollback.membership.data.members.clone();
    connection.execute(
        "UPDATE channels SET json=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&rollback)?, channel.id.to_string()],
    )?;
    ensure!(alice.refresh().await.is_err());
    connection.execute(
        "UPDATE channels SET json=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&channel)?, channel.id.to_string()],
    )?;
    connection.execute_batch("PRAGMA foreign_keys=OFF")?;
    connection.execute(
        "DELETE FROM devices WHERE id=?",
        [bob.profile.device_id.to_string()],
    )?;
    ensure!(alice.refresh().await.is_err());
    Ok(())
}
#[tokio::test]
async fn tampered_message_attachment_and_sequence_are_rejected() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let path: PathBuf = f.dir.path().join("file.txt");
    std::fs::write(&path, b"safe")?;
    let mut opts = options("signed body");
    opts.files.push(path);
    let root = alice.send("#all", "Signed", opts).await?;
    let connection = rusqlite::Connection::open(f.dir.path().join("server/phi.db"))?;
    let original = serde_json::to_string(&root)?;
    let mut tampered = root.clone();
    tampered.seq += 100;
    connection.execute(
        "UPDATE messages SET json=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&tampered)?, root.id.to_string()],
    )?;
    ensure!(alice.read(root.id, ReadOptions::default()).await.is_err());
    tampered = root.clone();
    tampered.envelope.ciphertext = Base64Bytes::new(vec![0; root.envelope.ciphertext.len()]);
    connection.execute(
        "UPDATE messages SET json=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&tampered)?, root.id.to_string()],
    )?;
    ensure!(alice.read(root.id, ReadOptions::default()).await.is_err());
    connection.execute(
        "UPDATE messages SET json=? WHERE id=?",
        rusqlite::params![original, root.id.to_string()],
    )?;
    let paths: Vec<_> = std::fs::read_dir(f.dir.path().join("server/blobs"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(!paths.is_empty());
    std::fs::write(paths[0].path(), b"tampered")?;
    ensure!(alice.read(root.id, ReadOptions::default()).await.is_err());
    f.server.sweep(u64::MAX / 2).await?;
    Ok(())
}

#[derive(Clone, Default)]
struct MemoryKeys(std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>);
impl KeyStore for MemoryKeys {
    fn get(&self, account: &str) -> phi_core::Result<Option<zeroize::Zeroizing<Vec<u8>>>> {
        let map = self
            .0
            .lock()
            .map_err(|_| Error::User("keychain mock poisoned".into()))?;
        Ok(map.get(account).cloned().map(zeroize::Zeroizing::new))
    }
    fn put(&self, account: &str, bytes: &[u8]) -> phi_core::Result<()> {
        self.0
            .lock()
            .map_err(|_| Error::User("keychain mock poisoned".into()))?
            .insert(account.into(), bytes.to_vec());
        Ok(())
    }
}
fn gui_device(keys: MemoryKeys) -> DeviceOptions {
    DeviceOptions {
        kind: DeviceKind::Gui,
        name: "desktop".into(),
        keystore: Some(Box::new(move |_| Ok(Box::new(keys)))),
    }
}
#[tokio::test]
async fn gui_keystore_boundary_and_authenticated_device_kind() -> Result<()> {
    let f = Fixture::new().await?;
    let keys = MemoryKeys::default();
    let mut gui = Client::create_for_device(
        f.store("gui"),
        &f.url,
        "gui-space",
        "alice",
        None,
        gui_device(keys.clone()),
    )
    .await?;
    ensure!(
        !f.store("gui")
            .profile_dir(&gui.profile)
            .join("keys")
            .exists()
    );
    let link = gui.invite_for_device(true, false, DeviceKind::Cli).await?;
    let mut cli = Client::join(f.store("cli"), &link, "alice").await?;
    let root = gui
        .send(
            "#all",
            "Human message",
            SendOptions {
                body: "from desktop".into(),
                ..Default::default()
            },
        )
        .await?;
    ensure!(
        cli.read(root.id, ReadOptions::default())
            .await?
            .text()
            .contains("human · sent from GUI")
    );
    let cli_gui_enroll = cli.invite_for_device(true, false, DeviceKind::Gui).await;
    ensure!(cli_gui_enroll.is_err());
    let gui_code = gui.invite_for_device(true, true, DeviceKind::Gui).await?;
    ensure!(
        Client::join(f.store("wrong-kind"), &gui_code, "alice")
            .await
            .is_err()
    );
    let gui2 = Client::join_for_device(
        f.store("gui2"),
        &gui_code,
        "alice",
        gui_device(MemoryKeys::default()),
    )
    .await?;
    ensure!(
        gui2.profile.kind == DeviceKind::Gui && gui2.profile.person_id == gui.profile.person_id
    );
    let reopened =
        Client::open_with_keystore(f.store("gui"), gui.profile.clone(), None, Box::new(keys))
            .await?;
    // The shared core can be moved into a GUI async task without a local executor.
    tokio::spawn(async move {
        let mut client = reopened;
        client.counts().await
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn remaining_members_rekey_private_channels_unknown_to_removal_issuer() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let invite = alice.invite(false, false).await?;
    let mut charlie = Client::join(f.store("charlie"), &invite, "charlie").await?;
    let private = bob.group_create("private", &["@charlie".into()]).await?;
    let old = bob
        .send("#private", "Before removal", options("old group secret"))
        .await?;
    charlie.read(old.id, ReadOptions::default()).await?;
    alice.refresh().await?;
    ensure!(!alice.channels.iter().any(|c| c.channel.id == private));
    alice
        .revoke(RevocationTarget::Person {
            person_id: charlie.profile.person_id,
        })
        .await?;
    let new = bob
        .send("#private", "After removal", options("new group secret"))
        .await?;
    ensure!(new.envelope.epoch == 1);
    ensure!(
        bob.channels
            .iter()
            .find(|c| c.channel.id == private)
            .is_some_and(|c| c.channel.members == vec![bob.profile.person_id])
    );
    let next = bob.reply(new.id, options("still safe")).await?;
    ensure!(next.envelope.epoch == 1);
    ensure!(matches!(charlie.counts().await, Err(Error::Auth)));
    let conn = rusqlite::Connection::open(f.dir.path().join("server/phi.db"))?;
    let leaked: u64 = conn.query_row(
        "SELECT COUNT(*) FROM wrapped_keys WHERE channel_id=? AND epoch=1 AND device_id=?",
        rusqlite::params![private.to_string(), charlie.profile.device_id.to_string()],
        |r| r.get(0),
    )?;
    ensure!(leaked == 0);
    Ok(())
}
