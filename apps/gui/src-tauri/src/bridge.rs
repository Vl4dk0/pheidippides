use base64::{Engine, engine::general_purpose::STANDARD};
use phi_core::{
    Client, DeviceOptions, Profile, ReadOptions, SendOptions, Store, WaitEntry, reaction_summary,
};
use phi_proto::{
    ChannelKind, DeviceKind, MessageId, ReactionKind, RevocationTarget, SpaceId, SummaryMode,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::State;

fn profile_id(profile: &Profile) -> String {
    blake3::hash(
        format!(
            "{}/{}/{}",
            profile.server, profile.space_id, profile.device_id
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string()
}

pub struct AppState(pub Store);
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Serialize)]
pub struct ProfileView {
    id: String,
    name: String,
    server: String,
    space_id: SpaceId,
    person_id: phi_proto::PersonId,
    device_id: phi_proto::DeviceId,
    display_name: String,
    summarizer_configured: bool,
}
impl From<&Profile> for ProfileView {
    fn from(p: &Profile) -> Self {
        Self {
            id: profile_id(p),
            name: p.name.clone(),
            server: p.server.clone(),
            space_id: p.space_id,
            person_id: p.person_id,
            device_id: p.device_id,
            display_name: p.display_name.clone(),
            summarizer_configured: p.summarizer.is_some(),
        }
    }
}
#[derive(Serialize)]
struct ChannelView {
    id: phi_proto::ChannelId,
    name: String,
    kind: ChannelKind,
}
#[derive(Serialize)]
struct DeviceView {
    id: phi_proto::DeviceId,
    person_id: phi_proto::PersonId,
    kind: DeviceKind,
    revoked_at: Option<u64>,
    last_seen_at: Option<u64>,
}
#[derive(Serialize)]
struct ReactionView {
    kind: ReactionKind,
    person_id: phi_proto::PersonId,
    author: String,
    handle: String,
    created_at: u64,
}
fn reactions(client: &Client, reactions: &[phi_proto::Reaction]) -> Vec<ReactionView> {
    reactions
        .iter()
        .map(|r| ReactionView {
            kind: r.data.kind,
            person_id: r.data.person_id,
            author: client.person_name(r.data.person_id),
            handle: r.data.handle.to_string(),
            created_at: r.data.created_at,
        })
        .collect()
}
#[derive(Serialize)]
struct ClaimView {
    message: MessageId,
    number: usize,
    person: phi_proto::PersonId,
    author: String,
    handle: String,
}
#[derive(Serialize)]
struct ReactionSummaryView {
    up: usize,
    down: usize,
    ack: usize,
    claims: Vec<ClaimView>,
}
fn summary(client: &Client, reactions: &[phi_proto::Reaction]) -> ReactionSummaryView {
    let summary = reaction_summary(reactions, &client.names());
    ReactionSummaryView {
        up: summary.up,
        down: summary.down,
        ack: summary.ack,
        claims: summary
            .claims
            .into_iter()
            .map(|c| ClaimView {
                number: client
                    .cached_reference(c.message)
                    .ok()
                    .flatten()
                    .map_or(0, |r| r.number),
                message: c.message,
                person: c.person,
                author: c.author,
                handle: c.handle.to_string(),
            })
            .collect(),
    }
}
#[derive(Serialize)]
struct ThreadEntry {
    id: MessageId,
    channel: String,
    to: String,
    title: String,
    from: String,
    created_at: u64,
    summary: Option<String>,
    mentioned: bool,
    unread: bool,
    own_agent_activity: bool,
    latest_seq: u64,
    reactions: Vec<ReactionView>,
    reaction_summary: ReactionSummaryView,
}
#[derive(Serialize)]
struct Snapshot {
    profile: ProfileView,
    channels: Vec<ChannelView>,
    threads: Vec<ThreadEntry>,
    retention_days: u32,
    summary_mode: SummaryMode,
    summary_every: u32,
    members: BTreeMap<phi_proto::PersonId, String>,
    devices: Vec<DeviceView>,
}
#[derive(Serialize)]
struct AttachmentView {
    name: String,
    mime: String,
    size: u64,
    path: String,
    preview: Option<String>,
}
#[derive(Serialize)]
struct MessageView {
    id: MessageId,
    number: usize,
    author: String,
    person_id: phi_proto::PersonId,
    handle: String,
    kind: DeviceKind,
    created_at: u64,
    body: String,
    verification_error: Option<String>,
    reactions: Vec<ReactionView>,
    reaction_summary: ReactionSummaryView,
    attachments: Vec<AttachmentView>,
}
#[derive(Serialize)]
struct ThreadView {
    id: MessageId,
    title: String,
    participants: Vec<String>,
    summary: Option<String>,
    summary_number: Option<usize>,
    newer_messages: usize,
    messages: Vec<MessageView>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Provision {
    Join {
        link: String,
        display_name: String,
    },
    Create {
        server: String,
        name: String,
        display_name: String,
        admin_token: Option<String>,
    },
}
fn device() -> DeviceOptions {
    DeviceOptions {
        kind: DeviceKind::Gui,
        name: "gui".into(),
    }
}
#[tauri::command]
pub fn profiles(state: State<'_, AppState>) -> Result<Vec<ProfileView>, String> {
    Ok(state
        .0
        .profiles()
        .map_err(err)?
        .iter()
        .filter(|p| p.kind == DeviceKind::Gui)
        .map(ProfileView::from)
        .collect())
}
#[tauri::command]
pub async fn provision(
    state: State<'_, AppState>,
    request: Provision,
) -> Result<ProfileView, String> {
    let store = state.0.clone();
    let client = match request {
        Provision::Join { link, display_name } => {
            Client::join_for_device(store.clone(), &link, &display_name, device()).await
        }
        Provision::Create {
            server,
            name,
            display_name,
            admin_token,
        } => {
            Client::create_for_device(
                store.clone(),
                &server,
                &name,
                &display_name,
                admin_token,
                device(),
            )
            .await
        }
    }
    .map_err(err)?;
    Ok(ProfileView::from(&client.profile))
}
async fn open(store: &Store, id: &str) -> Result<Client, String> {
    let profile = store
        .profiles()
        .map_err(err)?
        .into_iter()
        .find(|p| p.kind == DeviceKind::Gui && profile_id(p) == id)
        .ok_or("unknown GUI profile")?;
    Client::open(store.clone(), profile, Some("gui"))
        .await
        .map_err(err)
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Snapshot,
    Read {
        thread: MessageId,
    },
    Send {
        thread: Option<MessageId>,
        to: String,
        title: String,
        body: String,
    },
    React {
        message: MessageId,
        reaction: ReactionKind,
        remove: bool,
    },
    Unread {
        thread: MessageId,
    },
    Settings {
        retention_days: u32,
        summary_mode: SummaryMode,
        summary_every: u32,
    },
    DisplayName {
        name: String,
    },
    Revoke {
        target: RevocationTarget,
    },
    Grant {
        enroll: bool,
        recovery: bool,
    },
}
#[tauri::command]
pub async fn execute(
    state: State<'_, AppState>,
    profile: String,
    request: Request,
) -> Result<serde_json::Value, String> {
    let mut client = open(&state.0, &profile).await?;
    match request {
        Request::Snapshot => {
            let threads = client
                .threads()
                .await
                .map_err(err)?
                .into_iter()
                .map(|t| ThreadEntry {
                    id: t.id,
                    channel: t.channel,
                    to: t.to,
                    title: t.title,
                    from: t.from,
                    created_at: t
                        .reactions
                        .iter()
                        .map(|r| r.data.created_at)
                        .fold(t.created_at, u64::max),
                    summary: t.summary,
                    mentioned: t.mentioned,
                    unread: t.unread,
                    own_agent_activity: t.own_agent_activity,
                    latest_seq: t.latest_seq,
                    reactions: reactions(&client, &t.reactions),
                    reaction_summary: summary(&client, &t.reactions),
                })
                .collect();
            serde_json::to_value(Snapshot {
                profile: ProfileView::from(&client.profile),
                threads,
                channels: client
                    .channels
                    .iter()
                    .map(|c| ChannelView {
                        id: c.channel.id,
                        name: client.channel_name(c.channel.id),
                        kind: c.channel.kind,
                    })
                    .collect(),
                retention_days: client.settings.retention_days,
                summary_mode: client.settings.summary_mode,
                summary_every: client.settings.summary_every,
                members: client.names(),
                devices: client
                    .devices
                    .iter()
                    .map(|d| DeviceView {
                        id: d.cert.data.device_id,
                        person_id: d.cert.data.person_id,
                        kind: d.cert.data.kind,
                        revoked_at: d.revoked_at,
                        last_seen_at: d.last_seen_at,
                    })
                    .collect(),
            })
            .map_err(err)
        }
        Request::Read { thread } => {
            let view = client
                .read(
                    thread,
                    ReadOptions {
                        all: true,
                        last: None,
                    },
                )
                .await
                .map_err(err)?;
            let mut messages = Vec::new();
            let mut preview_budget = 20 * 1024 * 1024;
            for message in view.messages {
                let mut attachments = Vec::new();
                for (attachment, path) in
                    message.content.attachments.iter().zip(&message.attachments)
                {
                    let preview = if message.verification_error.is_none()
                        && matches!(
                            attachment.mime.as_str(),
                            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
                        )
                        && attachment.size <= 5 * 1024 * 1024
                        && attachment.size <= preview_budget
                    {
                        let bytes = std::fs::read(path).map_err(err)?;
                        preview_budget -= attachment.size;
                        Some(format!(
                            "data:{};base64,{}",
                            attachment.mime,
                            STANDARD.encode(bytes)
                        ))
                    } else {
                        None
                    };
                    attachments.push(AttachmentView {
                        name: attachment.name.clone(),
                        mime: attachment.mime.clone(),
                        size: attachment.size,
                        path: path.to_string_lossy().into(),
                        preview,
                    });
                }
                messages.push(MessageView {
                    id: message.message.id,
                    number: message.number,
                    author: client.person_name(message.message.author_person),
                    person_id: message.message.author_person,
                    handle: message.message.envelope.handle.to_string(),
                    kind: message.author_kind,
                    created_at: message.message.created_at,
                    body: message.content.body,
                    verification_error: message.verification_error,
                    reactions: reactions(&client, &message.reactions),
                    reaction_summary: summary(&client, &message.reactions),
                    attachments,
                });
            }
            serde_json::to_value(ThreadView {
                id: view.id,
                title: view.title,
                participants: view.participants,
                summary: view.summary,
                summary_number: view.summary_number,
                newer_messages: view.newer_messages,
                messages,
            })
            .map_err(err)
        }
        Request::Send {
            thread,
            to,
            title,
            body,
        } => {
            if body.trim().is_empty() {
                return Err("write a message first".into());
            }
            let options = SendOptions {
                body,
                ..Default::default()
            };
            let message = if let Some(thread) = thread {
                client.reply(thread, options).await
            } else {
                client.send(&to, &title, options).await
            }
            .map_err(err)?;
            serde_json::to_value(message.envelope.thread_id.unwrap_or(message.id)).map_err(err)
        }
        Request::React {
            message,
            reaction,
            remove,
        } => {
            client.react(message, reaction, remove).await.map_err(err)?;
            Ok(serde_json::Value::Null)
        }
        Request::Unread { thread } => {
            client.unread(thread).await.map_err(err)?;
            Ok(serde_json::Value::Null)
        }
        Request::Settings {
            retention_days,
            summary_mode,
            summary_every,
        } => {
            if summary_every == 0 {
                return Err("summary_every must be a positive integer".into());
            }
            if client.settings.retention_days != retention_days {
                client
                    .set_settings("retention_days", &retention_days.to_string())
                    .await
                    .map_err(err)?;
            }
            if client.settings.summary_every != summary_every {
                client
                    .set_settings("summary_every", &summary_every.to_string())
                    .await
                    .map_err(err)?;
            }
            if client.settings.summary_mode != summary_mode {
                client
                    .set_settings(
                        "summary_mode",
                        match summary_mode {
                            SummaryMode::Required => "required",
                            SummaryMode::Off => "off",
                        },
                    )
                    .await
                    .map_err(err)?;
            }
            Ok(serde_json::Value::Null)
        }
        Request::DisplayName { name } => {
            client.set_display_name(&name).await.map_err(err)?;
            Ok(serde_json::Value::Null)
        }
        Request::Revoke { target } => {
            client.revoke(target).await.map_err(err)?;
            Ok(serde_json::Value::Null)
        }
        Request::Grant { enroll, recovery } => {
            let link = client.invite(enroll, recovery).await.map_err(err)?;
            Ok(serde_json::Value::String(link.to_string()))
        }
    }
}
#[derive(Serialize)]
pub struct Notice {
    mentioned: bool,
    direct: bool,
}
#[derive(Serialize)]
pub struct PollView {
    changed: bool,
    errors: Vec<String>,
    notices: Vec<Notice>,
}
#[tauri::command]
pub async fn poll(
    state: State<'_, AppState>,
    profile: String,
    initial: bool,
) -> Result<PollView, String> {
    // A separate core client keeps long-polling from blocking the human composer.
    let mut client = open(&state.0, &profile).await?;
    let events = client
        .wait(if initial { 0 } else { 60 })
        .await
        .map_err(err)?;
    let notices = events
        .iter()
        .filter_map(|event| match event {
            WaitEntry::Message(entry) => {
                let own = entry.from.starts_with(&format!(
                    "{}/",
                    client.person_name(client.profile.person_id)
                ));
                Some(Notice {
                    mentioned: entry.mentioned && (!own || entry.own_agent_activity),
                    direct: !own
                        && entry.channel.starts_with('@')
                        && !entry.channel.eq(&format!(
                            "@{}",
                            client.person_name(client.profile.person_id)
                        )),
                })
            }
            _ => None,
        })
        .filter(|notice| notice.mentioned || notice.direct)
        .collect();
    let errors = events
        .iter()
        .filter_map(|event| match event {
            WaitEntry::VerificationFailure { seq, description } => {
                Some(format!("Event {seq}: {description}"))
            }
            _ => None,
        })
        .collect();
    Ok(PollView {
        changed: !events.is_empty(),
        errors,
        notices,
    })
}
