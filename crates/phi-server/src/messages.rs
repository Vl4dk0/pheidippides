use crate::{
    db::{Auth, Db, json, now},
    error::{ApiError, Result},
};
use phi_proto::*;
use rusqlite::{OptionalExtension, named_params, params};

// Notification routing shares one predicate so counts and long-polls agree without
// changing person-wide read cursors or restricting access to other conversations.
const NOTIFICATION_MESSAGE: &str = "
    (:handle IS NULL OR m.author_person<>:person OR EXISTS (
        SELECT 1 FROM message_mentions direct WHERE direct.message_id=m.id
        AND direct.person_id=:person AND direct.handle=:handle
    ))
";

impl Db {
    pub fn send(&mut self, a: &Auth, cid: ChannelId, r: SendMessageRequest) -> Result<Message> {
        r.validate()?;
        let c = self.channel(a, cid)?;
        let e = &r.envelope;
        if e.space_id != a.sid() || e.channel_id != cid || e.epoch != c.current_epoch {
            return Err(ApiError::invalid(
                "message space/channel/current epoch mismatch",
            ));
        }
        if e.mentions.iter().any(|m| !c.members.contains(&m.person_id)) {
            return Err(ApiError::invalid("mention is outside channel"));
        }
        phi_crypto::verify_message_signature(&a.cert.data.sign_pub, e, &r.signature)?;
        self.transaction(|db| {
            let thread = if let Some(tid) = e.thread_id {
                let root = db.message(a, tid)?;
                if root.envelope.thread_id.is_some() || root.envelope.channel_id != cid {
                    return Err(ApiError::invalid("thread must be a root in this channel"));
                }
                tid
            } else {
                db.conn.execute(
                    "INSERT INTO threads VALUES(?,?,?,?)",
                    params![
                        r.id.to_string(),
                        a.sid().to_string(),
                        cid.to_string(),
                        now()
                    ],
                )?;
                r.id
            };
            let settings = db.space(a.sid())?.settings;
            if settings.summary_mode == SummaryMode::Required
                && a.cert.data.kind == DeviceKind::Cli
                && !r.has_summary
            {
                // Count at commit so simultaneous replies cannot both bypass the threshold.
                let since_summary: u64 = db.conn.query_row(
                    "SELECT COUNT(*)+1 FROM messages WHERE thread_id=?1 AND seq>COALESCE(
                        (SELECT MAX(seq) FROM messages WHERE thread_id=?1
                        AND json_extract(json,'$.has_summary')=1),0)",
                    [thread.to_string()],
                    |row| row.get(0),
                )?;
                let summary_every = settings.summary_every;
                if since_summary >= u64::from(summary_every) {
                    return Err(ApiError::summary_required(since_summary, summary_every));
                }
            }
            let latest: u64 = db.conn.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM messages WHERE thread_id=?",
                [thread.to_string()],
                |row| row.get(0),
            )?;
            if e.summary_covers_seq
                .is_some_and(|coverage| coverage > latest)
            {
                return Err(ApiError::invalid(
                    "summary coverage exceeds existing thread history",
                ));
            }
            let seq = db.seq(a.sid())?;
            let message = Message {
                id: r.id,
                envelope: r.envelope.clone(),
                author_person: a.person(),
                author_device: a.device(),
                signature: r.signature,
                seq,
                created_at: now(),
                has_summary: r.has_summary,
                summary_covers_seq: e.summary_covers_seq,
            };
            db.conn.execute(
                "INSERT INTO messages VALUES(?,?,?,?,?,?,?)",
                params![
                    message.id.to_string(),
                    a.sid().to_string(),
                    cid.to_string(),
                    thread.to_string(),
                    seq,
                    a.person().to_string(),
                    json(&message)?
                ],
            )?;
            for mention in &message.envelope.mentions {
                db.conn.execute(
                    "INSERT INTO message_mentions VALUES(?,?,?)",
                    params![
                        message.id.to_string(),
                        mention.person_id.to_string(),
                        mention.handle.as_ref().map_or("",Handle::as_str)
                    ],
                )?;
            }
            // Blob IDs are routing metadata; plaintext attachment keys never reach the host.
            for id in &r.blob_ids {
                let blob: Blob = db.one(
                    "SELECT json FROM blobs WHERE id=? AND space_id=?",
                    params![id.to_string(), a.sid().to_string()],
                )?;
                let permitted: bool = db.conn.query_row(
                    "
            SELECT uploader=? OR EXISTS(SELECT 1 FROM blob_refs br JOIN threads t
            ON t.id=br.thread_id JOIN members cm ON cm.channel_id=t.channel_id
            WHERE br.blob_id=blobs.id AND cm.person_id=?) FROM blobs WHERE id=?
        ",
                    params![
                        a.device().to_string(),
                        a.person().to_string(),
                        id.to_string()
                    ],
                    |r| r.get(0),
                )?;
                if !permitted {
                    return Err(ApiError::forbidden());
                }
                db.conn.execute(
                    "INSERT OR IGNORE INTO blob_refs VALUES(?,?)",
                    params![blob.id.to_string(), thread.to_string()],
                )?;
            }
            db.conn.execute(
                "UPDATE threads SET activity=? WHERE id=?",
                params![now(), thread.to_string()],
            )?;
            // Self-channel recipients are other sessions of the same person; sending must leave their inbox unread.
            if c.kind != ChannelKind::SelfChannel {
                db.conn.execute(
                    "INSERT INTO cursors VALUES(?,?,?) ON CONFLICT(person_id,thread_id) DO UPDATE SET seq=MAX(seq,excluded.seq)",
                    params![a.person().to_string(), thread.to_string(), seq],
                )?;
            }
            db.event(
                a.sid(),
                seq,
                EventData::Message(message.clone()),
                Some(cid),
                None,
                Some(thread),
            )?;
            Ok(message)
        })
    }
    pub fn messages(
        &self,
        a: &Auth,
        cid: ChannelId,
        q: MessagesQuery,
    ) -> Result<ListMessagesResponse> {
        q.validate()?;
        self.channel(a, cid)?;
        if let Some(t) = q.thread {
            let m = self.message(a, t)?;
            if m.envelope.channel_id != cid || m.envelope.thread_id.is_some() {
                return Err(ApiError::invalid("invalid thread"));
            }
        }
        let messages: Vec<Message> = self.list(
            "
            SELECT json FROM messages WHERE channel_id=? AND (? IS NULL OR
            thread_id=?) AND seq>? ORDER BY seq LIMIT ?
        ",
            params![
                cid.to_string(),
                q.thread.map(|t| t.to_string()),
                q.thread.map(|t| t.to_string()),
                q.after.unwrap_or(0).min(i64::MAX as u64),
                q.limit.unwrap_or(100)
            ],
        )?;
        let next_after = messages.last().map(|m| m.seq);
        let mut reactions = Vec::new();
        let mut cursors = Vec::new();
        let mut activities = Vec::new();
        let mut threads = std::collections::BTreeSet::new();
        for message in &messages {
            reactions.extend(self.list::<Reaction>(
                "SELECT json FROM reactions WHERE message_id=?",
                [message.id.to_string()],
            )?);
            let thread = message.envelope.thread_id.unwrap_or(message.id);
            if threads.insert(thread) {
                let (last_activity, last_activity_seq) = self.conn.query_row(
                    "SELECT activity,COALESCE((SELECT MAX(seq) FROM events WHERE thread_id=?1),0) FROM threads WHERE id=?1",
                    [thread.to_string()], |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                activities.push(ThreadActivity {
                    thread_id: thread,
                    last_activity,
                    last_activity_seq,
                });
                let seq: u64 = self.conn.query_row(
                    "SELECT COALESCE((SELECT seq FROM cursors WHERE person_id=? AND thread_id=?),0)",
                    params![a.person().to_string(), thread.to_string()], |row| row.get(0),
                )?;
                cursors.push(ReadCursor {
                    person_id: a.person(),
                    thread_id: thread,
                    last_read_seq: seq,
                });
            }
        }
        Ok(ListMessagesResponse {
            messages,
            reactions,
            cursors,
            activities,
            next_after,
        })
    }
    pub fn reaction(
        &mut self,
        a: &Auth,
        mid: MessageId,
        r: ReactionRequest,
        delete: Option<ReactionKind>,
    ) -> Result<Reaction> {
        r.validate()?;
        let reaction = r.reaction;
        let d = &reaction.data;
        let message = self.message(a, mid)?;
        if d.space_id != a.sid()
            || d.message_id != mid
            || d.person_id != a.person()
            || d.device_id != a.device()
            || d.remove != delete.is_some()
            || delete.is_some_and(|k| k != d.kind)
            || d.created_at > now()
            || d.created_at < a.cert.data.created_at
        {
            return Err(ApiError::invalid("reaction identity/action/time mismatch"));
        }
        self.verify(
            a,
            &phi_crypto::reaction_signing_bytes(d)?,
            &reaction.signature,
        )?;
        let kind = json(&d.kind)?.trim_matches('"').to_owned();
        self.transaction(|db| {
            if d.remove {
                let n = if d.kind == ReactionKind::Claim {
                    db.conn.execute(
                        "
            DELETE FROM reactions WHERE message_id=? AND person_id=? AND
            kind='claim'
        ",
                        params![mid.to_string(), a.person().to_string()],
                    )?
                } else {
                    db.conn.execute(
                        "
            DELETE FROM reactions WHERE message_id=? AND person_id=? AND handle=?
            AND kind=?
        ",
                        params![
                            mid.to_string(),
                            a.person().to_string(),
                            d.handle.as_str(),
                            kind
                        ],
                    )?
                };
                if n == 0 {
                    return Err(ApiError::conflict(
                        "reaction does not belong to you or does not exist",
                    ));
                }
            } else {
                if d.kind == ReactionKind::Claim {
                    let existing: Option<String> = db.conn.query_row(
                        "SELECT json FROM reactions WHERE message_id=? AND kind='claim'",
                        [mid.to_string()], |row| row.get(0),
                    ).optional()?;
                    if let Some(existing) = existing {
                        return Err(ApiError(ErrorBody::Conflict {
                            message: format!("message {mid} is already claimed"),
                            claim: Some(Box::new(serde_json::from_str(&existing)?)),
                        }));
                    }
                } else {
                    let exists: bool = db.conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM reactions WHERE message_id=? AND person_id=? AND handle=? AND kind=?)",
                        params![mid.to_string(), a.person().to_string(), d.handle.as_str(), kind],
                        |row| row.get(0),
                    )?;
                    if exists {
                        return Err(ApiError::conflict(format!("{kind} reaction already exists on message {mid} for handle {}", d.handle)));
                    }
                }
                db.conn.execute(
                    "INSERT INTO reactions VALUES(?,?,?,?,?)",
                    params![
                        mid.to_string(),
                        a.person().to_string(),
                        d.handle.as_str(),
                        kind,
                        json(&reaction)?
                    ],
                )?;
            }
            let tid = message.envelope.thread_id.unwrap_or(message.id);
            db.conn.execute(
                "UPDATE threads SET activity=? WHERE id=?",
                params![now(), tid.to_string()],
            )?;
            let seq = db.seq(a.sid())?;
            db.event(
                a.sid(),
                seq,
                EventData::Reaction(reaction.clone()),
                Some(message.envelope.channel_id),
                Some(message.author_person),
                Some(tid),
            )?;
            Ok(reaction.clone())
        })
    }
    pub fn cursor(&self, a: &Auth, tid: MessageId, r: PutCursorRequest) -> Result<ReadCursor> {
        r.validate()?;
        let root = self.message(a, tid)?;
        if root.envelope.thread_id.is_some() {
            return Err(ApiError::invalid("cursor requires a thread root"));
        }
        let latest: u64 = self.conn.query_row(
            "SELECT MAX(seq) FROM messages WHERE thread_id=?",
            [tid.to_string()],
            |r| r.get(0),
        )?;
        if r.last_read_seq > latest {
            return Err(ApiError::invalid("cursor beyond latest thread message"));
        }
        self.conn.execute(
            "
            INSERT INTO cursors VALUES(?,?,?) ON CONFLICT(person_id,thread_id) DO
            UPDATE SET seq=excluded.seq
        ",
            params![a.person().to_string(), tid.to_string(), r.last_read_seq],
        )?;
        Ok(ReadCursor {
            person_id: a.person(),
            thread_id: tid,
            last_read_seq: r.last_read_seq,
        })
    }
    pub fn counts(&self, a: &Auth, q: CountsQuery) -> Result<Counts> {
        q.validate()?;
        let mut counts = Counts {
            latest_seq: self.latest(a.sid())?,
            ..Counts::default()
        };
        let mut statement = self.conn.prepare(&format!(
            "
            SELECT m.channel_id,COUNT(DISTINCT m.thread_id),COALESCE(SUM((SELECT
            COUNT(*) FROM message_mentions mm WHERE mm.message_id=m.id
            AND mm.person_id=:person AND (:handle IS NULL OR mm.handle='' OR mm.handle=:handle))),0) FROM messages m JOIN members
            cm ON cm.channel_id=m.channel_id LEFT JOIN cursors c ON
            c.thread_id=m.thread_id AND c.person_id=cm.person_id WHERE
            cm.person_id=:person AND m.space_id=:space AND m.seq>COALESCE(c.seq,0)
            AND {NOTIFICATION_MESSAGE} GROUP BY
            m.channel_id
        ",
        ))?;
        let rows = statement.query_map(
            named_params! {
                ":person": a.person().to_string(),
                ":handle": q.handle.as_ref().map(|h| h.as_str()),
                ":space": a.sid().to_string(),
            },
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, u64>(1)?,
                    r.get::<_, u64>(2)?,
                ))
            },
        )?;
        for row in rows {
            let (cid, unread, mentions) = row?;
            counts.unread_threads += unread;
            counts.mentions += mentions;
            counts.channels.push(ChannelCounts {
                channel_id: cid.parse()?,
                unread_threads: unread,
                mentions,
            });
        }
        counts.claimed_by_you = self.conn.query_row(
            "
            SELECT COUNT(*) FROM reactions r JOIN messages m ON m.id=r.message_id
            JOIN members cm ON cm.channel_id=m.channel_id WHERE cm.person_id=? AND
            m.space_id=? AND r.person_id=? AND r.kind='claim'
        ",
            params![
                a.person().to_string(),
                a.sid().to_string(),
                a.person().to_string()
            ],
            |r| r.get(0),
        )?;
        Ok(counts)
    }
    pub fn events(&self, a: &Auth, q: &EventsQuery) -> Result<EventsResponse> {
        q.validate()?;
        let events: Vec<Event> = self.list(
            &format!(
                "
            SELECT e.json FROM events e LEFT JOIN messages m
            ON m.space_id=e.space_id AND m.seq=e.seq
            WHERE e.space_id=:space AND e.seq>:after AND
            ((e.channel_id IS NULL) OR EXISTS(SELECT 1 FROM members m WHERE
            m.channel_id=e.channel_id AND m.person_id=:person))
            AND (m.id IS NULL OR {NOTIFICATION_MESSAGE}) ORDER BY e.seq LIMIT 1000
        "
            ),
            named_params! {
                ":space": a.sid().to_string(),
                ":after": q.after.min(i64::MAX as u64),
                ":person": a.person().to_string(),
                ":handle": q.handle.as_ref().map(|h| h.as_str()),
            },
        )?;
        let next_after = events.last().map_or(q.after, |event| event.seq);
        Ok(EventsResponse { events, next_after })
    }
}
