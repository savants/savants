use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

use crate::auth::middleware::AuthUser;
use crate::db::GraphScope;
use crate::AppState;

pub async fn list_graphs(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<Vec<GraphScope>>, StatusCode> {
    let graphs = sqlx::query_as::<_, GraphScope>(
        "SELECT * FROM graph_scopes WHERE org_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.org_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(graphs))
}
