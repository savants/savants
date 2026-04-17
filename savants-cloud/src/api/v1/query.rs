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
    pub metadata: QueryMetadata,
}

#[derive(Serialize)]
pub struct QueryMetadata {
    pub graph: String,
    pub duration_ms: f64,
}

pub async fn run_query(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<QueryRequest>,
) -> Result<Json<QueryResponse>, StatusCode> {
    // Resolve the graph name for this org
    let graph_name = if let Some(ref g) = body.graph {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM graph_scopes WHERE org_id = $1 AND name = $2)"
        )
        .bind(auth.org_id)
        .bind(g)
        .fetch_one(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        if !exists {
            return Err(StatusCode::FORBIDDEN);
        }
        g.clone()
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT falkordb_graph_name FROM graph_scopes WHERE org_id = $1 ORDER BY created_at LIMIT 1"
        )
        .bind(auth.org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?
    };

    tracing::info!(
        user_id = %auth.user_id,
        org_id = %auth.org_id,
        graph = %graph_name,
        query = %body.query,
        "running graph query"
    );

    // Execute against FalkorDB
    let start = std::time::Instant::now();
    let mut conn = state.redis.get_connection()
        .map_err(|e| {
            tracing::error!("FalkorDB connection error: {}", e);
            StatusCode::SERVICE_UNAVAILABLE
        })?;

    let result: redis::Value = redis::cmd("GRAPH.QUERY")
        .arg(&graph_name)
        .arg(&body.query)
        .arg("--compact")
        .query(&mut conn)
        .map_err(|e| {
            tracing::error!("FalkorDB query error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
    let results = parse_falkordb_result(&result);

    // Log usage event
    let _ = sqlx::query(
        "INSERT INTO usage_events (org_id, endpoint, duration_ms, status_code) VALUES ($1, 'query', $2, 200)"
    )
    .bind(auth.org_id)
    .bind(duration_ms as i32)
    .execute(&state.db)
    .await;

    Ok(Json(QueryResponse {
        results,
        metadata: QueryMetadata { graph: graph_name, duration_ms },
    }))
}

/// Parse FalkorDB response (redis 0.25: Nil, Int, Data, Bulk, Status, Okay)
fn parse_falkordb_result(value: &redis::Value) -> serde_json::Value {
    match value {
        redis::Value::Bulk(arr) => {
            if arr.is_empty() {
                return serde_json::json!([]);
            }

            // First element: headers
            let headers: Vec<String> = match &arr[0] {
                redis::Value::Bulk(cols) => cols.iter().filter_map(|c| {
                    match c {
                        redis::Value::Bulk(pair) if pair.len() >= 2 => redis_to_string(&pair[1]),
                        redis::Value::Data(s) => Some(String::from_utf8_lossy(s).to_string()),
                        _ => None,
                    }
                }).collect(),
                _ => vec![],
            };

            // Second element: data rows
            let rows: Vec<serde_json::Value> = if arr.len() > 1 {
                match &arr[1] {
                    redis::Value::Bulk(data_rows) => data_rows.iter().map(|row| {
                        match row {
                            redis::Value::Bulk(cells) => {
                                let mut obj = serde_json::Map::new();
                                for (i, cell) in cells.iter().enumerate() {
                                    let key = headers.get(i).cloned().unwrap_or_else(|| format!("col_{}", i));
                                    obj.insert(key, redis_to_json(cell));
                                }
                                serde_json::Value::Object(obj)
                            }
                            _ => redis_to_json(row),
                        }
                    }).collect(),
                    _ => vec![],
                }
            } else {
                vec![]
            };

            serde_json::json!(rows)
        }
        _ => serde_json::json!([]),
    }
}

fn redis_to_json(value: &redis::Value) -> serde_json::Value {
    match value {
        redis::Value::Nil => serde_json::Value::Null,
        redis::Value::Int(i) => serde_json::json!(i),
        redis::Value::Data(s) => {
            let s = String::from_utf8_lossy(s);
            if let Ok(n) = s.parse::<i64>() {
                serde_json::json!(n)
            } else if let Ok(f) = s.parse::<f64>() {
                serde_json::json!(f)
            } else {
                serde_json::json!(s.to_string())
            }
        }
        redis::Value::Bulk(arr) => {
            if arr.len() == 2 {
                return redis_to_json(&arr[1]);
            }
            serde_json::Value::Array(arr.iter().map(redis_to_json).collect())
        }
        redis::Value::Status(s) => serde_json::json!(s),
        redis::Value::Okay => serde_json::json!("OK"),
    }
}

fn redis_to_string(value: &redis::Value) -> Option<String> {
    match value {
        redis::Value::Data(s) => Some(String::from_utf8_lossy(s).to_string()),
        redis::Value::Status(s) => Some(s.clone()),
        redis::Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}
