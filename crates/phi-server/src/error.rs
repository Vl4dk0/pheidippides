use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use phi_proto::ErrorBody;

pub struct ApiError(pub ErrorBody);
pub type Result<T> = std::result::Result<T, ApiError>;
impl ApiError {
    pub fn invalid(message: impl ToString) -> Self {
        Self(ErrorBody::Invalid {
            message: message.to_string(),
            summary_required: None,
        })
    }
    pub fn summary_required(bytes_since_summary: u64, threshold: u32) -> Self {
        Self(ErrorBody::Invalid {
            message: format!(
                "--summary is required now: {bytes_since_summary} messages since the last summary (including this message; threshold {threshold}) and summary_mode=required"
            ),
            summary_required: Some(phi_proto::SummaryRequirement {
                bytes_since_summary,
                threshold,
            }),
        })
    }
    pub fn forbidden() -> Self {
        Self(ErrorBody::Forbidden {
            message: "access denied".into(),
        })
    }
    pub fn missing() -> Self {
        Self(ErrorBody::NotFound {
            message: "resource not found".into(),
        })
    }
    pub fn conflict(message: impl ToString) -> Self {
        Self(ErrorBody::Conflict {
            message: message.to_string(),
            claim: None,
        })
    }
    pub fn unauthorized() -> Self {
        Self(ErrorBody::Unauthorized {
            message: "invalid or revoked bearer token".into(),
        })
    }
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "server operation failed");
        Self(ErrorBody::Internal {
            message: "storage operation failed".into(),
        })
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry = if let ErrorBody::RateLimited { retry_after } = &self.0 {
            Some(*retry_after)
        } else {
            None
        };
        let mut response = (
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(self.0),
        )
            .into_response();
        if let Some(seconds) = retry.and_then(|v| v.to_string().parse().ok()) {
            response.headers_mut().insert(header::RETRY_AFTER, seconds);
        }
        response
    }
}
impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(ref e, _) = error {
            if e.code == rusqlite::ErrorCode::ConstraintViolation {
                return Self::conflict("duplicate or inconsistent resource");
            }
        }
        Self::internal(error)
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::internal(e)
    }
}
impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        Self::internal(e)
    }
}
impl From<phi_crypto::CryptoError> for ApiError {
    fn from(e: phi_crypto::CryptoError) -> Self {
        Self::invalid(e)
    }
}
impl From<phi_proto::ValidationError> for ApiError {
    fn from(e: phi_proto::ValidationError) -> Self {
        Self::invalid(e)
    }
}
