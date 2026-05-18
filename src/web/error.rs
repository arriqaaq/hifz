//! HTTP error contract.
//!
//! Handlers now return `ApiResult` (= `Result<Json<Value>, ApiError>`) instead
//! of unconditionally `Json<Value>`. An `ApiError` serializes to the same
//! `{"error": "..."}` body the API has always used, but with a *real* HTTP
//! status code instead of a fake 200:
//!
//! - a `crate::error::HifzError::NotFound` (recovered from `anyhow` by
//!   `downcast_ref` — classification by type, never by string) → 404
//! - `HifzError::InvalidInput` → 400
//! - anything else → 500
//!
//! `AppJson<T>` replaces `axum::Json<T>` as the request-body extractor so a
//! body-deserialization failure returns `{"error": "..."}` JSON (not Axum's
//! default plain-text 422) — this is the direct fix for the original
//! `hifz_save` "error decoding response body" symptom.

use axum::extract::FromRequest;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};

use crate::error::HifzError;

/// An HTTP error: a status code plus a human-readable message. Renders as
/// `(status, Json({"error": message}))`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

/// Every handler's return type. `Ok` is the legacy JSON body; `Err` carries a
/// real status code.
pub type ApiResult = Result<Json<serde_json::Value>, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        // Type-based classification: recover the concrete HifzError if the
        // library wrapped one inside anyhow. No string matching.
        if let Some(he) = e.downcast_ref::<HifzError>() {
            let status = match he {
                HifzError::NotFound(_) => StatusCode::NOT_FOUND,
                HifzError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            };
            return ApiError {
                status,
                message: he.to_string(),
            };
        }
        ApiError {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    }
}

impl From<HifzError> for ApiError {
    fn from(he: HifzError) -> Self {
        let status = match he {
            HifzError::NotFound(_) => StatusCode::NOT_FOUND,
            HifzError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        };
        ApiError {
            status,
            message: he.to_string(),
        }
    }
}

/// Drop-in replacement for `axum::Json<T>` as a request-body extractor. On a
/// deserialization rejection it preserves Axum's status (400/422) but replaces
/// the plain-text body with the canonical `{"error": "..."}` JSON envelope.
pub struct AppJson<T>(pub T);

impl<S, T> FromRequest<S> for AppJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let axum::Json(value) =
            axum::Json::<T>::from_request(req, state)
                .await
                .map_err(|rej: JsonRejection| ApiError {
                    status: rej.status(),
                    message: rej.body_text(),
                })?;
        Ok(AppJson(value))
    }
}
