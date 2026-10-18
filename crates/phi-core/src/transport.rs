use crate::{Result, user};
use phi_proto::{API_PREFIX, ErrorBody, PROTOCOL_HEADER, PROTOCOL_MAJOR, Validate};
use reqwest::{Client, Method};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    User(String),
    #[error("server HTTP {status}: {message}")]
    Server { status: u16, message: String },
    #[error("authentication failed or device revoked; enroll this device again")]
    Auth,
    #[error("phi {client_major}.x can't talk to a {0}.x server — upgrade phi", client_major = PROTOCOL_MAJOR)]
    Upgrade(u16),
    #[error("rate limited; retry after {0} seconds")]
    RateLimited(u64),
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Crypto(#[from] phi_crypto::CryptoError),
    #[error(transparent)]
    Validation(#[from] phi_proto::ValidationError),
    #[error("local storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("local cache: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid stored or server JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid profile TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("profile serialization: {0}")]
    TomlWrite(#[from] toml::ser::Error),
}
impl Error {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Auth => 3,
            Self::Upgrade(_) => 4,
            Self::Network(_) | Self::RateLimited(_) | Self::Server { status: 500.., .. } => 2,
            _ => 1,
        }
    }
}

pub(crate) struct Http {
    client: Client,
    pub(crate) base: String,
    token: Option<String>,
}
impl Http {
    pub(crate) fn new(base: &str, token: Option<String>) -> Result<Self> {
        let parsed = reqwest::Url::parse(base)
            .map_err(|_| user("invalid server URL; use http:// or https://"))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(user(
                "server must be an http:// or https:// origin without credentials or path",
            ));
        }
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(930))
                .build()?,
            base: base.trim_end_matches('/').into(),
            token,
        })
    }
    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request::<(), T>(Method::GET, path, None).await
    }
    pub(crate) async fn send<B: Serialize + Validate, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T> {
        body.validate()?;
        self.request(method, path, Some(body)).await
    }
    async fn request<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let mut req = self
            .client
            .request(method, format!("{}{API_PREFIX}{path}", self.base))
            .header(PROTOCOL_HEADER, PROTOCOL_MAJOR);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let response = req.send().await?;
        let status = response.status();
        let retry = response
            .headers()
            .get("retry-after")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.parse().ok());
        let bytes = response.bytes().await?;
        if !status.is_success() {
            let error = serde_json::from_slice::<ErrorBody>(&bytes).ok();
            return Err(match (status.as_u16(), error) {
                (401, _) => Error::Auth,
                (426, Some(ErrorBody::Upgrade { required_major })) => {
                    Error::Upgrade(required_major)
                }
                (429, Some(ErrorBody::RateLimited { retry_after })) => {
                    Error::RateLimited(retry.unwrap_or(retry_after))
                }
                (429, _) => Error::RateLimited(retry.unwrap_or(1)),
                (_, Some(e)) => Error::Server {
                    status: status.as_u16(),
                    message: match e {
                        ErrorBody::Invalid { message }
                        | ErrorBody::Unauthorized { message }
                        | ErrorBody::Forbidden { message }
                        | ErrorBody::NotFound { message }
                        | ErrorBody::Conflict { message }
                        | ErrorBody::TooLarge { message }
                        | ErrorBody::Internal { message } => message,
                        _ => "unexpected response".into(),
                    },
                },
                _ => Error::Server {
                    status: status.as_u16(),
                    message: "invalid error response".into(),
                },
            });
        }
        serde_json::from_slice(&bytes).map_err(|error| Error::Server {
            status: 502,
            message: format!("invalid server JSON: {error}"),
        })
    }
}
pub(crate) fn token_string(token: &phi_proto::Base64<32>) -> Result<String> {
    Ok(serde_json::to_value(token)?
        .as_str()
        .ok_or_else(|| user("invalid token"))?
        .to_owned())
}
