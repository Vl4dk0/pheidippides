use super::{
    Server, blobs,
    db::{Auth, Db},
    error::{ApiError, Result},
};
use axum::{
    Extension, Json, Router,
    body::to_bytes,
    extract::{FromRequest, FromRequestParts, Path, Query, Request, State},
    http::{Method, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
};
use phi_proto::*;
use rusqlite::params;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Clone)]
struct Bearer(Option<String>);
struct Input<T>(T);
impl<T: DeserializeOwned + Validate + Send> FromRequest<Server> for Input<T> {
    type Rejection = ApiError;
    async fn from_request(request: Request, _: &Server) -> Result<Self> {
        let body = to_bytes(request.into_body(), 1024 * 1024)
            .await
            .map_err(|_| {
                ApiError(ErrorBody::TooLarge {
                    message: "request body limit exceeded".into(),
                })
            })?;
        let input: T = serde_json::from_slice(&body).map_err(ApiError::invalid)?;
        input.validate()?;
        Ok(Self(input))
    }
}
struct Id<T>(T);
impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for Id<T> {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(ApiError::invalid)
    }
}
struct Parameters<T>(T);
impl<S: Send + Sync, T: DeserializeOwned + Validate + Send> FromRequestParts<S> for Parameters<T> {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(ApiError::invalid)?;
        value.validate()?;
        Ok(Self(value))
    }
}

pub(super) fn router(server: Server) -> Router {
    let protected = Router::new()
        .route("/spaces/{sid}", get(space))
        .route("/spaces/{sid}/settings", patch(settings))
        .route("/spaces/{sid}/grants", post(grant))
        .route("/spaces/{sid}/devices", get(devices))
        .route("/spaces/{sid}/revocations", post(revoke))
        .route("/spaces/{sid}/channels", get(channels).post(create_channel))
        .route("/spaces/{sid}/counts", get(counts))
        .route("/spaces/{sid}/events", get(events))
        .route("/channels/{cid}", patch(update_channel))
        .route("/channels/{cid}/keys", put(keys))
        .route("/channels/{cid}/keys/mine", get(my_keys))
        .route(
            "/channels/{cid}/messages",
            get(messages).merge(post(send).route_layer(middleware::from_fn_with_state(
                server.clone(),
                admit_message,
            ))),
        )
        .route("/messages/{mid}", get(message))
        .route("/messages/{mid}/reactions", post(react))
        .route("/messages/{mid}/reactions/{kind}", delete(unreact))
        .route("/threads/{tid}/cursor", put(cursor))
        .route("/blobs", post(upload))
        .route("/blobs/{bid}", get(download))
        .route_layer(middleware::from_fn_with_state(server.clone(), authenticate));
    let api = protected
        .route(
            "/spaces",
            post(create_space).route_layer(middleware::from_fn_with_state(
                server.clone(),
                admit_creation,
            )),
        )
        .route("/grants/{gid}", get(get_grant))
        .route("/grants/{gid}/redeem", post(redeem));
    Router::new()
        .nest(API_PREFIX, api)
        .route("/healthz", get(health))
        .layer(middleware::from_fn(protocol))
        .with_state(server)
}
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok", "protocol_major": PROTOCOL_MAJOR}))
}
async fn protocol(mut request: Request, next: Next) -> Response {
    // Only infrastructure GET/HEAD probes bypass admission; all other requests keep it.
    if request.uri().path() == "/healthz" && matches!(*request.method(), Method::GET | Method::HEAD)
    {
        return next.run(request).await;
    }
    let major = request
        .headers()
        .get(PROTOCOL_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let result = major
        .ok_or(ErrorBody::Upgrade {
            required_major: PROTOCOL_MAJOR,
        })
        .and_then(check_protocol);
    if let Err(error) = result {
        return ApiError(error).into_response();
    }
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned);
    request.extensions_mut().insert(Bearer(token));
    next.run(request).await
}
async fn authenticate(State(server): State<Server>, request: Request, next: Next) -> Response {
    let token = request
        .extensions()
        .get::<Bearer>()
        .cloned()
        .unwrap_or(Bearer(None));
    match server
        .store
        .call(move |db| db.auth(token.0.as_deref()).map(|_| ()))
        .await
    {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}
async fn admit_creation(State(server): State<Server>, request: Request, next: Next) -> Response {
    let token = request
        .extensions()
        .get::<Bearer>()
        .cloned()
        .unwrap_or(Bearer(None));
    match server
        .store
        .call(move |db| db.authorize_creation(token.0.as_deref()))
        .await
    {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}
async fn admit_message(State(server): State<Server>, request: Request, next: Next) -> Response {
    let token = request
        .extensions()
        .get::<Bearer>()
        .cloned()
        .unwrap_or(Bearer(None));
    match server
        .store
        .call(move |db| db.rate_limit(&db.auth(token.0.as_deref())?))
        .await
    {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}
impl Server {
    async fn authorized<T: Serialize + Send + 'static>(
        &self,
        token: Bearer,
        action: impl FnOnce(&mut Db, &Auth) -> Result<T> + Send + 'static,
    ) -> Result<Json<T>> {
        let notifier = self.clone();
        self.store
            .call(move |db| {
                let auth = db.auth(token.0.as_deref())?;
                let before = db.latest(auth.sid())?;
                let response = action(db, &auth)?;
                db.touch(&auth)?;
                // Notification happens inside the committed DB task even if HTTP is cancelled.
                if db.latest(auth.sid())? > before {
                    notifier.notify(auth.sid())?;
                }
                Ok(Json(response))
            })
            .await
    }
}
async fn create_space(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Input(input): Input<CreateSpaceRequest>,
) -> Result<Json<CreateSpaceResponse>> {
    server
        .store
        .call(move |db| db.create_space(input, token.0.as_deref()).map(Json))
        .await
}
async fn get_grant(
    State(server): State<Server>,
    Id(id): Id<GrantId>,
) -> Result<Json<GetGrantResponse>> {
    server.store.call(move |db| db.grant(id).map(Json)).await
}
async fn redeem(
    State(server): State<Server>,
    Id(id): Id<GrantId>,
    Input(input): Input<RedeemGrantRequest>,
) -> Result<Json<RedeemGrantResponse>> {
    server
        .store
        .call(move |db| db.redeem(id, input).map(Json))
        .await
}
async fn space(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
) -> Result<Json<Space>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.space(id)
        })
        .await
}
async fn settings(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Input(input): Input<UpdateSettingsRequest>,
) -> Result<Json<SettingsChange>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.settings(auth, input)
        })
        .await
}
async fn grant(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Input(input): Input<CreateGrantRequest>,
) -> Result<Json<Grant>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.create_grant(auth, input)
        })
        .await
}
async fn revoke(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Input(input): Input<CreateRevocationRequest>,
) -> Result<Json<Revocation>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.revoke(auth, input)
        })
        .await
}
async fn devices(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
) -> Result<Json<ListDevicesResponse>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            Ok(ListDevicesResponse {
                persons: db.list(
                    "SELECT json FROM persons WHERE space_id=?",
                    [id.to_string()],
                )?,
                devices: db.list(
                    "SELECT json FROM devices WHERE space_id=?",
                    [id.to_string()],
                )?,
                directory: db.directory(id)?,
            })
        })
        .await
}
async fn channels(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
) -> Result<Json<ListChannelsResponse>> {
    server.authorized(token, move |db, auth| { auth.space(id)?; Ok(ListChannelsResponse { channels:db.list("SELECT c.json FROM channels c JOIN members m ON m.channel_id=c.id WHERE c.space_id=? AND m.person_id=? ORDER BY c.id", params![id.to_string(),auth.person().to_string()])? }) }).await
}
async fn create_channel(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Input(input): Input<CreateChannelRequest>,
) -> Result<Json<Channel>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.transaction(|db| db.create_channel(auth, input, false))
        })
        .await
}
async fn update_channel(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<ChannelId>,
    Input(input): Input<UpdateChannelRequest>,
) -> Result<Json<Channel>> {
    server
        .authorized(token, move |db, auth| db.update_channel(auth, id, input))
        .await
}
async fn keys(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<ChannelId>,
    Input(input): Input<PutKeysRequest>,
) -> Result<Json<GetKeysResponse>> {
    server
        .authorized(token, move |db, auth| db.put_keys(auth, id, input))
        .await
}
async fn my_keys(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<ChannelId>,
) -> Result<Json<GetKeysResponse>> {
    server
        .authorized(token, move |db, auth| db.keys(auth, id))
        .await
}
async fn send(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<ChannelId>,
    Input(input): Input<SendMessageRequest>,
) -> Result<Json<Message>> {
    server
        .authorized(token, move |db, auth| db.send(auth, id, input))
        .await
}
async fn messages(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<ChannelId>,
    Parameters(query): Parameters<MessagesQuery>,
) -> Result<Json<ListMessagesResponse>> {
    server
        .authorized(token, move |db, auth| db.messages(auth, id, query))
        .await
}
async fn message(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<MessageId>,
) -> Result<Json<Message>> {
    server
        .authorized(token, move |db, auth| db.message(auth, id))
        .await
}
async fn react(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<MessageId>,
    Input(input): Input<ReactionRequest>,
) -> Result<Json<Reaction>> {
    server
        .authorized(token, move |db, auth| db.reaction(auth, id, input, None))
        .await
}
async fn unreact(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id((id, kind)): Id<(MessageId, ReactionKind)>,
    Input(input): Input<ReactionRequest>,
) -> Result<Json<Reaction>> {
    server
        .authorized(token, move |db, auth| {
            db.reaction(auth, id, input, Some(kind))
        })
        .await
}
async fn cursor(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<MessageId>,
    Input(input): Input<PutCursorRequest>,
) -> Result<Json<ReadCursor>> {
    server
        .authorized(token, move |db, auth| db.cursor(auth, id, input))
        .await
}
async fn counts(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Parameters(query): Parameters<CountsQuery>,
) -> Result<Json<Counts>> {
    server
        .authorized(token, move |db, auth| {
            auth.space(id)?;
            db.counts(auth, query)
        })
        .await
}
async fn events(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<SpaceId>,
    Parameters(query): Parameters<EventsQuery>,
) -> Result<Json<EventsResponse>> {
    server.events(token.0, id, query).await.map(Json)
}
async fn upload(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    request: Request,
) -> Result<Json<Blob>> {
    // Admission and the semaphore precede body reads, bounding concurrent ciphertext buffers.
    let permit = server
        .uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(ErrorBody::RateLimited { retry_after: 1 }))?;
    let body = to_bytes(request.into_body(), server.max_body)
        .await
        .map_err(|_| {
            ApiError(ErrorBody::TooLarge {
                message: "request body limit exceeded".into(),
            })
        })?;
    let config = server.config.clone();
    let staged = tokio::task::spawn_blocking(move || {
        let input: UploadBlobRequest = serde_json::from_slice(&body).map_err(ApiError::invalid)?;
        blobs::StagedBlob::write(&config, input, permit)
    })
    .await
    .map_err(ApiError::internal)??;
    let (result, staged) = server
        .store
        .call(move |db| {
            let mut staged = staged;
            let result = (|| {
                let auth = db.auth(token.0.as_deref())?;
                db.touch(&auth)?;
                db.commit_blob(&auth, &staged.blob)?;
                staged.persist();
                Ok(Json(staged.blob.clone()))
            })();
            Ok((result, staged))
        })
        .await?;
    tokio::task::spawn_blocking(move || drop(staged))
        .await
        .map_err(ApiError::internal)?;
    result
}
async fn download(
    State(server): State<Server>,
    Extension(token): Extension<Bearer>,
    Id(id): Id<BlobId>,
) -> Result<Json<DownloadBlobResponse>> {
    let (blob, mut file) = server
        .store
        .call(move |db| {
            let auth = db.auth(token.0.as_deref())?;
            db.touch(&auth)?;
            let blob = db.blob(&auth, id)?;
            // Opening under the DB guard preserves in-flight downloads through retention unlink.
            let file =
                std::fs::File::open(db.config.data_dir.join("blobs").join(blob.id.to_string()))?;
            Ok((blob, file))
        })
        .await?;
    tokio::task::spawn_blocking(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes)?;
        Ok(Json(DownloadBlobResponse {
            blob,
            ciphertext: Base64Bytes::new(bytes),
        }))
    })
    .await
    .map_err(ApiError::internal)?
}
