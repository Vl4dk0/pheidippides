use anyhow::{Result, ensure};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use phi_core::*;
use phi_proto::*;
use phi_server::{
    Server,
    config::{Config, Creation},
};
use rusqlite::OptionalExtension;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Default)]
struct Faults {
    omit_device: Option<DeviceId>,
    hide_revocations: bool,
    inflate_cursor: bool,
    corrupt_event: Option<MessageId>,
    unsigned_retention: Option<u32>,
    backdate_messages: bool,
    fail_metadata: Option<ChannelId>,
    fail_revocations: bool,
    fail_message: Option<MessageId>,
    lose_revocation_response: bool,
    directory_reads: usize,
    drop_event_polls: usize,
    /// Status to return for event polls, instead of serving them. Auth and validation stay fatal.
    event_reject: Option<u16>,
}
#[derive(Clone)]
struct Host {
    faults: Arc<Mutex<Faults>>,
    /// Wakes an in-flight long-poll so a restart looks like a dropped connection.
    cut: Arc<tokio::sync::Notify>,
    polls: Arc<AtomicUsize>,
}
fn event_reject(status: u16) -> Response {
    let (code, body) = match status {
        401 => (
            StatusCode::UNAUTHORIZED,
            serde_json::json!({"error":"unauthorized","message":"no"}),
        ),
        403 => (
            StatusCode::FORBIDDEN,
            serde_json::json!({"error":"forbidden","message":"no"}),
        ),
        400 => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error":"invalid","message":"bad"}),
        ),
        426 => (
            StatusCode::UPGRADE_REQUIRED,
            serde_json::json!({"error":"upgrade","required_major":9}),
        ),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error":"internal","message":"no"}),
        ),
    };
    (code, axum::Json(body)).into_response()
}
fn connection_reset() -> Response {
    let body = Body::from_stream(futures_util::stream::once(async {
        Err::<axum::body::Bytes, std::io::Error>(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "server restarted",
        ))
    }));
    Response::builder()
        .status(StatusCode::OK)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
async fn hostile_host(State(host): State<Host>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_owned();
    let Ok((faults, drop_poll, reject)) = host.faults.lock().map(|mut f| {
        if path.ends_with("/devices") {
            f.directory_reads += 1;
        }
        let events = path.ends_with("/events");
        let drop_poll = events && f.drop_event_polls > 0;
        if drop_poll {
            f.drop_event_polls -= 1;
        }
        let reject = events.then_some(f.event_reject).flatten();
        (f.clone(), drop_poll, reject)
    }) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if let Some(status) = reject {
        return event_reject(status);
    }
    if drop_poll {
        // What nginx returns when its proxy_read_timeout cuts an idle long-poll.
        return (StatusCode::BAD_GATEWAY, "<html>502 Bad Gateway</html>").into_response();
    }
    if (faults
        .fail_metadata
        .is_some_and(|id| path == format!("/v1/channels/{id}"))
        && request.method() == Method::PATCH)
        || (faults.fail_revocations
            && path.ends_with("/revocations")
            && request.method() == Method::POST)
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"interrupted publication"})),
        )
            .into_response();
    }
    if faults
        .fail_message
        .is_some_and(|id| path == format!("/v1/messages/{id}"))
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"temporary failure"})),
        )
            .into_response();
    }
    let response = if path.ends_with("/events") {
        host.polls.fetch_add(1, Ordering::SeqCst);
        let response = tokio::select! {
            biased;
            _ = host.cut.notified() => {
                host.polls.fetch_sub(1, Ordering::SeqCst);
                return connection_reset();
            }
            response = next.run(request) => response,
        };
        host.polls.fetch_sub(1, Ordering::SeqCst);
        response
    } else {
        next.run(request).await
    };
    if faults.lose_revocation_response
        && path.ends_with("/revocations")
        && response.status().is_success()
    {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"lost response after commit"})),
        )
            .into_response();
    }
    if !response.status().is_success()
        || !response
            .headers()
            .get("content-type")
            .is_some_and(|v| v.as_bytes().starts_with(b"application/json"))
    {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = to_bytes(body, 32 * 1024 * 1024).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Response::from_parts(parts, Body::from(bytes));
    };
    if path.ends_with("/devices") {
        if let Some(id) = faults.omit_device {
            if let Some(devices) = value.get_mut("devices").and_then(|v| v.as_array_mut()) {
                devices.retain(|device| device["cert"]["data"]["device_id"] != id.to_string());
            }
        }
        if faults.hide_revocations {
            value["directory"]["revocations"] = serde_json::json!([]);
        }
    }
    if value.get("settings").is_some() {
        if let Some(retention) = faults.unsigned_retention {
            value["settings"]["retention_days"] = retention.into();
            value["settings_change"] = serde_json::Value::Null;
        }
    }
    if path.ends_with("/events") {
        if faults.inflate_cursor {
            value["next_after"] = 1_000_000.into();
        }
        if let Some(id) = faults.corrupt_event {
            if let Some(events) = value.get_mut("events").and_then(|v| v.as_array_mut()) {
                for event in events {
                    if event["event"]["data"]["id"] == id.to_string() {
                        event["event"]["data"]["signature"] =
                            serde_json::to_value(Signature::from_bytes([0; 64]))
                                .unwrap_or_default();
                    }
                }
            }
        }
    }
    if faults.backdate_messages {
        backdate(&mut value);
    }
    parts.headers.remove("content-length");
    match serde_json::to_vec(&value) {
        Ok(body) => Response::from_parts(parts, Body::from(body)),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
fn backdate(value: &mut serde_json::Value) {
    if value.get("envelope").is_some() && value.get("author_device").is_some() {
        value["created_at"] = 1.into();
    }
    match value {
        serde_json::Value::Object(map) => {
            for value in map.values_mut() {
                backdate(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                backdate(value);
            }
        }
        _ => {}
    }
}
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    url: String,
    server: Server,
    task: tokio::task::JoinHandle<()>,
    host: Host,
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
        let host = Host {
            faults: Arc::new(Mutex::new(Faults::default())),
            cut: Arc::new(tokio::sync::Notify::new()),
            polls: Arc::new(AtomicUsize::new(0)),
        };
        let task = spawn_server(listener, server.clone(), host.clone());
        Ok(Self {
            dir,
            url,
            server,
            task,
            host,
        })
    }
    fn faults(&self, update: impl FnOnce(&mut Faults)) -> Result<()> {
        let mut faults = self
            .host
            .faults
            .lock()
            .map_err(|_| anyhow::anyhow!("fault mutex poisoned"))?;
        update(&mut faults);
        Ok(())
    }
    /// Drop the listening socket and the in-flight long-poll, then bind the same address again.
    async fn restart(&mut self) -> Result<()> {
        self.host.cut.notify_waiters();
        tokio::time::sleep(Duration::from_millis(30)).await;
        self.task.abort();
        let _ = (&mut self.task).await;
        let addr = self
            .url
            .strip_prefix("http://")
            .ok_or_else(|| anyhow::anyhow!("fixture url is not http"))?;
        let mut listener = None;
        for _ in 0..50 {
            if let Ok(bound) = tokio::net::TcpListener::bind(addr).await {
                listener = Some(bound);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let listener = listener.ok_or_else(|| anyhow::anyhow!("restart did not rebind {addr}"))?;
        self.task = spawn_server(listener, self.server.clone(), self.host.clone());
        Ok(())
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
fn spawn_server(
    listener: tokio::net::TcpListener,
    server: Server,
    host: Host,
) -> tokio::task::JoinHandle<()> {
    let router = server
        .router()
        .layer(axum::middleware::from_fn_with_state(host, hostile_host));
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    })
}
fn options(body: &str) -> SendOptions {
    SendOptions {
        body: body.into(),
        summary: Some(format!("Summary: {body}")),
        files: vec![],
    }
}
fn stored_device_keys(store: &Store, profile: &Profile) -> Result<phi_crypto::DeviceKeys> {
    let keys = FileKeyStore::open(&store.profile_dir(profile).join("keys"))?;
    let raw = keys
        .get("identity")?
        .ok_or_else(|| anyhow::anyhow!("missing identity"))?;
    let value: serde_json::Value = serde_json::from_slice(&raw)?;
    let sign: KeyBytes = serde_json::from_value(
        value
            .get("sign")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing sign key"))?,
    )?;
    let kex: KeyBytes = serde_json::from_value(
        value
            .get("kex")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing kex key"))?,
    )?;
    Ok(phi_crypto::DeviceKeys::from_secrets(
        *sign.as_bytes(),
        *kex.as_bytes(),
    ))
}

#[tokio::test]
async fn create_invite_join_attachment_fetch_read_reply() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let path = f.dir.path().join("shot.png");
    std::fs::write(&path, b"image bytes")?;
    let mut message = options("Please review repo foo at abc123.");
    message.files.push(path);
    for (name, bytes) in [
        ("renderer.PY", b"print('hello')\n".as_slice()),
        ("settings.toml", b"enabled = true\n".as_slice()),
        ("notes.custom", b"plain text\n".as_slice()),
        ("opaque.bin", b"\0\xff".as_slice()),
    ] {
        let path = f.dir.path().join(name);
        std::fs::write(&path, bytes)?;
        message.files.push(path);
    }
    let root = alice.send("@bob/cli", "Review PR", message).await?;
    ensure!(bob.fetch().await?.contains("1 unread threads"));
    ensure!(bob.fetch().await?.contains("1 mention"));
    let inbox = bob.inbox().await?;
    ensure!(inbox.len() == 1 && inbox[0].mentioned && inbox[0].title == "Review PR");
    let waited = bob.wait_text(0).await?;
    ensure!(
        waited.starts_with(&format!("{UNTRUSTED_BANNER}\n"))
            && !waited.contains("UNTRUSTED")
            && waited.contains("to: @bob/cli (you)")
    );
    let downloaded = f.store("bob").cache_dir(&bob.profile).join("blobs");
    let paths = std::fs::read_dir(&downloaded)?.collect::<std::io::Result<Vec<_>>>()?;
    let image = paths
        .into_iter()
        .map(|entry| entry.path().join("shot.png"))
        .find(|path| path.exists());
    ensure!(
        image
            .as_ref()
            .is_some_and(|path| waited.contains(&path.display().to_string()))
    );
    ensure!(
        std::fs::read(image.ok_or_else(|| anyhow::anyhow!("missing downloaded image"))?)?
            == b"image bytes"
    );
    ensure!(bob.fetch().await?.is_empty());
    let view = bob.read(root.id, ReadOptions::default()).await?;
    ensure!(
        view.text().starts_with(&format!("{UNTRUSTED_BANNER}\n"))
            && !view.text().contains("UNTRUSTED")
            && view.text().contains("alice/cli (agent)")
    );
    ensure!(std::fs::read(&view.messages[0].attachments[0])? == b"image bytes");
    let mimes: Vec<_> = view.messages[0]
        .content
        .attachments
        .iter()
        .map(|a| a.mime.as_str())
        .collect();
    ensure!(
        mimes
            == [
                "image/png",
                "text/x-python",
                "text/plain",
                "text/plain",
                "application/octet-stream"
            ]
    );
    ensure!(view.messages[0].reference().to_string() == format!("{}#1", root.id));
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
    ensure!(view.text().contains("summary (as of #2; up to date)"));
    let own_tail = bob
        .read(
            root.id,
            ReadOptions {
                all: false,
                last: Some(1),
            },
        )
        .await?;
    ensure!(
        own_tail.messages.len() == 1 && own_tail.messages[0].perspective == MessagePerspective::You
    );
    ensure!(
        own_tail.text().matches(UNTRUSTED_BANNER).count() == 1,
        "the foreign root title stays framed even when only an own reply is shown"
    );
    let own = format_message(&view.messages[0], &alice.names());
    ensure!(
        own.contains("(you · agent)")
            && !own.contains(UNTRUSTED_BANNER)
            && !own.contains("UNTRUSTED")
    );
    let foreign = format_message(&view.messages[1], &alice.names());
    ensure!(
        foreign.starts_with(&format!("{UNTRUSTED_BANNER}\n"))
            && !foreign.contains("UNTRUSTED")
            && foreign.contains(&reply.id.to_string())
    );
    let reference: MessageRef = format!("{}#2", root.id).parse()?;
    ensure!(alice.resolve_message(reference).await? == reply.id);
    ensure!(alice.resolve_thread(reference).await? == root.id);
    ensure!(alice.resolve_thread(MessageRef::Id(reply.id)).await? == root.id);
    let target = alice.resolve_message(reference).await?;
    alice.react(target, ReactionKind::Ack, false).await?;
    ensure!(
        alice
            .resolve_message(format!("{}#999", root.id).parse()?)
            .await
            .is_err()
    );
    let mut reads_before = 0;
    f.faults(|faults| reads_before = faults.directory_reads)?;
    let mut fresh = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    let resolved = fresh
        .resolve_thread(format!("{}#1", root.id).parse()?)
        .await?;
    fresh.read(resolved, ReadOptions::default()).await?;
    f.faults(|faults| {
        assert_eq!(
            faults.directory_reads,
            reads_before + 1,
            "opening/resolution/read share one verified snapshot"
        )
    })?;
    for suffix in ["0", "-1", "abc", "1#2"] {
        ensure!(
            format!("{}#{suffix}", root.id)
                .parse::<MessageRef>()
                .is_err()
        );
    }
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
    ensure!(events.is_empty());
    let view = carol.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("claimed by bob/cli"));
    let conflict = alice
        .react(root.id, ReactionKind::Claim, false)
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("claim conflict must fail"))?;
    ensure!(conflict.to_string() == "already claimed by bob/cli just now");
    ensure!(alice.wait_text(0).await? == format!("bob/cli claim {}#1 (Task)\n", root.id));
    alice.react(root.id, ReactionKind::Ack, false).await?;
    let view = alice.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("claimed by bob/cli") && view.text().contains("ack by alice/cli"));
    let mut other_handle =
        Client::open(f.store("alice"), alice.profile.clone(), Some("reviewer")).await?;
    other_handle
        .react(root.id, ReactionKind::Ack, false)
        .await?;
    let reply = alice.reply(root.id, options("work ready")).await?;
    carol.react(reply.id, ReactionKind::Down, false).await?;
    let view = carol
        .read(
            root.id,
            ReadOptions {
                all: true,
                last: None,
            },
        )
        .await?;
    let rendered = view.text();
    ensure!(rendered.matches(UNTRUSTED_BANNER).count() == 1 && !rendered.contains("UNTRUSTED"));
    let (first, second) = rendered
        .split_once("── #2")
        .ok_or_else(|| anyhow::anyhow!("missing reply"))?;
    ensure!(
        first.contains("ack by alice/cli, alice/reviewer") && first.contains("claimed by bob/cli")
    );
    ensure!(
        !second.contains("ack by")
            && !second.contains("claimed by")
            && second.contains("down by carol/cli")
    );
    bob.unread(root.id).await?;
    let inbox = bob.inbox().await?;
    ensure!(
        format_inbox(&inbox, &bob.names())
            .matches(UNTRUSTED_BANNER)
            .count()
            == 1
    );
    let preview = inbox
        .iter()
        .find(|entry| entry.id == root.id)
        .ok_or_else(|| anyhow::anyhow!("missing inbox entry"))?
        .text(&bob.names());
    ensure!(
        preview.contains("latest message reactions: down by carol/cli · 4 reactions in thread")
            && preview.contains("claims: 1 · bob/cli #1")
            && !preview.contains("ack by")
            && !preview.contains("claimed by")
    );
    carol.wait(0).await?;
    bob.react(root.id, ReactionKind::Claim, true).await?;
    ensure!(alice.wait_text(0).await? == format!("bob/cli unclaim {}#1 (Task)\n", root.id));
    ensure!(carol.wait(0).await?.is_empty());
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
    carol
        .reply(root.id, options("now participating by authoring a reply"))
        .await?;
    carol.wait(0).await?;
    alice.react(reply.id, ReactionKind::Up, false).await?;
    alice.react(reply.id, ReactionKind::Down, false).await?;
    ensure!(carol.wait_text(0).await?.is_empty());
    bob.react(reply.id, ReactionKind::Ack, false).await?;
    ensure!(
        carol.wait_text(0).await?.is_empty(),
        "authoring another message in a thread does not qualify"
    );
    ensure!(
        alice
            .wait_text(0)
            .await?
            .contains(&format!("bob/cli ack {}#2 (Task)", root.id))
    );
    let carol_reply = carol
        .reply(root.id, options("carol owns this message"))
        .await?;
    bob.react(carol_reply.id, ReactionKind::Claim, false)
        .await?;
    ensure!(
        format_threads(&alice.discover_threads(Some("#all")).await?, &alice.names())
            .contains("claims: 2 · alice/cli #1, bob/cli #4")
    );
    ensure!(carol.wait_text(0).await? == format!("bob/cli claim {}#4 (Task)\n", root.id));
    bob.react(carol_reply.id, ReactionKind::Claim, true).await?;
    let notice = carol.wait_json(0).await?;
    ensure!(notice["events"][0]["type"] == "reaction" && notice["events"][0]["kind"] == "unclaim");
    ensure!(
        notice["events"][0]["trusted"] == false
            && notice["events"][0]["message_id"] == carol_reply.id.to_string()
    );
    let threads = alice.discover_threads(Some("#all")).await?;
    ensure!(threads[0].id == root.id && threads[0].message_count == 4);
    ensure!(threads[0].participants.iter().any(|p| p == "carol/cli"));
    ensure!(
        format_threads(&threads, &alice.names())
            .matches(UNTRUSTED_BANNER)
            .count()
            == 1
    );
    ensure!(alice.discover_threads(Some("@bob")).await?.is_empty());
    let channels = carol.channel_views().await?;
    ensure!(
        channels
            .iter()
            .any(|c| c.name == "#all" && c.members.len() == 3)
    );
    ensure!(!alice.who().await?.contains("#all:"));
    let later = alice
        .send("#all", "Later thread", options("later activity"))
        .await?;
    ensure!(alice.discover_threads(None).await?[0].id == later.id);
    bob.react(root.id, ReactionKind::Down, false).await?;
    let reordered = alice.discover_threads(None).await?;
    ensure!(
        reordered[0].id == root.id
            && reordered[0].last_activity_seq > reordered[1].last_activity_seq,
        "reaction activity reorders discovery even within one second"
    );
    ensure!(reordered[0].unread_count == 1 && reordered[0].unread);
    bob.close_thread(root.id).await?;
    ensure!(
        alice.inbox().await?.iter().all(|entry| entry.id != root.id),
        "closed thread hidden from inbox"
    );
    ensure!(
        alice
            .discover_threads(None)
            .await?
            .iter()
            .all(|entry| entry.id != root.id),
        "closed thread hidden from default threads"
    );
    let closed_list = alice.discover_threads_filtered(None, true).await?;
    ensure!(
        closed_list.len() == 1
            && closed_list[0].id == root.id
            && closed_list[0].closed
            && closed_list[0].closed_by.as_deref() == Some("bob")
    );
    let closed_view = alice.read(root.id, ReadOptions::default()).await?;
    ensure!(closed_view.closed && closed_view.can_delete);
    carol
        .reply(root.id, options("reply reopens closed thread"))
        .await?;
    ensure!(
        alice
            .discover_threads_filtered(None, true)
            .await?
            .is_empty()
    );
    ensure!(
        alice
            .inbox()
            .await?
            .iter()
            .any(|entry| entry.id == root.id && !entry.closed),
        "replying to a closed thread reopens it automatically"
    );
    bob.close_thread(root.id).await?;
    ensure!(alice.discover_threads_filtered(None, true).await?.len() == 1);
    bob.reopen_thread(root.id).await?;
    ensure!(
        alice
            .discover_threads_filtered(None, true)
            .await?
            .is_empty()
    );
    bob.read(later.id, ReadOptions::default()).await?;
    ensure!(bob.search("later activity").await?.len() == 1);
    ensure!(bob.delete_thread(later.id).await.is_err());
    alice.delete_thread(later.id).await?;
    ensure!(alice.search("later activity").await?.is_empty());
    ensure!(
        bob.discover_threads(None)
            .await?
            .iter()
            .all(|entry| entry.id != later.id)
    );
    ensure!(
        bob.search("later activity").await?.is_empty(),
        "deletion event drops deleted thread from peer local cache on next sync"
    );

    Ok(())
}
#[tokio::test]
async fn wait_backlog_wakes_on_reply_and_keeps_per_handle_cursor() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let root = alice.send("@bob", "Task", options("hello")).await?;
    ensure!(alice.wait(0).await?.is_empty());
    f.faults(|faults| faults.inflate_cursor = true)?;
    let events = bob.wait(0).await?;
    ensure!(events.iter().any(|e| matches!(e, WaitEntry::Message(_))));
    let output = bob.format_wait_entry(&events[0]).await?;
    ensure!(
        output.contains(&format!("thread {} · Task", root.id))
            && output.contains(&format!("{}#1", root.id))
            && output.contains("hello")
            && output.matches(UNTRUSTED_BANNER).count() == 1
            && !output.contains("UNTRUSTED")
            && !output.contains("summary (as of")
            && !output.contains("phi read")
    );
    ensure!(bob.wait(0).await?.is_empty());
    let unread_before = bob.counts().await?.unread_threads;
    let ack_sender = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        alice.react(root.id, ReactionKind::Ack, false).await
    };
    let (notice, sent) = tokio::join!(bob.wait_text(1), ack_sender);
    sent?;
    ensure!(
        notice?.is_empty(),
        "being addressed in a thread does not qualify for reactions to others' messages"
    );
    ensure!(
        bob.counts().await?.unread_threads == unread_before,
        "reaction-only wait does not mark the thread read"
    );
    f.faults(|faults| faults.inflate_cursor = false)?;
    bob.react(root.id, ReactionKind::Claim, false).await?;
    let own = bob.reply(root.id, options("my reply")).await?;
    let mut alice_reviewer =
        Client::open(f.store("alice"), alice.profile.clone(), Some("reviewer")).await?;
    ensure!(
        alice_reviewer
            .wait(0)
            .await?
            .iter()
            .any(|event| matches!(event,
        WaitEntry::Message(entry) if entry.latest_seq == own.seq))
    );
    ensure!(alice_reviewer.fetch().await?.contains("1 unread threads"));
    ensure!(alice.counts().await?.unread_threads == 1);
    let events = alice.wait(0).await?;
    let mut output = String::new();
    for event in &events {
        output.push_str(&alice.format_wait_entry(event).await?);
    }
    ensure!(
        output.contains(&format!("reaction context · {}#1:", root.id))
            && output.contains("claimed by bob/cli")
            && output.contains("summary (as of #2; up to date)")
            && output.contains("my reply")
            && output.contains(&format!("{}#2", root.id))
            && output.contains(&own.id.to_string())
    );
    let mut parallel = Client::open(f.store("bob"), bob.profile.clone(), None).await?;
    let second_waiter = parallel.wait(3);
    let waiter = bob.wait(3);
    let sender = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        alice.reply(root.id, options("new reply")).await
    };
    let (events, second_events, message) = tokio::join!(waiter, second_waiter, sender);
    let message = message?;
    for entries in [events?, second_events?] {
        ensure!(entries.len() == 1);
        let output = bob.format_wait_entry(&entries[0]).await?;
        ensure!(
            output.contains(&format!("{}#3", root.id))
                && output.contains(&message.id.to_string())
                && !output.contains(&own.id.to_string())
        );
    }
    ensure!(bob.wait(0).await?.is_empty());
    let mut second = Client::open(f.store("bob"), bob.profile.clone(), Some("second")).await?;
    let entries = second.wait(0).await?;
    ensure!(entries.iter().any(|event| matches!(event,
        WaitEntry::Message(entry) if entry.latest_seq == message.seq)));
    ensure!(entries.iter().all(|event| !matches!(event,
        WaitEntry::Message(entry) if entry.latest_seq == own.seq)));
    ensure!(second.counts().await?.unread_threads == 1);
    let mut reopened = Client::open(f.store("bob"), bob.profile.clone(), None).await?;
    ensure!(reopened.wait(0).await?.is_empty());
    // An own-only backlog consumes the original timeout instead of waking the session.
    bob.react(message.id, ReactionKind::Ack, false).await?;
    let started = std::time::Instant::now();
    ensure!(bob.wait(1).await?.is_empty());
    ensure!(started.elapsed() >= Duration::from_millis(900));
    let ack = alice.wait_text(0).await?;
    ensure!(ack == format!("bob/cli ack {}#3 (Task)\n", root.id));
    let next_message = bob
        .reply(root.id, options("next message carries reaction context"))
        .await?;
    bob.react(next_message.id, ReactionKind::Up, false).await?;
    let next = alice.wait_text(0).await?;
    ensure!(
        next.contains("up by bob/cli")
            && next.contains(&format!("reaction context · {}#3: ack by bob/cli", root.id))
            && next.contains("to: @bob")
    );
    ensure!(alice.wait_text(0).await?.is_empty());
    let mut reviewer = Client::open(f.store("bob"), bob.profile.clone(), Some("reviewer")).await?;
    reviewer.wait(0).await?;
    let other = alice
        .send("@bob/other", "Other handle", options("not for reviewer"))
        .await?;
    let targeted = bob
        .send(
            "@bob/reviewer",
            "For reviewer",
            options("own other session"),
        )
        .await?;
    let general = alice
        .send("#all", "General", options("for everyone"))
        .await?;
    let entries = reviewer.wait(0).await?;
    ensure!(
        matches!(&entries[0], WaitEntry::Message(entry) if entry.id == targeted.id && entry.to == "@bob/reviewer (you)")
    );
    ensure!(
        entries
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(entry) if entry.id == other.id))
    );
    ensure!(
        entries
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(entry) if entry.id == general.id))
    );
    ensure!(reviewer.counts().await?.unread_threads == 3);
    // Inbox remains person-wide; notification routing does not change read state.
    let inbox = reviewer.inbox().await?;
    ensure!(inbox[0].id == targeted.id && inbox[0].to == "@bob/reviewer (you)");
    bob.react(targeted.id, ReactionKind::Ack, false).await?;
    let start = std::time::Instant::now();
    ensure!(reviewer.wait(1).await?.is_empty());
    ensure!(start.elapsed() >= Duration::from_millis(900));
    let started = std::time::Instant::now();
    let unrelated = async {
        tokio::time::sleep(Duration::from_millis(40)).await;
        bob.reply(targeted.id, options("own untargeted reply"))
            .await
    };
    let (waiting, sent) = tokio::join!(reviewer.wait(1), unrelated);
    sent?;
    ensure!(waiting?.is_empty());
    ensure!(started.elapsed() >= Duration::from_millis(900));
    let external = alice
        .reply(root.id, options("reply for every bob handle"))
        .await?;
    ensure!(reviewer.wait(0).await?.iter().any(|event| matches!(event,
        WaitEntry::Message(entry) if entry.latest_seq == external.seq)));
    let direct = alice
        .send("@bob/reviewer", "Called reviewer", options("please review"))
        .await?;
    ensure!(reviewer.wait(0).await?.len() == 1);
    bob.react(direct.id, ReactionKind::Ack, false).await?;
    ensure!(reviewer.wait(0).await?.is_empty());
    alice.reply(direct.id, options("carries context")).await?;
    let entries = reviewer.wait(0).await?;
    ensure!(
        matches!(&entries[0], WaitEntry::Message(entry) if entry.reactions.iter().any(|r| r.data.kind == ReactionKind::Ack))
    );
    let batch = alice
        .send("@bob/reviewer", "Whole batch", options("batch root body"))
        .await?;
    let sibling = alice
        .send("#all", "Parallel batch", options("parallel root body"))
        .await?;
    alice.reply(batch.id, options("batch first reply")).await?;
    let last = alice.reply(batch.id, options("batch final reply")).await?;
    let sibling_last = alice
        .reply(sibling.id, options("parallel reply body"))
        .await?;
    alice
        .send("@bob/other", "Unrelated", options("not displayed"))
        .await?;
    let output = reviewer.wait_text(0).await?;
    ensure!(
        output
            .matches(&format!("thread {} · Whole batch", batch.id))
            .count()
            == 1
    );
    ensure!(
        ["batch root body", "batch first reply", "batch final reply"]
            .iter()
            .all(|body| output.contains(body))
    );
    ensure!(
        !output.contains("summary (as of #3;")
            && output.matches(UNTRUSTED_BANNER).count() == 1
            && !output.contains("UNTRUSTED")
    );
    let sibling_header = format!("thread {} · Parallel batch", sibling.id);
    ensure!(output.matches(&sibling_header).count() == 1);
    ensure!(output.find("batch final reply") < output.find(&sibling_header));
    ensure!(output.contains("parallel root body") && output.contains("parallel reply body"));
    ensure!(output.contains("not displayed") && !output.contains("phi read"));
    let cache =
        rusqlite::Connection::open(f.store("bob").cache_dir(&bob.profile).join("cache.db"))?;
    let cursor: u64 = cache.query_row(
        "SELECT seq FROM cursors WHERE thread=?",
        [batch.id.to_string()],
        |r| r.get(0),
    )?;
    ensure!(cursor == last.seq);
    let sibling_cursor: u64 = cache.query_row(
        "SELECT seq FROM cursors WHERE thread=?",
        [sibling.id.to_string()],
        |r| r.get(0),
    )?;
    ensure!(sibling_cursor == sibling_last.seq);
    let mut same_person =
        Client::open(f.store("bob"), bob.profile.clone(), Some("another")).await?;
    ensure!(
        same_person
            .inbox()
            .await?
            .iter()
            .all(|entry| entry.id != batch.id && entry.id != sibling.id)
    );
    ensure!(reviewer.wait_text(0).await?.is_empty());
    alice
        .group_create("notifications", &["@bob".into()])
        .await?;
    let grouped = alice
        .send(
            "#notifications",
            "Group task",
            options("for all bob handles"),
        )
        .await?;
    for client in [&mut reviewer, &mut second] {
        ensure!(client.wait(0).await?.iter().any(|event| matches!(event,
            WaitEntry::Message(entry) if entry.id == grouped.id)));
        ensure!(
            client
                .counts()
                .await?
                .channels
                .iter()
                .any(|channel| channel.channel_id == grouped.envelope.channel_id
                    && channel.unread_threads == 1)
        );
    }
    let read_before_wait = alice
        .send(
            "@bob",
            "Read before wait",
            options("do not replay this body"),
        )
        .await?;
    let json = reviewer
        .read(read_before_wait.id, ReadOptions::default())
        .await?
        .json();
    ensure!(json["messages"][0]["trusted"] == false && json["next_reply"]["required"] == false);
    let mut reopened = Client::open(f.store("bob"), bob.profile.clone(), Some("reviewer")).await?;
    ensure!(
        reopened.wait_text(0).await?.is_empty(),
        "read then wait must not replay the same handle's body"
    );
    let mut independent =
        Client::open(f.store("bob"), bob.profile.clone(), Some("independent")).await?;
    ensure!(
        independent
            .wait_text(0)
            .await?
            .contains("do not replay this body"),
        "different handles retain independent notification history"
    );
    let fresh = alice
        .reply(read_before_wait.id, options("fresh after read"))
        .await?;
    let delivered = reopened.wait_json(0).await?;
    ensure!(delivered["events"][0]["thread"]["messages"][0]["id"] == fresh.id.to_string());
    ensure!(delivered["events"][0]["thread"]["messages"][0]["trusted"] == false);
    ensure!(
        reopened.wait_json(0).await?["events"]
            .as_array()
            .is_some_and(|e| e.is_empty())
    );
    Ok(())
}
#[tokio::test]
async fn summaries_required_off_explicit_and_local_summarizer() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let unsummarized = alice
        .send("#all", "No summary", SendOptions::default())
        .await?;
    alice.reply(unsummarized.id, SendOptions::default()).await?;
    let preflight = alice.read(unsummarized.id, ReadOptions::default()).await?;
    ensure!(preflight.next_reply.required && preflight.next_reply.messages_since_summary == 3);
    ensure!(
        preflight
            .text()
            .contains("next reply: --summary required (3 since last")
            && !preflight.text().contains("previous summary")
    );
    let error = alice
        .reply(unsummarized.id, SendOptions::default())
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("third message must require a summary"))?;
    ensure!(
        error
            .to_string()
            .contains("3 messages since the last summary")
            && error
                .to_string()
                .contains("including this message; threshold 3")
    );
    alice
        .reply(unsummarized.id, options("first summary"))
        .await?;
    for _ in 0..2 {
        alice.reply(unsummarized.id, SendOptions::default()).await?;
    }
    let extended = alice.read(unsummarized.id, ReadOptions::default()).await?;
    ensure!(
        extended.next_reply.required
            && extended
                .text()
                .contains("previous summary (extend it): Summary: first summary")
    );
    ensure!(
        alice
            .reply(unsummarized.id, SendOptions::default())
            .await
            .is_err()
    );
    alice.set_settings("summary_every", "5").await?;
    for _ in 0..2 {
        alice.reply(unsummarized.id, SendOptions::default()).await?;
    }
    let error = alice
        .reply(unsummarized.id, SendOptions::default())
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("fifth message requires a summary"))?;
    ensure!(
        error
            .to_string()
            .contains("5 messages since the last summary")
            && error.to_string().contains("threshold 5")
    );
    ensure!(alice.set_settings("summary_every", "0").await.is_err());
    ensure!(
        alice
            .set_settings("summary_every", "4294967296")
            .await
            .is_err()
    );
    alice.set_settings("summary_every", "1").await?;
    ensure!(
        alice
            .send("#all", "Every message", SendOptions::default())
            .await
            .is_err()
    );
    let reopened = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(reopened.settings.summary_bytes == 1);
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
    // Summarizers run under the host shell (cmd on Windows, sh elsewhere) and
    // their stdout is kept verbatim apart from trimming, so cmd's CRLF stays.
    #[cfg(windows)]
    let (summarizer, expected) = (
        "(echo first line& echo second line)",
        "first line\r\nsecond line",
    );
    #[cfg(not(windows))]
    let (summarizer, expected) = (
        "printf 'first line\nsecond line\n'",
        "first line\nsecond line",
    );
    alice.profile.summarizer = Some(summarizer.into());
    let multiline = alice
        .send(
            "#all",
            "Multiline",
            SendOptions {
                body: "body".into(),
                ..Default::default()
            },
        )
        .await?;
    ensure!(
        alice
            .read(multiline.id, ReadOptions::default())
            .await?
            .summary
            .as_deref()
            == Some(expected)
    );
    alice.profile.summarizer = None;
    alice.set_settings("summary_mode", "off").await?;
    alice.reply(unsummarized.id, SendOptions::default()).await?;
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
    ensure!(view.summary_number == Some(8) && view.messages[0].number == 6);
    ensure!(
        view.text()
            .contains("summary (as of #8; 2 newer messages below)")
    );
    let view = alice
        .read(
            root.id,
            ReadOptions {
                last: Some(1),
                all: false,
            },
        )
        .await?;
    ensure!(
        view.messages.len() == 2 && view.messages[0].number == 9 && view.messages[1].number == 10
    );
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
    ensure!(laptop.wait(0).await?.is_empty());
    let own_history = laptop.read(root.id, ReadOptions::default()).await?.text();
    ensure!(
        own_history.contains("your other session")
            && !own_history.contains(UNTRUSTED_BANNER)
            && !own_history.contains("UNTRUSTED")
    );
    ensure!(
        laptop
            .read(root.id, ReadOptions::default())
            .await?
            .messages
            .len()
            == 1
    );
    let all = alice
        .channels
        .iter()
        .find(|c| c.channel.kind == ChannelKind::All)
        .ok_or_else(|| anyhow::anyhow!("all missing"))?
        .channel
        .id;
    f.faults(|faults| {
        faults.omit_device = Some(laptop.profile.device_id);
        faults.hide_revocations = true;
        faults.fail_revocations = true;
    })?;
    ensure!(
        alice
            .revoke(RevocationTarget::Device {
                device_id: laptop.profile.device_id
            })
            .await
            .is_err()
    );
    let cache =
        rusqlite::Connection::open(f.store("alice").cache_dir(&alice.profile).join("cache.db"))?;
    let pinned: String =
        cache.query_row("SELECT value FROM state WHERE name='trust'", [], |r| {
            r.get(0)
        })?;
    let pinned: serde_json::Value = serde_json::from_str(&pinned)?;
    ensure!(
        pinned["bundle"]["revocations"]
            .as_array()
            .is_some_and(|r| r.len() == 1)
    );
    ensure!(
        cache.query_row("SELECT COUNT(*) FROM pending_revocations", [], |r| r
            .get::<_, u64>(0))?
            == 1
    );
    f.faults(|faults| {
        faults.fail_revocations = false;
        faults.lose_revocation_response = true;
        faults.fail_metadata = Some(all);
    })?;
    ensure!(
        Client::open(f.store("alice"), alice.profile.clone(), None)
            .await
            .is_err()
    );
    ensure!(matches!(laptop.counts().await, Err(Error::Auth)));
    f.faults(|faults| faults.lose_revocation_response = false)?;
    ensure!(
        Client::open(f.store("alice"), alice.profile.clone(), None)
            .await
            .is_err()
    );
    ensure!(
        cache.query_row("SELECT COUNT(*) FROM pending_revocations", [], |r| r
            .get::<_, u64>(0))?
            == 0
    );
    // Every channel was processed even though the all-channel metadata PATCH failed.
    alice.refresh().await?;
    ensure!(alice.channels.iter().all(|c| if c.channel.id == all {
        c.channel.current_epoch == 0
    } else {
        c.channel.current_epoch == 1
    }));
    let protected: Option<String> = cache
        .query_row(
            "SELECT value FROM state WHERE name=?",
            [format!("protected/{all}")],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(protected.is_none());
    f.faults(|faults| faults.fail_metadata = None)?;
    alice = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(alice.channels.iter().all(|c| c.channel.current_epoch == 1));
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
    remaining_members_rekey_private_channels_unknown_to_removal_issuer().await?;
    Ok(())
}
#[tokio::test]
async fn groups_add_remove_rotate_and_enroll_after_rotation() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let group = alice.group_create("backend", &[]).await?;
    alice
        .send(
            "#backend",
            "Before Bob joined",
            options("private old history"),
        )
        .await?;
    f.faults(|faults| faults.fail_metadata = Some(group))?;
    ensure!(alice.group_member("backend", "@bob", false).await.is_err());
    f.faults(|faults| faults.fail_metadata = None)?;
    alice = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(
        alice
            .channels
            .iter()
            .find(|c| c.channel.id == group)
            .is_some_and(|c| c.channel.current_epoch == 0
                && !c.channel.members.contains(&bob.profile.person_id))
    );
    alice.group_member("backend", "@bob", false).await?;
    ensure!(
        alice
            .channels
            .iter()
            .find(|c| c.channel.id == group)
            .is_some_and(|c| c.channel.current_epoch == 1)
    );
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
    let repo = f.dir.path().join("repo");
    std::fs::create_dir_all(&repo)?;
    ensure!(f.store("alice").bind(&repo, None)?.space_id == alice.profile.space_id);
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
    let error = store
        .bind(&repo, None)
        .err()
        .ok_or_else(|| anyhow::anyhow!("ambiguous init must fail"))?
        .to_string();
    for profile in [&alice.profile, &other.profile] {
        ensure!(
            error.contains(&format!("{} ({})", profile.name, profile.space_id))
                && error.contains(&format!("phi init {}", profile.space_id))
        );
    }
    let nested = repo.join("deep");
    std::fs::create_dir_all(&nested)?;
    store.bind(&repo, Some("game"))?;
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
async fn names_slug_collisions_self_channel_and_link_content() -> Result<()> {
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
    ensure!(alice.counts().await?.unread_threads == 0);
    let mut other_session =
        Client::open(f.store("alice"), alice.profile.clone(), Some("second")).await?;
    ensure!(!other_session.fetch().await?.is_empty());
    ensure!(alice.wait(0).await?.is_empty());
    ensure!(
        other_session
            .wait(0)
            .await?
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(_)))
    );
    let own_view = other_session.read(root.id, ReadOptions::default()).await?;
    ensure!(
        own_view.text().contains("your other session")
            && !own_view.text().contains(UNTRUSTED_BANNER)
            && !own_view.text().contains("UNTRUSTED")
    );
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
    let called = alice
        .send(
            "@alice/cli",
            "Explicit self-call",
            options("for this handle"),
        )
        .await?;
    ensure!(alice.counts().await?.unread_threads == 1);
    ensure!(alice.wait(0).await?.iter().any(|event| matches!(event,
        WaitEntry::Message(entry) if entry.id == called.id)));
    ensure!(other_session.wait(0).await?.is_empty());
    for body in ["phi://host/space#secret", "PHIS://host/space#secret"] {
        let linked = alice.send("#all", body, options(body)).await?;
        let reply = alice.reply(linked.id, options(body)).await?;
        let view = alice
            .read(
                linked.id,
                ReadOptions {
                    all: true,
                    last: None,
                },
            )
            .await?;
        ensure!(
            view.title == body
                && view
                    .messages
                    .iter()
                    .all(|message| message.content.body == body)
        );
        ensure!(
            view.messages
                .last()
                .is_some_and(|message| message.message.id == reply.id)
        );
        ensure!(view.summary.as_deref() == Some(format!("Summary: {body}").as_str()));
    }
    Ok(())
}
#[tokio::test]
async fn local_cache_search_and_retention_prune_on_startup() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    alice.set_settings("retention_days", "2").await?;
    f.faults(|faults| {
        faults.backdate_messages = true;
        faults.unsigned_retention = Some(0);
    })?;
    let root = alice
        .send("#all", "Needle", options("searchable plaintext"))
        .await?;
    ensure!(alice.search("searchable").await?.len() == 1);
    ensure!(alice.search("absent").await?.is_empty());
    let cache = f.store("alice").cache_dir(&alice.profile).join("cache.db");
    let conn = rusqlite::Connection::open(cache)?;
    let mut reopened = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(reopened.settings.retention_days == 2);
    ensure!(reopened.search("searchable").await?.len() == 1);
    let observed: u64 = conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM state WHERE name=?",
        [format!("activity/{}", root.id)],
        |r| r.get(0),
    )?;
    ensure!(observed > 1);
    conn.execute("UPDATE messages SET created=1", [])?;
    conn.execute(
        "UPDATE state SET value='1' WHERE name LIKE 'activity/%'",
        [],
    )?;
    // Reading identical signed evidence must not refresh retention.
    reopened.read(root.id, ReadOptions::default()).await?;
    let activity: String = conn.query_row(
        "SELECT value FROM state WHERE name=?",
        [format!("activity/{}", root.id)],
        |r| r.get(0),
    )?;
    ensure!(activity == "1");
    let mut reopened = Client::open(f.store("alice"), alice.profile.clone(), None).await?;
    ensure!(reopened.search("searchable").await?.is_empty());
    reopened.read(root.id, ReadOptions::default()).await?;
    ensure!(reopened.search("searchable").await?.len() == 1);
    f.server.sweep(u64::MAX / 2).await?;
    ensure!(reopened.search("searchable").await?.len() == 1);
    Ok(())
}
#[tokio::test]
async fn http_errors_admin_token_rate_limit_protocol_and_auth() -> Result<()> {
    let f = Fixture::configured(|c| {
        c.allow_space_creation = Creation::Token;
        c.admin_token = Some("admin-secret".into());
        c.limits.messages_per_min = 1;
        c.limits.max_blob_bytes = 32;
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
    ensure!(alice.limits.max_attachment_bytes == 32);
    let file = f.dir.path().join("limit.txt");
    std::fs::write(&file, [b'x'; 33])?;
    let mut too_large = options("over advertised limit");
    too_large.files.push(file.clone());
    ensure!(alice.send("#all", "Too large", too_large).await.is_err());
    std::fs::write(&file, [b'x'; 32])?;
    let mut allowed = options("one");
    allowed.files.push(file);
    alice.send("#all", "First", allowed).await?;
    let error = alice
        .send("#all", "Second", options("two"))
        .await
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected rate limit"))?;
    ensure!(matches!(error, Error::RateLimited(_)) && error.exit_code() == 2);
    let response = reqwest::Client::new()
        .get(format!("{}/v1/spaces/{}", f.url, alice.profile.space_id))
        .header("Phi-Protocol", "99")
        .send()
        .await?;
    ensure!(response.status().as_u16() == 426);
    ensure!(response.headers()["Phi-Protocol"] == PROTOCOL);
    ensure!(Error::Upgrade(None).exit_code() == 4 && Error::HostUpgrade(None).exit_code() == 4);
    ensure!(Error::Auth.exit_code() == 3);
    Ok(())
}
#[tokio::test]
async fn persisted_trust_rejects_omitted_certificate_and_roster_rollback() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, bob) = f.pair().await?;
    alice.group_create("backend", &["@bob".into()]).await?;
    let link = alice.invite(true, false).await?;
    let mut laptop = Client::join(f.store("laptop"), &link, "alice").await?;
    alice.group_member("backend", "@bob", true).await?;
    let connection = rusqlite::Connection::open(f.dir.path().join("server/phi.db"))?;
    let channel = alice
        .channels
        .iter()
        .find(|c| c.meta.name == "backend")
        .ok_or_else(|| anyhow::anyhow!("missing group"))?
        .channel
        .clone();
    let accounts =
        FileKeyStore::open(&f.store("laptop").profile_dir(&laptop.profile).join("keys"))?;
    let epoch_account = format!("channel/{}/1", channel.id);
    ensure!(accounts.get(&epoch_account)?.is_none());
    let mut invalid_meta = channel.clone();
    invalid_meta.meta.ciphertext = Base64Bytes::new(vec![0; channel.meta.ciphertext.len()]);
    connection.execute(
        "UPDATE channels SET json=? WHERE id=?",
        rusqlite::params![
            serde_json::to_string(&invalid_meta)?,
            channel.id.to_string()
        ],
    )?;
    ensure!(laptop.refresh().await.is_err());
    ensure!(accounts.get(&epoch_account)?.is_none());
    // A former key holder can produce a valid sealed box and metadata under old
    // bytes relabeled as an existing new epoch. Only the signed commitment detects it.
    let prior = accounts
        .get(&format!("channel/{}/0", channel.id))?
        .ok_or_else(|| anyhow::anyhow!("old key missing"))?;
    let prior: KeyBytes = serde_json::from_slice(&prior)?;
    let recipient = laptop
        .devices
        .iter()
        .find(|d| d.cert.data.device_id == laptop.profile.device_id)
        .ok_or_else(|| anyhow::anyhow!("recipient missing"))?
        .clone();
    let mut entropy =
        |bytes: &mut [u8]| getrandom::fill(bytes).map_err(|_| phi_crypto::CryptoError::Entropy);
    let context = serde_json::to_vec(&(
        "phi wrapped key v1",
        (
            laptop.profile.space_id,
            channel.id,
            1_u64,
            laptop.profile.device_id,
        ),
    ))?;
    let forged = WrappedKey {
        channel_id: channel.id,
        epoch: 1,
        device_id: laptop.profile.device_id,
        sealed: phi_crypto::seal(
            &recipient.cert.data.kex_pub,
            prior.as_bytes(),
            &context,
            &mut entropy,
        )?,
    };
    let original_wrap: String = connection.query_row(
        "SELECT json FROM wrapped_keys WHERE channel_id=? AND epoch=1 AND device_id=?",
        rusqlite::params![channel.id.to_string(), laptop.profile.device_id.to_string()],
        |r| r.get(0),
    )?;
    let meta = alice
        .channels
        .iter()
        .find(|c| c.channel.id == channel.id)
        .ok_or_else(|| anyhow::anyhow!("meta missing"))?
        .meta
        .clone();
    let old_as_new = phi_crypto::ChannelKey::from_bytes(
        laptop.profile.space_id,
        channel.id,
        1,
        *prior.as_bytes(),
    );
    let mut forged_channel = channel.clone();
    forged_channel.meta = phi_crypto::encrypt_channel_metadata(
        &old_as_new,
        &serde_json::to_vec(&meta)?,
        &mut entropy,
    )?;
    connection.execute(
        "UPDATE channels SET json=? WHERE id=?",
        rusqlite::params![
            serde_json::to_string(&forged_channel)?,
            channel.id.to_string()
        ],
    )?;
    connection.execute(
        "UPDATE wrapped_keys SET json=? WHERE channel_id=? AND epoch=1 AND device_id=?",
        rusqlite::params![
            serde_json::to_string(&forged)?,
            channel.id.to_string(),
            laptop.profile.device_id.to_string()
        ],
    )?;
    ensure!(laptop.refresh().await.is_err());
    ensure!(accounts.get(&epoch_account)?.is_none());
    let pinned = rusqlite::Connection::open(
        f.store("laptop")
            .cache_dir(&laptop.profile)
            .join("cache.db"),
    )?;
    let roster: String = pinned.query_row(
        "SELECT value FROM state WHERE name=?",
        [format!("roster/{}", channel.id)],
        |r| r.get(0),
    )?;
    ensure!(
        serde_json::from_str::<ChannelMembership>(&roster)?
            .data
            .epoch
            == 0
    );
    connection.execute(
        "UPDATE channels SET json=? WHERE id=?",
        rusqlite::params![serde_json::to_string(&channel)?, channel.id.to_string()],
    )?;
    connection.execute(
        "UPDATE wrapped_keys SET json=? WHERE channel_id=? AND epoch=1 AND device_id=?",
        rusqlite::params![
            original_wrap,
            channel.id.to_string(),
            laptop.profile.device_id.to_string()
        ],
    )?;
    laptop.refresh().await?;
    ensure!(accounts.get(&epoch_account)?.is_some());
    // Even a correctly encrypted box for an uncommitted future epoch cannot reserve an account.
    let future_context = serde_json::to_vec(&(
        "phi wrapped key v1",
        (
            laptop.profile.space_id,
            channel.id,
            99_u64,
            laptop.profile.device_id,
        ),
    ))?;
    let future = WrappedKey {
        epoch: 99,
        sealed: phi_crypto::seal(
            &recipient.cert.data.kex_pub,
            prior.as_bytes(),
            &future_context,
            &mut entropy,
        )?,
        ..forged
    };
    connection.execute(
        "INSERT INTO wrapped_keys VALUES(?,?,?,?)",
        rusqlite::params![
            channel.id.to_string(),
            99,
            laptop.profile.device_id.to_string(),
            serde_json::to_string(&future)?
        ],
    )?;
    ensure!(laptop.refresh().await.is_err());
    ensure!(
        accounts
            .get(&format!("channel/{}/99", channel.id))?
            .is_none()
    );
    connection.execute(
        "DELETE FROM wrapped_keys WHERE channel_id=? AND epoch=99",
        [channel.id.to_string()],
    )?;
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
    let (mut alice, mut bob) = f.pair().await?;
    let path: PathBuf = f.dir.path().join("file.txt");
    std::fs::write(&path, b"safe")?;
    let mut opts = options("signed body");
    opts.files.push(path);
    let root = alice.send("#all", "Signed", opts).await?;
    let blobs = std::fs::read_dir(f.dir.path().join("server/blobs"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    let blob = blobs
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing uploaded blob"))?
        .path();
    let ciphertext = std::fs::read(&blob)?;
    std::fs::write(&blob, b"tampered")?;
    ensure!(bob.wait_text(0).await.is_err());
    ensure!(bob.counts().await?.unread_threads == 1);
    std::fs::write(&blob, ciphertext)?;
    let output = bob.wait_text(0).await?;
    ensure!(output.contains("signed body") && output.contains("attachments:"));
    ensure!(bob.counts().await?.unread_threads == 0);
    alice.react(root.id, ReactionKind::Up, false).await?;
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
    ensure!(bob.wait_text(0).await?.is_empty());
    connection.execute(
        "UPDATE messages SET json=? WHERE id=?",
        rusqlite::params![original, root.id.to_string()],
    )?;
    ensure!(bob.wait_text(0).await?.is_empty());
    let bad = alice
        .send("#all", "Bad event", options("host corrupts this event"))
        .await?;
    let good = alice
        .send("#all", "Good event", options("delivered after rejection"))
        .await?;
    f.faults(|faults| {
        faults.corrupt_event = Some(bad.id);
        faults.inflate_cursor = true;
    })?;
    let events = bob.wait(0).await?;
    ensure!(
        events
            .iter()
            .any(|e| matches!(e, WaitEntry::VerificationFailure { seq, .. } if *seq == bad.seq))
    );
    ensure!(
        events
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(entry) if entry.latest_seq == good.seq))
    );
    let cache =
        rusqlite::Connection::open(f.store("bob").cache_dir(&bob.profile).join("cache.db"))?;
    ensure!(
        cache.query_row(
            "SELECT COUNT(*) FROM wait_outcomes WHERE error IS NOT NULL",
            [],
            |r| r.get::<_, u64>(0)
        )? == 1
    );
    f.faults(|faults| {
        faults.corrupt_event = None;
        faults.inflate_cursor = false;
    })?;
    let later = alice
        .send("#all", "Still live", options("cursor not poisoned"))
        .await?;
    let events = bob.wait(0).await?;
    ensure!(
        events
            .iter()
            .any(|e| matches!(e, WaitEntry::Message(entry) if entry.latest_seq == later.seq))
    );
    ensure!(bob.wait(0).await?.is_empty());
    let paths: Vec<_> = std::fs::read_dir(f.dir.path().join("server/blobs"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(!paths.is_empty());
    std::fs::write(paths[0].path(), b"tampered")?;
    ensure!(alice.read(root.id, ReadOptions::default()).await.is_err());
    f.server.sweep(u64::MAX / 2).await?;
    Ok(())
}

#[tokio::test]
async fn gui_and_cli_share_one_profile_and_database() -> Result<()> {
    let f = Fixture::new().await?;
    let (alice, mut bob) = f.pair().await?;
    let store = f.store("alice");
    // Both front ends stay open at once on the same keys and cache, as the real apps do.
    let mut cli = Client::open(store.clone(), alice.profile.clone(), None).await?;
    let mut gui = Client::open_gui(store.clone(), alice.profile.clone()).await?;
    ensure!(store.profiles()?.len() == 1 && gui.profile.device_id == cli.profile.device_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let keys = store.profile_dir(&gui.profile).join("keys");
        ensure!(std::fs::metadata(keys)?.permissions().mode() & 0o777 == 0o600);
    }
    let root = gui
        .send("#all", "Human message", {
            // A person typing in the GUI is never asked for a thread summary.
            let mut message = options("from desktop");
            message.summary = None;
            message
        })
        .await?;
    for n in 0..4 {
        let mut message = options(&format!("human reply {n}"));
        message.summary = None;
        gui.reply(root.id, message).await?;
    }
    // The CLI sees the GUI's writes without reopening, labelled by the signed flag.
    let view = cli.read(root.id, ReadOptions::default()).await?;
    ensure!(view.text().contains("human · sent from GUI"));
    ensure!(
        bob.read(root.id, ReadOptions::default())
            .await?
            .text()
            .contains("human · sent from GUI")
    );
    let threads = cli.threads().await?;
    ensure!(threads.len() == 1 && !threads[0].own_agent_activity);
    // Agents on the CLI still owe summaries once the threshold is reached.
    let mut bare = options("agent follow-up");
    bare.summary = None;
    ensure!(matches!(
        cli.reply(root.id, bare).await,
        Err(Error::SummaryRequired(_))
    ));
    cli.reply(root.id, options("Agent follow-up")).await?;
    ensure!(gui.threads().await?[0].own_agent_activity);
    // A mention of any handle reaches the person in the GUI, but only that handle's CLI session.
    let mention = bob
        .send(
            "@alice/reviewer",
            "Agent mention",
            options("For your reviewer"),
        )
        .await?;
    ensure!(
        gui.inbox()
            .await?
            .iter()
            .any(|entry| entry.id == mention.id && entry.mentioned)
    );
    ensure!(
        !cli.inbox()
            .await?
            .iter()
            .any(|entry| entry.id == mention.id && entry.mentioned)
    );
    // The shared core can be moved into a GUI async task without a local executor.
    tokio::spawn(async move {
        let mut client = gui;
        client.counts().await
    })
    .await??;
    Ok(())
}

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

#[tokio::test]
async fn wait_survives_transient_gateway_errors() -> Result<()> {
    let mut f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    alice
        .send("@bob/cli", "Ping", options("Proxy restarted mid-wait."))
        .await?;
    f.faults(|faults| faults.drop_event_polls = 2)?;
    let waited = bob.wait_text(10).await?;
    ensure!(waited.contains("Proxy restarted mid-wait."));
    // Gateway errors that outlast the deadline look like a quiet timeout and leave the cursor.
    alice
        .send("@bob/cli", "Queued", options("Still queued."))
        .await?;
    f.faults(|faults| faults.drop_event_polls = 100)?;
    let started = std::time::Instant::now();
    let missed = bob.wait_text(3).await?;
    ensure!(missed.is_empty());
    ensure!(started.elapsed() >= Duration::from_millis(1500));
    ensure!(started.elapsed() < Duration::from_secs(8));
    f.faults(|faults| faults.drop_event_polls = 0)?;
    ensure!(bob.wait_text(0).await?.contains("Still queued."));
    for (status, needle) in [
        (401, "authentication"),
        (403, "HTTP 403"),
        (400, "HTTP 400"),
        (426, "ask the host to upgrade"),
        (500, "HTTP 500"),
    ] {
        f.faults(|faults| faults.event_reject = Some(status))?;
        let started = std::time::Instant::now();
        let error = bob.wait_text(30).await.err().map(|e| e.to_string());
        ensure!(
            started.elapsed() < Duration::from_secs(2),
            "{status} took {:?}",
            started.elapsed()
        );
        ensure!(
            error.as_deref().is_some_and(|text| text.contains(needle)),
            "{status} {error:?}"
        );
    }
    f.faults(|faults| faults.event_reject = None)?;
    let polls = f.host.polls.clone();
    let waiting = tokio::spawn(async move { bob.wait_text(15).await });
    let ready = std::time::Instant::now();
    while polls.load(Ordering::SeqCst) == 0 {
        ensure!(ready.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    f.restart().await?;
    alice
        .send("@bob/cli", "Back", options("Server came back."))
        .await?;
    let waited = waiting
        .await
        .map_err(|error| anyhow::anyhow!("wait task: {error}"))??;
    ensure!(waited.contains("Server came back."));
    Ok(())
}

#[tokio::test]
async fn revoking_the_root_keeps_history_invites_and_devices_working() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let link = alice.invite(false, false).await?;
    let mut bob = Client::join(f.store("bob"), &link, "bob").await?;
    alice.refresh().await?;
    let root = alice
        .send("#all", "Root history", options("clear history"))
        .await?;
    alice.set_settings("summary_every", "7").await?;
    alice.close_thread(root.id).await?;
    bob.react(root.id, ReactionKind::Ack, false).await?;
    bob.revoke(RevocationTarget::Person {
        person_id: alice.profile.person_id,
    })
    .await?;
    let invite = bob.invite(false, false).await?;
    let mut carol = Client::join(f.store("carol"), &invite, "carol").await?;
    ensure!(carol.settings.summary_bytes == 7);
    let text = carol.read(root.id, ReadOptions::default()).await?.text();
    ensure!(
        text.contains("clear history")
            && text.contains("status: closed")
            && !text.contains("VERIFICATION FAILED")
    );
    let closed = carol.discover_threads_filtered(Some("#all"), true).await?;
    ensure!(
        closed
            .iter()
            .any(|thread| thread.id == root.id && thread.closed)
    );
    ensure!(!carol.wait_text(0).await?.contains("VERIFICATION FAILED"));
    let laptop_link = carol.invite(true, true).await?;
    let mut laptop = Client::join(f.store("laptop"), &laptop_link, "carol").await?;
    let laptop_text = laptop.read(root.id, ReadOptions::default()).await?.text();
    ensure!(
        laptop_text.contains("clear history")
            && laptop_text.contains("status: closed")
            && !laptop_text.contains("VERIFICATION FAILED")
    );
    let reopened = Client::open(f.store("carol"), carol.profile.clone(), None).await?;
    ensure!(reopened.settings.summary_bytes == 7);
    Ok(())
}

#[tokio::test]
async fn leaving_removes_only_this_device_and_the_others_rekey() -> Result<()> {
    let f = Fixture::new().await?;
    let (mut alice, mut bob) = f.pair().await?;
    let enroll = bob.invite(true, true).await?;
    let mut laptop = Client::join(f.store("laptop"), &enroll, "bob").await?;
    let old = bob.send("#all", "Before", options("old")).await?;
    let profile = bob.profile.clone();
    bob.leave().await?;
    f.store("bob").remove_profile(&profile)?;
    ensure!(f.store("bob").profiles()?.is_empty());
    ensure!(!f.store("bob").cache_dir(&profile).exists());
    let new = alice.send("#all", "After", options("new")).await?;
    ensure!(new.envelope.epoch == 1);
    ensure!(
        alice
            .read(old.id, ReadOptions::default())
            .await?
            .text()
            .contains("old")
    );
    let seen = laptop.read(new.id, ReadOptions::default()).await?.text();
    ensure!(seen.contains("new") && !seen.contains("VERIFICATION FAILED"));
    Ok(())
}

#[tokio::test]
async fn revoked_device_and_colluding_host_cannot_extend_history() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let bob_link = alice.invite(false, false).await?;
    let mut bob = Client::join(f.store("bob"), &bob_link, "bob").await?;
    alice.refresh().await?;
    let carol_link = alice.invite(false, false).await?;
    let mut carol = Client::join(f.store("carol"), &carol_link, "carol").await?;
    alice.refresh().await?;
    bob.refresh().await?;
    let root = alice
        .send("#all", "Visible root", options("root body"))
        .await?;
    let reply = bob.reply(root.id, options("listed secret")).await?;
    bob.group_create("quiet", &["@carol".into()]).await?;
    alice
        .revoke(RevocationTarget::Device {
            device_id: bob.profile.device_id,
        })
        .await?;
    carol.refresh().await?;
    let honest = carol.read(root.id, ReadOptions::default()).await?.text();
    ensure!(honest.contains("listed secret") && !honest.contains("VERIFICATION FAILED"));
    let dave_link = alice.invite(false, false).await?;
    let mut dave = Client::join(f.store("dave"), &dave_link, "dave").await?;
    let bob_keys = stored_device_keys(&f.store("bob"), &bob.profile)?;
    let db_path = f.dir.path().join("server/phi.db");
    let (original_reply, copy_id) = {
        let conn = rusqlite::Connection::open(&db_path)?;
        let original_reply: String = conn.query_row(
            "SELECT json FROM messages WHERE id=?",
            [reply.id.to_string()],
            |row| row.get(0),
        )?;
        let mut copy: Message = serde_json::from_str(&original_reply)?;
        copy_id_of(&mut copy, &bob_keys)?;
        let seq: u64 = conn.query_row(
            "UPDATE spaces SET seq=seq+1 WHERE id=? RETURNING seq",
            [alice.profile.space_id.to_string()],
            |row| row.get(0),
        )?;
        copy.seq = seq;
        conn.execute(
            "INSERT INTO messages(id, space_id, channel_id, thread_id, seq, author_person, json) VALUES(?,?,?,?,?,?,?)",
            rusqlite::params![
                copy.id.to_string(),
                copy.envelope.space_id.to_string(),
                copy.envelope.channel_id.to_string(),
                root.id.to_string(),
                seq,
                copy.author_person.to_string(),
                serde_json::to_string(&copy)?
            ],
        )?;
        let mut changed: Message = serde_json::from_str(&original_reply)?;
        changed.envelope.handle = "forged".parse()?;
        changed.signature = bob_keys.sign(&phi_crypto::message_signing_bytes(&changed.envelope)?);
        conn.execute(
            "UPDATE messages SET json=? WHERE id=?",
            rusqlite::params![serde_json::to_string(&changed)?, reply.id.to_string()],
        )?;
        (original_reply, copy.id)
    };
    for reader in [&mut carol, &mut dave] {
        let text = reader.read(root.id, ReadOptions::default()).await?.text();
        ensure!(
            text.matches("VERIFICATION FAILED").count() >= 2
                && text
                    .contains("revoked device, or not covered by its issuer's revocation history")
                && text.contains("root body")
                && !text.contains("listed secret")
        );
    }
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "UPDATE messages SET json=? WHERE id=?",
            rusqlite::params![original_reply, reply.id.to_string()],
        )?;
        conn.execute("DELETE FROM messages WHERE id=?", [copy_id.to_string()])?;
        // read() advances the cursor to the forged seq. The server rejects a cursor past the
        // messages that remain, and the next sync would copy that cursor back into the cache.
        conn.execute(
            "UPDATE cursors SET seq=? WHERE thread_id=?",
            rusqlite::params![reply.seq, root.id.to_string()],
        )?;
    }
    let restored = carol.read(root.id, ReadOptions::default()).await?.text();
    ensure!(restored.contains("listed secret") && !restored.contains("VERIFICATION FAILED"));

    let original_channel = {
        let conn = rusqlite::Connection::open(&db_path)?;
        let group_id: String = conn.query_row(
            "SELECT id FROM channels WHERE kind='group' AND json_extract(json,'$.members') LIKE ?",
            [format!("%{}%", carol.profile.person_id)],
            |row| row.get(0),
        )?;
        let original: String =
            conn.query_row("SELECT json FROM channels WHERE id=?", [group_id], |row| {
                row.get(0)
            })?;
        let mut channel: Channel = serde_json::from_str(&original)?;
        let mut data = channel.membership.data.clone();
        data.issuer_device = bob.profile.device_id;
        data.previous = Some(channel.membership.signature.clone());
        let signed = phi_crypto::sign_membership(data, &bob_keys)?;
        channel.membership_history.push(channel.membership.clone());
        channel.membership = signed;
        conn.execute(
            "UPDATE channels SET json=? WHERE id=?",
            rusqlite::params![serde_json::to_string(&channel)?, channel.id.to_string()],
        )?;
        original
    };
    ensure!(matches!(
        carol.refresh().await,
        Err(Error::Crypto(phi_crypto::CryptoError::Revoked))
    ));
    ensure!(dave.refresh().await.is_ok());
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        let group_id: String =
            conn.query_row("SELECT id FROM channels WHERE kind='group'", [], |row| {
                row.get(0)
            })?;
        conn.execute(
            "UPDATE channels SET json=? WHERE id=?",
            rusqlite::params![original_channel, group_id],
        )?;
    }
    carol.refresh().await?;

    let original_space = {
        let conn = rusqlite::Connection::open(&db_path)?;
        let original: String = conn.query_row(
            "SELECT json FROM spaces WHERE id=?",
            [alice.profile.space_id.to_string()],
            |row| row.get(0),
        )?;
        let mut space: Space = serde_json::from_str(&original)?;
        let mut settings = space.settings.clone();
        settings.summary_bytes = settings.summary_bytes.saturating_add(1);
        space.settings_change = Some(phi_crypto::sign_settings(
            SettingsChangeData {
                space_id: alice.profile.space_id,
                issuer_device: bob.profile.device_id,
                created_at: 50,
                settings: settings.clone(),
            },
            &bob_keys,
        )?);
        space.settings = settings;
        conn.execute(
            "UPDATE spaces SET json=? WHERE id=?",
            rusqlite::params![
                serde_json::to_string(&space)?,
                alice.profile.space_id.to_string()
            ],
        )?;
        original
    };
    ensure!(matches!(
        carol.refresh().await,
        Err(Error::Crypto(phi_crypto::CryptoError::Revoked))
    ));
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute(
            "UPDATE spaces SET json=? WHERE id=?",
            rusqlite::params![original_space, alice.profile.space_id.to_string()],
        )?;
    }
    carol.refresh().await?;

    {
        let conn = rusqlite::Connection::open(&db_path)?;
        let reaction = phi_crypto::sign_reaction(
            ReactionData {
                space_id: alice.profile.space_id,
                message_id: root.id,
                person_id: bob.profile.person_id,
                device_id: bob.profile.device_id,
                handle: "forged".parse()?,
                kind: ReactionKind::Claim,
                created_at: 50,
                remove: false,
            },
            &bob_keys,
        )?;
        conn.execute(
            "INSERT INTO reactions(message_id, person_id, handle, kind, json) VALUES(?,?,?,?,?)",
            rusqlite::params![
                root.id.to_string(),
                bob.profile.person_id.to_string(),
                "forged",
                "claim",
                serde_json::to_string(&reaction)?
            ],
        )?;
    }
    let claimed = carol.read(root.id, ReadOptions::default()).await?.text();
    ensure!(
        claimed.contains("listed secret")
            && claimed.contains("reactions: none")
            && !claimed.contains("claimed by")
            && !claimed.contains("forged")
    );
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute("DELETE FROM reactions WHERE handle='forged'", [])?;
    }
    let clean = carol.read(root.id, ReadOptions::default()).await?.text();
    ensure!(clean.contains("listed secret") && !clean.contains("VERIFICATION FAILED"));
    dave.refresh().await?;
    Ok(())
}

fn copy_id_of(message: &mut Message, keys: &phi_crypto::DeviceKeys) -> Result<()> {
    let id = MessageId::from_bytes([0xab; 16]);
    message.id = id;
    message.envelope.id = id;
    message.signature = keys.sign(&phi_crypto::message_signing_bytes(&message.envelope)?);
    Ok(())
}

#[tokio::test]
async fn private_channels_the_revoker_cannot_see_survive_revocation() -> Result<()> {
    let f = Fixture::new().await?;
    let mut alice = f.alice().await?;
    let bob_link = alice.invite(false, false).await?;
    let mut bob = Client::join(f.store("bob"), &bob_link, "bob").await?;
    alice.refresh().await?;
    let carol_link = alice.invite(false, false).await?;
    let carol = Client::join(f.store("carol"), &carol_link, "carol").await?;
    alice.refresh().await?;
    bob.refresh().await?;
    let carol_profile = carol.profile.clone();
    bob.group_create("secret", &["@carol".into()]).await?;
    bob.group_member("secret", "@carol", false).await?;
    let group = bob
        .channels
        .iter()
        .find(|channel| channel.meta.name == "secret")
        .ok_or_else(|| anyhow::anyhow!("secret group missing"))?;
    ensure!(
        group.channel.membership_history.len() == 1
            && group.channel.membership.data.issuer_device == bob.profile.device_id
            && group.channel.membership_history[0].data.issuer_device == bob.profile.device_id
            && group.channel.current_epoch == group.channel.membership_history[0].data.epoch
    );
    let group_id = group.channel.id;
    let group_message = bob
        .send("#secret", "Group", options("group history"))
        .await?;
    let dm_message = bob.send("@carol", "DM", options("dm history")).await?;
    let dm_id = dm_message.envelope.channel_id;
    // Open resumes rotations, so Carol may already have dropped Bob before she sends.
    // Compare with the epochs from before the revocation either way.
    let group_epoch = bob
        .channels
        .iter()
        .find(|channel| channel.channel.id == group_id)
        .map(|channel| channel.channel.current_epoch)
        .ok_or_else(|| anyhow::anyhow!("group missing before revocation"))?;
    let dm_epoch = bob
        .channels
        .iter()
        .find(|channel| channel.channel.id == dm_id)
        .map(|channel| channel.channel.current_epoch)
        .ok_or_else(|| anyhow::anyhow!("dm missing before revocation"))?;
    alice
        .revoke(RevocationTarget::Person {
            person_id: bob.profile.person_id,
        })
        .await?;
    ensure!(
        !alice
            .channels
            .iter()
            .any(|channel| { channel.channel.id == group_id || channel.channel.id == dm_id })
    );
    let cache =
        rusqlite::Connection::open(f.store("alice").cache_dir(&alice.profile).join("cache.db"))?;
    let pending: u64 = cache.query_row("SELECT COUNT(*) FROM pending_revocations", [], |row| {
        row.get(0)
    })?;
    ensure!(pending == 0);
    drop(cache);
    let server = rusqlite::Connection::open(f.dir.path().join("server/phi.db"))?;
    let revocations: u64 =
        server.query_row("SELECT COUNT(*) FROM revocations", [], |row| row.get(0))?;
    ensure!(revocations == 1);
    drop(server);
    let mut carol = Client::open(f.store("carol"), carol_profile, None).await?;
    let group_text = carol
        .read(group_message.id, ReadOptions::default())
        .await?
        .text();
    let dm_text = carol
        .read(dm_message.id, ReadOptions::default())
        .await?
        .text();
    ensure!(
        group_text.contains("group history")
            && dm_text.contains("dm history")
            && !group_text.contains("VERIFICATION FAILED")
            && !dm_text.contains("VERIFICATION FAILED")
    );
    carol
        .send("#secret", "After", options("rotated group"))
        .await?;
    carol
        .send("@bob", "After dm", options("rotated dm"))
        .await?;
    let group = carol
        .channels
        .iter()
        .find(|channel| channel.channel.id == group_id)
        .ok_or_else(|| anyhow::anyhow!("group missing after send"))?;
    ensure!(
        group.channel.current_epoch > group_epoch
            && !group.channel.members.contains(&bob.profile.person_id)
            && group.channel.members.contains(&carol.profile.person_id)
    );
    let dm = carol
        .channels
        .iter()
        .find(|channel| channel.channel.id == dm_id)
        .ok_or_else(|| anyhow::anyhow!("dm missing after send"))?;
    ensure!(
        dm.channel.current_epoch > dm_epoch
            && dm.channel.members.contains(&bob.profile.person_id)
            && dm.channel.members.contains(&carol.profile.person_id)
    );
    let laptop_link = carol.invite(true, true).await?;
    let mut laptop = Client::join(f.store("laptop"), &laptop_link, "carol").await?;
    let laptop_group = laptop
        .read(group_message.id, ReadOptions::default())
        .await?
        .text();
    let laptop_dm = laptop
        .read(dm_message.id, ReadOptions::default())
        .await?
        .text();
    ensure!(
        laptop_group.contains("group history")
            && laptop_dm.contains("dm history")
            && !laptop_group.contains("VERIFICATION FAILED")
            && !laptop_dm.contains("VERIFICATION FAILED")
    );
    Ok(())
}
