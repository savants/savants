use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct GitHubWebhookPayload {
    pub action: Option<String>,
    pub pull_request: Option<GitHubPR>,
    pub repository: Option<GitHubRepo>,
    pub installation: Option<GitHubInstallation>,
}

#[derive(Deserialize)]
pub struct GitHubPR {
    pub number: i64,
    pub title: Option<String>,
    pub head: Option<GitHubRef>,
    pub base: Option<GitHubRef>,
    pub html_url: Option<String>,
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
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct GitHubInstallation {
    pub id: i64,
}

pub async fn handle_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GitHubWebhookPayload>,
) -> Result<StatusCode, StatusCode> {
    let action = body.action.as_deref().unwrap_or("unknown");
    let repo_full = body.repository.as_ref()
        .and_then(|r| r.full_name.as_deref())
        .unwrap_or("unknown");
    let repo_name = body.repository.as_ref()
        .and_then(|r| r.name.as_deref())
        .unwrap_or("unknown");

    tracing::info!(action = %action, repo = %repo_full, "github webhook");

    // Handle PR events - run pr-risk on opened/synchronized PRs
    if let Some(pr) = &body.pull_request {
        match action {
            "opened" | "synchronize" | "reopened" => {
                tracing::info!(
                    pr_number = pr.number,
                    title = pr.title.as_deref().unwrap_or(""),
                    "PR event - triggering pr-risk analysis"
                );

                // Look up org by repo name
                let org_id = sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT org_id FROM graph_scopes WHERE scope_name = $1"
                )
                .bind(repo_name)
                .fetch_optional(&state.db)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                if let Some(org_id) = org_id {
                    let state_clone = state.clone();
                    let pr_number = pr.number;
                    let repo = repo_full.to_string();
                    let repo_short = repo_name.to_string();
                    let installation_id = body.installation.as_ref().map(|i| i.id);

                    tokio::spawn(async move {
                        if let Err(e) = run_pr_risk_and_comment(
                            &state_clone, org_id, &repo, &repo_short,
                            pr_number, installation_id
                        ).await {
                            tracing::error!("Failed to run pr-risk: {}", e);
                        }
                    });
                }
            }
            _ => {}
        }
    }

    // Handle push events - trigger reindex-diff
    if action == "push" || body.pull_request.is_none() {
        // Could trigger reindex-diff here for push events
        tracing::debug!("push event for {} - reindex-diff could be triggered", repo_full);
    }

    Ok(StatusCode::OK)
}

async fn run_pr_risk_and_comment(
    state: &Arc<AppState>,
    org_id: uuid::Uuid,
    repo_full: &str,
    repo_name: &str,
    pr_number: i64,
    _installation_id: Option<i64>,
) -> Result<(), String> {
    // Run pr-risk via the savants binary
    let init_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "savants-github-bot", "version": "0.1.0"}}
    });
    let tool_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "tools/call",
        "params": {"name": "pr-risk", "arguments": {"repo": repo_name}}
    });
    let input = format!("{}\n{}\n", init_msg, tool_msg);

    let mut child = tokio::process::Command::new("savants")
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(input.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| "timeout")?
    .map_err(|e| format!("process: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.lines().last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|v| v.get("result").cloned())
        .and_then(|r| r.get("content").cloned())
        .and_then(|c| c.as_array().and_then(|a| a.first().cloned()))
        .and_then(|t| t.get("text").and_then(|t| t.as_str().map(|s| s.to_string())))
        .unwrap_or_else(|| "No PR risk analysis available.".to_string());

    // Post as PR comment via GitHub API
    // This requires a GitHub App installation token
    let github_token = std::env::var("GITHUB_APP_TOKEN").unwrap_or_default();
    if github_token.is_empty() {
        tracing::warn!("GITHUB_APP_TOKEN not set, skipping PR comment");
        return Ok(());
    }

    let comment_body = format!(
        "## Savants PR Risk Analysis\n\n```\n{}\n```\n\n---\n*Powered by [Savants](https://savants.dev) - the context engine for your LLM*",
        if result.len() > 60000 { &result[..60000] } else { &result }
    );

    let client = reqwest::Client::new();
    let resp = client.post(&format!(
        "https://api.github.com/repos/{}/issues/{}/comments",
        repo_full, pr_number
    ))
    .header("Authorization", format!("Bearer {}", github_token))
    .header("Accept", "application/vnd.github+json")
    .header("User-Agent", "savants-bot")
    .json(&serde_json::json!({"body": comment_body}))
    .send()
    .await
    .map_err(|e| format!("GitHub API: {}", e))?;

    if resp.status().is_success() {
        tracing::info!("Posted pr-risk comment on {}/#{}", repo_full, pr_number);
    } else {
        tracing::warn!("GitHub comment failed: {}", resp.status());
    }

    // Log usage
    let _ = sqlx::query(
        "INSERT INTO usage_events (org_id, endpoint, duration_ms, status_code) VALUES ($1, 'pr-risk', 0, 200)"
    )
    .bind(org_id)
    .execute(&state.db)
    .await;

    Ok(())
}
