//! A space made by the released 0.2.0 binaries must keep working with this build.
//!
//! `fixtures/space-0.2.0` holds a server database and three client profiles written by the
//! real 0.2.0 server image and CLI (see `fixtures/README.md`). This build opens them as they
//! are: no migration step, no re-invite. If a change to a signing domain, a wire struct or a
//! database schema breaks this test, that change needs a versioned reader, not a new space.
use anyhow::{Context, Result, ensure};
use phi_core::*;
use phi_proto::*;
use phi_server::{
    Server,
    config::{Config, Creation},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

const FIXTURE: &str = "space-0.2.0";

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(FIXTURE)
}
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn options(body: &str) -> SendOptions {
    SendOptions {
        body: body.to_owned(),
        ..Default::default()
    }
}
#[derive(serde::Deserialize)]
struct Manifest {
    t_all: MessageId,
    t_backend: MessageId,
    t_dm: MessageId,
    t_laptop: MessageId,
    laptop_device: DeviceId,
}
struct Frozen {
    dir: TempDir,
    url: String,
    task: tokio::task::JoinHandle<()>,
    manifest: Manifest,
}
impl Drop for Frozen {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Frozen {
    /// Serves a copy of the frozen server database with the current server code.
    async fn start() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        copy_dir(&fixture_root().join("server"), &dir.path().join("server"))?;
        let config = Config {
            data_dir: dir.path().join("server"),
            allow_space_creation: Creation::Anyone,
            ..Config::default()
        };
        let server =
            Server::open(config).context("current server cannot open the 0.2.0 database")?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let router = server.router();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let manifest = serde_json::from_slice(&fs::read(fixture_root().join("manifest.json"))?)?;
        Ok(Self {
            dir,
            url,
            task,
            manifest,
        })
    }
    fn store(&self, who: &str) -> Store {
        Store::new(
            self.dir.path().join(who).join("config"),
            self.dir.path().join(who).join("cache"),
        )
    }
    /// Opens a frozen profile with the current client code. Only the server address changes,
    /// because the fixture was recorded against a container name.
    async fn open(&self, who: &str, handle: &str) -> Result<Client> {
        let source = fixture_root().join(who);
        let store = self.store(who);
        copy_dir(&source.join("config"), &store.root)?;
        copy_dir(&source.join("cache"), &store.cache_root)?;
        let mut profile = store.profiles()?.pop().context("fixture profile missing")?;
        let old_profile = store.profile_dir(&profile);
        let old_cache = store.cache_dir(&profile);
        profile.server = self.url.clone();
        fs::rename(old_profile, store.profile_dir(&profile))?;
        fs::rename(old_cache, store.cache_dir(&profile))?;
        store.save_profile(&profile)?;
        Ok(Client::open(store, profile, Some(handle)).await?)
    }
}
fn all_verified(view: &ThreadView) -> bool {
    view.messages.iter().all(|m| m.verification_error.is_none())
}

#[tokio::test]
async fn space_from_0_2_0_opens_reads_and_keeps_working() -> Result<()> {
    let f = Frozen::start().await?;
    let mut alice = f.open("alice", "main").await?;
    let mut bob = f.open("bob", "main").await?;

    // History recorded by 0.2.0 reads back verified: #all with a summary, a group, a DM.
    let all = ReadOptions {
        all: true,
        last: None,
    };
    let view = alice.read(f.manifest.t_all, all).await?;
    ensure!(
        view.messages.len() == 3 && all_verified(&view),
        "{}",
        view.text()
    );
    ensure!(
        view.messages[0]
            .content
            .body
            .contains("First message in #all")
    );
    ensure!(view.messages[1].content.body.contains("Reply from bob"));
    ensure!(view.summary.as_deref() == Some("Alice welcomed the team, bob acked and replied."));
    ensure!(
        view.reactions
            .iter()
            .any(|r| r.data.kind == ReactionKind::Ack)
    );
    let view = bob.read(f.manifest.t_backend, all).await?;
    ensure!(
        view.messages.len() == 2 && all_verified(&view),
        "{}",
        view.text()
    );
    let view = alice.read(f.manifest.t_dm, all).await?;
    ensure!(
        view.messages.len() == 1 && all_verified(&view),
        "{}",
        view.text()
    );
    ensure!(view.messages[0].content.body.contains("DM from bob"));

    // A message signed by the device that was later revoked is in that revocation's history,
    // so it still verifies; the device itself stays revoked.
    let view = bob.read(f.manifest.t_laptop, all).await?;
    ensure!(
        view.messages.len() == 1 && all_verified(&view),
        "{}",
        view.text()
    );
    let who = alice.who_view().await?;
    let laptop = who
        .people
        .iter()
        .flat_map(|p| &p.devices)
        .find(|d| d.id == f.manifest.laptop_device)
        .context("revoked laptop missing from the directory")?;
    ensure!(laptop.revoked);
    ensure!(who.people.len() == 2);

    // The signed settings change (summary_every=5) survived.
    ensure!(alice.settings.summary_every == 5);
    ensure!(bob.settings.summary_every == 5);

    // New traffic in both directions on the old channels.
    let fresh = bob
        .send("#all", "After upgrade", options("sent by this build"))
        .await?;
    let view = alice.read(fresh.id, ReadOptions::default()).await?;
    ensure!(view.messages[0].content.body == "sent by this build" && all_verified(&view));
    alice
        .reply(f.manifest.t_all, options("fourth, from this build"))
        .await?;
    let view = bob.read(f.manifest.t_all, all).await?;
    ensure!(
        view.messages.len() == 4 && all_verified(&view),
        "{}",
        view.text()
    );
    bob.reply(f.manifest.t_backend, options("group still works"))
        .await?;
    let view = alice.read(f.manifest.t_backend, all).await?;
    ensure!(
        view.messages.len() == 3 && all_verified(&view),
        "{}",
        view.text()
    );
    bob.react(fresh.id, ReactionKind::Ack, false).await?;

    // Enrollment and invites still work, and a brand-new device verifies the whole history
    // including the revoked device's message.
    let link = bob.invite(true, false).await?;
    let mut bob_laptop = Client::join(f.store("bob-laptop"), &link, "bob").await?;
    let view = bob_laptop.read(f.manifest.t_all, all).await?;
    ensure!(
        view.messages.len() == 4 && all_verified(&view),
        "{}",
        view.text()
    );
    let view = bob_laptop.read(f.manifest.t_laptop, all).await?;
    ensure!(
        view.messages.len() == 1 && all_verified(&view),
        "{}",
        view.text()
    );
    let link = alice.invite(false, false).await?;
    let mut carol = Client::join(f.store("carol"), &link, "carol").await?;
    let view = carol.read(f.manifest.t_all, all).await?;
    ensure!(
        view.messages.len() == 4 && all_verified(&view),
        "{}",
        view.text()
    );
    carol
        .send("#all", "Hello from carol", options("new member"))
        .await?;
    alice.refresh().await?;
    ensure!(alice.who_view().await?.people.len() == 3);
    Ok(())
}

#[tokio::test]
async fn revoked_device_from_0_2_0_stays_locked_out() -> Result<()> {
    let f = Frozen::start().await?;
    let Ok(mut laptop) = f.open("alice-laptop", "laptop").await else {
        return Ok(());
    };
    ensure!(
        laptop
            .send("#all", "Still here?", options("must fail"))
            .await
            .is_err()
    );
    Ok(())
}
