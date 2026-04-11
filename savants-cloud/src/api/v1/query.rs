use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::middleware::AuthUser;
use crate::AppState;

#[derive(Deserialize)]
pub struct QueryRequest {
    pub query: String,
    pub params: Option<serde_json::Value>,
    pub graph: Option<String>,
}

#[derive(Serialize)]
pub struct QueryResponse {
    pub results: serde_json::Value,
}

pub async fn run_query(
    State(_state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, StatusCode> {
    tracing::info!(
        user_id = %auth.user_id,
        org_id = %auth.org_id,
        query = %body.query,
        "running graph query"
    );

    // TODO: execute body.query against the org's FalkorDB graph
    // with body.params and return real results
    Ok(Json(QueryResponse {
        results: serde_json::json!([]),
    }))
}
