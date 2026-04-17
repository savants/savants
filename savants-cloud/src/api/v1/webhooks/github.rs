use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct GitHubWebhookPayload {
    pub action: Option<String>,
    pub pull_request: Option<GitHubPR>,
    pub repository: Option<GitHubRepo>,
}

#[derive(Deserialize)]
pub struct GitHubPR {
    pub number: i64,
    pub title: Option<String>,
    pub head: Option<GitHubRef>,
    pub base: Option<GitHubRef>,
}

#[derive(Deserialize)]
pub struct GitHubRef {
    #[serde(rename = "ref")]
    pub ref_name: Option<String>,
    pub sha: Option<String>,
}

#[derive(Deserialize)]
pub struct GitHubRepo {
    pub full_name: Option<String>,
}

pub async fn handle_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GitHubWebhookPayload>,
) -> Result<StatusCode, StatusCode> {
    let action = body.action.as_deref().unwrap_or("unknown");
    let repo = body.repository.as_ref()
        .and_then(|r| r.full_name.as_deref())
        .unwrap_or("unknown");

    tracing::info!(action = %action, repo = %repo, "github webhook");

    // Handle PR events
    if let Some(pr) = &body.pull_request {
        match action {
            "opened" | "synchronize" => {
                tracing::info!(
                    pr_number = pr.number,
                    title = pr.title.as_deref().unwrap_or(""),
                    "PR opened/updated - triggering pr-risk analysis"
                );
                // TODO: look up org by repo name, run pr-risk tool,
                // post result as PR comment via GitHub API
            }
            _ => {}
        }
    }

    Ok(StatusCode::OK)
}
