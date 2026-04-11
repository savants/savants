use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::db::{Membership, Org};
use crate::AppState;

#[derive(Serialize)]
pub struct MemberInfo {
    pub id: Uuid,
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub role: String,
    pub email: String,
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct InviteRequest {
    pub email: String,
    pub role: Option<String>,
}

#[derive(Serialize)]
pub struct InviteResponse {
    pub status: String,
    pub email: String,
}

pub async fn get_org(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<Org>, StatusCode> {
    let org = sqlx::query_as::<_, Org>("SELECT * FROM orgs WHERE id = $1")
        .bind(auth.org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(org))
}

pub async fn list_members(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<Vec<Membership>>, StatusCode> {
    let members = sqlx::query_as::<_, Membership>(
        "SELECT * FROM memberships WHERE org_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.org_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(members))
}

pub async fn invite_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<InviteRequest>,
) -> Result<Json<InviteResponse>, StatusCode> {
    // Verify the requesting user is admin/owner
    let membership = sqlx::query_as::<_, Membership>(
        "SELECT * FROM memberships WHERE user_id = $1 AND org_id = $2",
    )
    .bind(auth.user_id)
    .bind(auth.org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::FORBIDDEN)?;

    if membership.role != "admin" && membership.role != "owner" {
        return Err(StatusCode::FORBIDDEN);
    }

    // Check if user already exists
    let existing_user = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM users WHERE email = $1",
    )
    .bind(&body.email)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if let Some(user_id) = existing_user {
        // Check if already a member
        let already_member = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM memberships WHERE user_id = $1 AND org_id = $2",
        )
        .bind(user_id)
        .bind(auth.org_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        if already_member.is_some() {
            return Err(StatusCode::CONFLICT);
        }

        let role = body.role.unwrap_or_else(|| "member".to_string());
        sqlx::query(
            "INSERT INTO memberships (id, user_id, org_id, role, created_at) VALUES ($1, $2, $3, $4, NOW())",
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(auth.org_id)
        .bind(&role)
        .execute(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    // TODO: send invite email if user doesn't exist yet

    Ok(Json(InviteResponse {
        status: "invited".to_string(),
        email: body.email,
    }))
}
