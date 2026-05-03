use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::OnceLock;

pub static BOOT_TIME: OnceLock<DateTime<Utc>> = OnceLock::new();

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub boot_time: String,
}

pub async fn handler() -> Json<HealthResponse> {
    let boot_time = BOOT_TIME.get().cloned().unwrap_or_else(Utc::now);
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        boot_time: boot_time.to_rfc3339(),
    })
}
