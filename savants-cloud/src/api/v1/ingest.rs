use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::middleware::AuthUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct DeltaPayload {
    pub graph: Option<String>,
    pub nodes: Option<serde_json::Value>,
    pub edges: Option<serde_json::Value>,
    pub deletes: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct IngestResponse {
    pub status: String,
    pub accepted: bool,
}

pub async fn push_delta(
    State(_state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(_body): Json<DeltaPayload>,
) -> Result<Json<IngestResponse>, StatusCode> {
    tracing::info!(
        user_id = %auth.user_id,
        org_id = %auth.org_id,
        "accepted delta ingest"
    );

    // TODO: push delta into the org's FalkorDB graph
    Ok(Json(IngestResponse {
        status: "accepted".to_string(),
        accepted: true,
    }))
}
