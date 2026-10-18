use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use phi_crypto::{DeviceKeys, certify_device, message_signing_bytes, sign_membership, space_id};
use tower::ServiceExt;

async fn request(
    server: &Server,
    method: &str,
    path: &str,
    token: &str,
    body: impl Serialize,
) -> anyhow::Result<serde_json::Value> {
    let response = server
        .router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("Phi-Protocol", PROTOCOL_MAJOR)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn wait_notifications_follow_event_commits_only() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let server = Server::open(Config {
        data_dir: dir.path().into(),
        allow_space_creation: config::Creation::Anyone,
        ..Config::default()
    })?;
    let keys = DeviceKeys::from_secrets([1; 32], [2; 32]);
    let sid = space_id(&keys.sign_public());
    let did = DeviceId::from_bytes([1; 16]);
    let pid = PersonId::from_bytes([1; 16]);
    let cid = ChannelId::from_bytes([1; 16]);
    let root = certify_device(
        DeviceCertificateData {
            space_id: sid,
            person_id: pid,
            device_id: did,
            kind: DeviceKind::Cli,
            sign_pub: keys.sign_public(),
            kex_pub: keys.kex_public(),
            created_at: db::now() - 10,
            issuer: CertificateIssuer::Device { device_id: did },
        },
        &keys,
    )?;
    let channel = |id, kind| -> anyhow::Result<CreateChannelRequest> {
        Ok(CreateChannelRequest {
            membership: sign_membership(
                ChannelMembershipData {
                    key_commitment: PublicKey::from_bytes([42; 32]),
                    previous: None,
                    space_id: sid,
                    channel_id: id,
                    epoch: 0,
                    kind,
                    members: vec![pid],
                    issuer_device: did,
                },
                &keys,
            )?,
            id,
            space_id: sid,
            kind,
            members: vec![pid],
            meta: Ciphertext {
                nonce: Nonce::from_bytes([3; 24]),
                ciphertext: Base64Bytes::new(vec![4; 16]),
            },
            wrapped_keys: vec![WrappedKey {
                channel_id: id,
                device_id: did,
                epoch: 0,
                sealed: SealedBox {
                    ephemeral_pub: keys.kex_public(),
                    nonce: Nonce::from_bytes([3; 24]),
                    ciphertext: Base64Bytes::new(vec![4; 48]),
                },
            }],
        })
    };
    let create = CreateSpaceRequest {
        root_cert: root,
        all_channel: channel(cid, ChannelKind::All)?,
        self_channel: channel(ChannelId::from_bytes([2; 16]), ChannelKind::SelfChannel)?,
    };
    let response = server
        .store
        .call(move |db| db.create_space(create, None))
        .await
        .map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
    let token = serde_json::to_string(&response.device_token)?
        .trim_matches('"')
        .to_owned();
    let mut watcher = server
        .watch(sid)
        .map_err(|e| anyhow::anyhow!("{:?}", e.0))?;
    request(
        &server,
        "POST",
        "/v1/blobs",
        &token,
        UploadBlobRequest {
            space_id: sid,
            ciphertext: Base64Bytes::new(vec![4; 32]),
        },
    )
    .await?;
    assert!(!watcher.has_changed()?);
    let envelope = MessageEnvelope {
        id: MessageId::from_bytes([3; 16]),
        summary_covers_seq: Some(0),
        space_id: sid,
        channel_id: cid,
        thread_id: None,
        epoch: 0,
        handle: "main".parse()?,
        mentions: vec![],
        nonce: Nonce::from_bytes([3; 24]),
        ciphertext: Base64Bytes::new(vec![4; 16]),
    };
    let signature = keys.sign(&message_signing_bytes(&envelope)?);
    let message: Message = serde_json::from_value(
        request(
            &server,
            "POST",
            &format!("/v1/channels/{cid}/messages"),
            &token,
            SendMessageRequest {
                id: envelope.id,
                envelope,
                signature,
                has_summary: true,
                blob_ids: vec![],
            },
        )
        .await?,
    )?;
    assert!(watcher.has_changed()?);
    watcher.borrow_and_update();
    request(
        &server,
        "PUT",
        &format!("/v1/threads/{}/cursor", message.id),
        &token,
        PutCursorRequest {
            last_read_seq: message.seq,
        },
    )
    .await?;
    assert!(!watcher.has_changed()?);
    request(
        &server,
        "GET",
        &format!("/v1/spaces/{sid}/counts"),
        &token,
        serde_json::Value::Null,
    )
    .await?;
    assert!(!watcher.has_changed()?);
    Ok(())
}
