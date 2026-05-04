use axum::{
    async_trait,
    extract::{FromRequestParts, Query},
    http::request::Parts,
};
use serde::Deserialize;

/// Supported response formats for content negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFormat {
    /// Server-rendered HTML for browsers.
    Html,
    /// JSON for API consumers.
    Json,
    /// Binary pack format for CLI downloads.
    Pack,
}

#[derive(Debug, Deserialize)]
struct FormatQuery {
    format: Option<String>,
}

/// Axum extractor that performs content negotiation.
/// 
/// The rule:
/// 1. `?format=` query parameter wins if present (`json`, `html`, `pack`).
/// 2. `Accept` header containing `application/vnd.codetours.pack+sqlite` wins Pack.
/// 3. `Accept` header containing `application/json` wins JSON.
/// 4. Everything else (`text/html`, `*/*`, missing) wins HTML.
pub struct Negotiated(pub ResponseFormat);

#[async_trait]
impl<S> FromRequestParts<S> for Negotiated
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // 1. Check ?format= query param for debugging/overrides
        if let Ok(Query(FormatQuery { format: Some(format) })) =
            Query::<FormatQuery>::from_request_parts(parts, state).await
        {
            match format.to_lowercase().as_str() {
                "json" => return Ok(Negotiated(ResponseFormat::Json)),
                "html" => return Ok(Negotiated(ResponseFormat::Html)),
                "pack" => return Ok(Negotiated(ResponseFormat::Pack)),
                _ => {}
            }
        }

        // 2. Check Accept header
        if let Some(accept) = parts.headers.get(axum::http::header::ACCEPT).and_then(|v| v.to_str().ok()) {
            if accept.contains("application/vnd.codetours.pack+sqlite") {
                return Ok(Negotiated(ResponseFormat::Pack));
            }
            if accept.contains("application/json") {
                return Ok(Negotiated(ResponseFormat::Json));
            }
        }

        // Default to HTML for browsers and everything else
        Ok(Negotiated(ResponseFormat::Html))
    }
}
