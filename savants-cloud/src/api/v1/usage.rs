use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use std::sync::Arc;

use crate::auth::middleware::AuthUser;
use crate::AppState;

#[derive(Serialize)]
pub struct UsageSummary {
    pub period: String,
    pub total_calls: i64,
    pub total_cost_cents: i64,
    pub by_tool: Vec<ToolUsage>,
    pub plan: String,
    pub free_remaining: i64,
}

#[derive(Serialize)]
pub struct ToolUsage {
    pub tool: String,
    pub calls: i64,
    pub avg_duration_ms: f64,
    pub cost_cents: i64,
}

/// Pricing in cents per call
fn price_cents(tool: &str) -> i64 {
    match tool {
        "diagnose-error" => 500,
        "diagnose" => 250,
        "pr-risk" => 200,
        "diff-impact" | "pre-change-warning" => 100,
        "radar" | "pod-story" | "host-story" => 100,
        "reindex" => 200,
        "reindex-diff" => 25,
        "import_tree" | "blast_radius" => 25,
        "file_skeleton" | "module_exports" | "where_used" | "callers" | "dead_code" => 0, // free
        "search_code" | "find_references" | "dependency_chain" | "co_change_partners" => 0,
        "graph_stats" | "cluster_state" | "list_pods" | "namespace_summary" => 0,
        _ => 25, // default
    }
}

pub async fn get_usage(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<UsageSummary>, StatusCode> {
    // Get current month usage
    let rows = sqlx::query_as::<_, (String, i64, f64)>(
        "SELECT endpoint, count(*), avg(duration_ms)::float8 \
         FROM usage_events \
         WHERE org_id = $1 AND created_at >= date_trunc('month', now()) \
         GROUP BY endpoint ORDER BY count(*) DESC"
    )
    .bind(auth.org_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut by_tool = vec![];
    let mut total_calls = 0i64;
    let mut total_cost = 0i64;

    for (tool, calls, avg_ms) in &rows {
        let cost = price_cents(tool) * calls;
        by_tool.push(ToolUsage {
            tool: tool.clone(),
            calls: *calls,
            avg_duration_ms: *avg_ms,
            cost_cents: cost,
        });
        total_calls += calls;
        total_cost += cost;
    }

    let plan: String = sqlx::query_scalar("SELECT plan FROM orgs WHERE id = $1")
        .bind(auth.org_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let free_remaining = if plan == "free" { (10 - total_calls).max(0) } else { -1 };

    let period = chrono::Utc::now().format("%Y-%m").to_string();

    Ok(Json(UsageSummary {
        period,
        total_calls,
        total_cost_cents: total_cost,
        by_tool,
        plan,
        free_remaining,
    }))
}
