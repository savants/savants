//! MCP tool proxy - forwards API calls to the savants-cli binary's MCP server.
//! Each call invokes the binary with JSON-RPC over stdio, meters the usage,
//! and returns the result.

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use chrono::Datelike;
use crate::auth::middleware::AuthUser;
use crate::AppState;

/// Pricing table (cents per call)
const PRICING: &[(&str, u32)] = &[
    ("diagnose-error", 500),     // $5.00
    ("diagnose", 250),           // $2.50
    ("pr-risk", 200),            // $2.00
    ("diff-impact", 100),        // $1.00
    ("pre-change-warning", 100), // $1.00
    ("reindex", 200),            // $2.00 (full)
    ("reindex-diff", 25),        // $0.25
    ("radar", 100),              // $1.00
    ("pod-story", 100),          // $1.00
    ("host-story", 100),         // $1.00
    ("query", 25),               // $0.25
    ("search-code", 25),         // $0.25
    ("find-references", 25),     // $0.25
    ("dependency-chain", 25),    // $0.25
    ("co-change-partners", 25),  // $0.25
    ("cluster-state", 10),       // $0.10
    ("namespace-summary", 10),   // $0.10
    ("deployment-info", 10),     // $0.10
    ("list-pods", 5),            // $0.05
];

fn price_for_tool(tool: &str) -> u32 {
    PRICING.iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, cents)| *cents)
        .unwrap_or(25) // default $0.25 for unknown tools
}

#[derive(Deserialize)]
pub struct ToolCallRequest {
    pub tool: String,
    pub arguments: serde_json::Value,
    pub repo: Option<String>,
}

#[derive(Serialize)]
pub struct ToolCallResponse {
    pub result: serde_json::Value,
    pub cost_cents: u32,
    pub duration_ms: f64,
}

#[derive(Serialize)]
pub struct ToolListResponse {
    pub tools: Vec<ToolInfo>,
}

#[derive(Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub price: String,
    pub description: String,
}

pub async fn call_tool(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ToolCallRequest>,
) -> Result<Json<ToolCallResponse>, StatusCode> {
    let tool_name = &body.tool;
    let cost_cents = price_for_tool(tool_name);

    tracing::info!(
        org_id = %auth.org_id,
        tool = %tool_name,
        cost_cents = cost_cents,
        "tool call"
    );

    // Check free tier quota (10 calls/month)
    let now = chrono::Utc::now();
    let month_start = now.date_naive()
        .with_day(1)
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
        .map(|dt| chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc))
        .unwrap_or(now);
    let usage_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM usage_events WHERE org_id = $1 AND created_at >= $2"
    )
    .bind(auth.org_id)
    .bind(month_start)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Check if org is on free tier and over quota
    let plan: String = sqlx::query_scalar(
        "SELECT plan FROM orgs WHERE id = $1"
    )
    .bind(auth.org_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if plan == "free" && usage_count >= 10 {
        return Err(StatusCode::PAYMENT_REQUIRED);
    }

    // Resolve the graph name for this org
    let graph_name = sqlx::query_scalar::<_, String>(
        "SELECT falkordb_graph FROM graph_scopes WHERE org_id = $1 ORDER BY created_at LIMIT 1"
    )
    .bind(auth.org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .unwrap_or_else(|| "savants".to_string());

    // Build MCP JSON-RPC request
    let init_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "savants-cloud", "version": "0.1.0"}
        }
    });

    let tool_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": body.arguments,
        }
    });

    let input = format!("{}\n{}\n", init_msg, tool_msg);

    // Invoke the savants binary via MCP stdio
    let start = std::time::Instant::now();

    let mut child = tokio::process::Command::new("savants")
        .arg("serve")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Write input to stdin
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
    .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Parse the last line of stdout as JSON-RPC response
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.lines().last()
        .and_then(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .and_then(|v| v.get("result").cloned())
        .and_then(|r| r.get("content").cloned())
        .and_then(|c| c.as_array().and_then(|a| a.first().cloned()))
        .and_then(|t| t.get("text").cloned())
        .unwrap_or(serde_json::json!("No result"));

    // Log usage event
    let _ = sqlx::query(
        "INSERT INTO usage_events (org_id, endpoint, duration_ms, status_code) VALUES ($1, $2, $3, 200)"
    )
    .bind(auth.org_id)
    .bind(tool_name)
    .bind(duration_ms as i32)
    .execute(&state.db)
    .await;

    Ok(Json(ToolCallResponse {
        result,
        cost_cents,
        duration_ms,
    }))
}

pub async fn list_tools() -> Json<ToolListResponse> {
    let tools = PRICING.iter().map(|(name, cents)| {
        let price = if *cents >= 100 {
            format!("${}.{:02}", cents / 100, cents % 100)
        } else {
            format!("$0.{:02}", cents)
        };
        ToolInfo {
            name: name.to_string(),
            price,
            description: match *name {
                "diagnose-error" => "Root cause file + line. Upstream trace. Git blame. Slack context.".to_string(),
                "diagnose" => "General error analysis with full graph context.".to_string(),
                "pr-risk" => "8-check risk analysis on pull requests.".to_string(),
                "diff-impact" => "Blast radius: what breaks if this code changes.".to_string(),
                "pre-change-warning" => "Pre-merge safety check against production state.".to_string(),
                "reindex" => "Full repository re-index.".to_string(),
                "reindex-diff" => "Incremental re-index of changed files only.".to_string(),
                "radar" => "Personal digest: what you missed across all channels.".to_string(),
                "pod-story" | "host-story" => "Full incident timeline for any service or host.".to_string(),
                _ => "Graph query.".to_string(),
            },
        }
    }).collect();

    Json(ToolListResponse { tools })
}
