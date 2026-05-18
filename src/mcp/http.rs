//! Shared HTTP-response handling for the MCP→REST proxy.
//!
//! Before this module every proxy call site did
//! `…send().await?.json().await?`, which:
//!   - never checked the HTTP status, and
//!   - on any non-JSON body (e.g. Axum's old plain-text 422 extractor
//!     rejection) failed with reqwest's opaque "error decoding response
//!     body", surfaced to the MCP client as a bare `-32603`, and
//!   - silently passed a `{"error": …}` envelope through as a fake success.
//!
//! `RequestBuilderExt::send_decode` replaces that tail with one helper that
//! reports the real status + body, and a typed `ProxyError` the dispatch
//! layer maps to a correct JSON-RPC code.

use std::future::Future;

/// A proxied REST call that did not yield a usable JSON success body.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// REST replied with a non-2xx status. `body` is the (trimmed) response
    /// text — for the canonical `{"error": …}` envelope or Axum's extractor
    /// rejection this is the actual, human-readable reason.
    #[error("hifz REST {status}: {body}")]
    Http { status: u16, body: String },

    /// 2xx but the body was not valid JSON. Carries the raw body so the cause
    /// is legible instead of reqwest's opaque "error decoding response body".
    #[error("hifz REST returned a non-JSON body: {body}")]
    Decode { body: String },

    /// 2xx, valid JSON, but a top-level `{"error": "<msg>"}` envelope — a
    /// failure the old code passed through as a successful tool result.
    #[error("{0}")]
    Envelope(String),

    /// Could not reach / read from the REST server (connect, timeout, reset).
    #[error("transport error talking to hifz REST: {0}")]
    Transport(String),

    /// A required tool argument was missing/invalid (caught before the call).
    #[error("invalid arguments: {0}")]
    BadArgs(String),
}

impl ProxyError {
    fn from_reqwest(e: reqwest::Error) -> Self {
        ProxyError::Transport(e.to_string())
    }
}

/// Read a proxied response: status-check, then parse, then reject the
/// `{"error": …}` envelope. The single place response shape is trusted.
pub async fn decode_json(resp: reqwest::Response) -> Result<serde_json::Value, ProxyError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| ProxyError::Transport(format!("reading response body: {e}")))?;

    if !status.is_success() {
        return Err(ProxyError::Http {
            status: status.as_u16(),
            body: text.trim().to_string(),
        });
    }

    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| ProxyError::Decode { body: text.clone() })?;

    // Belt-and-suspenders: with the REST error-contract fix in place these
    // arrive as real 4xx/5xx (handled above), but a top-level string `error`
    // on a 2xx must never be reported as success.
    if let Some(msg) = value.get("error").and_then(|e| e.as_str()) {
        return Err(ProxyError::Envelope(msg.to_string()));
    }

    Ok(value)
}

/// Postfix extension so call sites only swap the `…send().await?.json()
/// .await?` tail for `…send_decode().await?` — no head-wrapping churn.
pub trait RequestBuilderExt {
    fn send_decode(self) -> impl Future<Output = Result<serde_json::Value, ProxyError>> + Send;
}

impl RequestBuilderExt for reqwest::RequestBuilder {
    async fn send_decode(self) -> Result<serde_json::Value, ProxyError> {
        let resp = self.send().await.map_err(ProxyError::from_reqwest)?;
        decode_json(resp).await
    }
}

/// JSON-RPC error code for a (possibly `ProxyError`) dispatch error.
/// 4xx / bad-args / `{"error"}`-envelope → `-32602` (Invalid params);
/// 5xx / decode / transport / non-proxy → `-32603` (Internal error).
pub fn rpc_code(err: &anyhow::Error) -> i64 {
    match err.downcast_ref::<ProxyError>() {
        Some(ProxyError::Http { status, .. }) if (400u16..500).contains(status) => -32602,
        Some(ProxyError::BadArgs(_)) | Some(ProxyError::Envelope(_)) => -32602,
        _ => -32603,
    }
}
