use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct SlackEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub challenge: Option<String>,
    pub token: Option<String>,
    pub event: Option<SlackEventPayload>,
    pub team_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SlackEventPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    pub text: Option<String>,
    pub user: Option<String>,
    pub channel: Option<String>,
    pub ts: Option<String>,
    pub thread_ts: Option<String>,
    pub bot_id: Option<String>,
}

#[derive(Serialize)]
pub struct SlackResponse {
    pub challenge: Option<String>,
}

pub async fn handle_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SlackEvent>,
) -> Result<Json<SlackResponse>, StatusCode> {
    // URL verification challenge (Slack sends this when setting up the webhook)
    if body.event_type == "url_verification" {
        return Ok(Json(SlackResponse { challenge: body.challenge }));
    }

    // Ignore bot messages (prevent infinite loops)
    if let Some(ref event) = body.event {
        if event.bot_id.is_some() {
            return Ok(Json(SlackResponse { challenge: None }));
        }
    }

    // Handle @savants mentions
    if let Some(event) = body.event {
        if event.event_type == "app_mention" {
            if let (Some(text), Some(channel)) = (&event.text, &event.channel) {
                let team_id = body.team_id.unwrap_or_default();
                tracing::info!(
                    channel = %channel,
                    user = event.user.as_deref().unwrap_or("?"),
                    text = %text,
                    team = %team_id,
                    "slack @savants mention"
                );

                // Parse command from the mention text
                // Format: "@savants diagnose-error TypeError in handlePayment"
                // or: "@savants what's wrong with the payment service?"
                let command = extract_command(text);

                // Spawn async task to process and respond
                let state_clone = state.clone();
                let channel = channel.clone();
                let thread_ts = event.ts.clone();
                tokio::spawn(async move {
                    if let Err(e) = process_and_respond(&state_clone, &team_id, &channel, thread_ts.as_deref(), &command).await {
                        tracing::error!("Failed to respond to Slack: {}", e);
                    }
                });
            }
        }
    }

    Ok(Json(SlackResponse { challenge: None }))
}

/// Extract the command from an @savants mention.
/// "@savants diagnose-error TypeError in foo" -> "diagnose-error TypeError in foo"
/// "@savants what's wrong?" -> "diagnose what's wrong?"
fn extract_command(text: &str) -> String {
    // Remove the @mention (format: <@U0XXXXXXX> or @savants)
    let cleaned = regex::Regex::new(r"<@[A-Z0-9]+>|@savants")
        .unwrap()
        .replace_all(text, "")
        .trim()
        .to_string();

    if cleaned.is_empty() {
        "diagnose".to_string() // default to general diagnosis
    } else {
        cleaned
    }
}

/// Process the command and post the result back to Slack.
async fn process_and_respond(
    state: &Arc<AppState>,
    team_id: &str,
    channel: &str,
    thread_ts: Option<&str>,
    command: &str,
) -> Result<(), String> {
    // Look up the org by Slack team ID
    let org_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT org_id FROM graph_scopes WHERE scope_type = 'slack' AND scope_name = $1"
    )
    .bind(team_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("DB error: {}", e))?;

    let org_id = match org_id {
        Some(id) => id,
        None => {
            // Try finding any org (for now, use first org - will be fixed with proper Slack app install flow)
            sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM orgs LIMIT 1")
                .fetch_optional(&state.db)
                .await
                .map_err(|e| format!("DB error: {}", e))?
                .ok_or("No org found")?
        }
    };

    // Determine which tool to call based on the command
    let (tool, arguments) = parse_tool_from_command(command);

    tracing::info!(org_id = %org_id, tool = %tool, "processing slack command");

    // Call the tool via the same proxy that the API uses
    // For now, spawn the savants binary directly
    let init_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "savants-slack-bot", "version": "0.1.0"}}
    });
    let tool_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    let input = format!("{}\n{}\n", init_msg, tool_msg);

    let output = tokio::process::Command::new("savants")
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn error: {}", e))?;

    let mut child = output;
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
    .map_err(|e| format!("process error: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.lines().last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|v| v.get("result").cloned())
        .and_then(|r| r.get("content").cloned())
        .and_then(|c| c.as_array().and_then(|a| a.first().cloned()))
        .and_then(|t| t.get("text").and_then(|t| t.as_str().map(|s| s.to_string())))
        .unwrap_or_else(|| "No result from context engine.".to_string());

    // Truncate for Slack (max 4000 chars in a message)
    let response_text = if result.len() > 3900 {
        format!("{}...\n\n(truncated - full result available via API)", &result[..3900])
    } else {
        result
    };

    // Post the result back to Slack
    let slack_token = std::env::var("SLACK_BOT_TOKEN").unwrap_or_default();
    if slack_token.is_empty() {
        return Err("SLACK_BOT_TOKEN not set".to_string());
    }

    let payload = serde_json::json!({
        "channel": channel,
        "text": format!("```\n{}\n```", response_text),
        "thread_ts": thread_ts,
    });

    let client = reqwest::Client::new();
    let resp = client.post("https://slack.com/api/chat.postMessage")
        .header("Authorization", format!("Bearer {}", slack_token))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Slack API error: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Slack returned status {}", resp.status()));
    }

    // Log usage
    let _ = sqlx::query(
        "INSERT INTO usage_events (org_id, endpoint, duration_ms, status_code) VALUES ($1, $2, 0, 200)"
    )
    .bind(org_id)
    .bind(&tool)
    .execute(&state.db)
    .await;

    Ok(())
}

/// Parse a natural language command into a tool name and arguments.
fn parse_tool_from_command(command: &str) -> (String, serde_json::Value) {
    let lower = command.to_lowercase();

    // Direct tool names
    if lower.starts_with("diagnose-error") || lower.starts_with("diagnose error") {
        let error_text = command.splitn(2, ' ').nth(1).unwrap_or(command);
        return ("diagnose-error".to_string(), serde_json::json!({"error": error_text}));
    }
    if lower.starts_with("pr-risk") || lower.starts_with("pr risk") {
        return ("pr-risk".to_string(), serde_json::json!({}));
    }
    if lower.starts_with("radar") {
        let user = command.split_whitespace().nth(1).unwrap_or("me");
        return ("radar".to_string(), serde_json::json!({"user": user}));
    }
    if lower.starts_with("search ") || lower.starts_with("find ") {
        let pattern = command.splitn(2, ' ').nth(1).unwrap_or("");
        return ("search_code".to_string(), serde_json::json!({"pattern": pattern}));
    }
    if lower.starts_with("skeleton ") || lower.starts_with("structure ") {
        let file = command.splitn(2, ' ').nth(1).unwrap_or("");
        return ("file_skeleton".to_string(), serde_json::json!({"file": file}));
    }
    if lower.starts_with("who uses ") || lower.starts_with("where is ") {
        let symbol = command.splitn(3, ' ').nth(2).unwrap_or("");
        return ("where_used".to_string(), serde_json::json!({"symbol": symbol}));
    }
    if lower.starts_with("callers ") {
        let func = command.splitn(2, ' ').nth(1).unwrap_or("");
        return ("callers".to_string(), serde_json::json!({"function": func}));
    }
    if lower.starts_with("imports ") {
        let file = command.splitn(2, ' ').nth(1).unwrap_or("");
        return ("import_tree".to_string(), serde_json::json!({"file": file}));
    }
    if lower.starts_with("status") || lower.starts_with("stats") {
        return ("graph_stats".to_string(), serde_json::json!({}));
    }

    // Default: treat as a diagnose-error query
    ("diagnose-error".to_string(), serde_json::json!({"error": command}))
}
