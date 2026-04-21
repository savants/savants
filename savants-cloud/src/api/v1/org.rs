use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::db::{Membership, Org, ApiKey};
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

// ---------------------------------------------------------------
// API Key management
// ---------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct CreateKeyResponse {
    pub key: String,
    pub name: String,
    pub prefix: String,
    pub note: String,
}

#[derive(Serialize)]
pub struct KeyInfo {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateKeyRequest>,
) -> Result<Json<CreateKeyResponse>, StatusCode> {
    // Generate a random API key: sk_live_ + 48 hex chars
    let raw_key: String = format!("sk_live_{}", (0..24).map(|_| format!("{:02x}", rand::random::<u8>())).collect::<String>());
    let prefix = raw_key[..12].to_string();

    // Hash with bcrypt (cost 4 for speed)
    let hash = bcrypt::hash(&raw_key, 4).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    sqlx::query(
        "INSERT INTO api_keys (org_id, name, key_hash, key_prefix) VALUES ($1, $2, $3, $4)"
    )
    .bind(auth.org_id)
    .bind(&body.name)
    .bind(&hash)
    .bind(&prefix)
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(CreateKeyResponse {
        key: raw_key,
        name: body.name,
        prefix,
        note: "Save this key now. It will not be shown again.".to_string(),
    }))
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<Vec<KeyInfo>>, StatusCode> {
    let keys = sqlx::query_as::<_, (Uuid, String, String, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, name, key_prefix, last_used_at, created_at FROM api_keys WHERE org_id = $1 ORDER BY created_at DESC"
    )
    .bind(auth.org_id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let result: Vec<KeyInfo> = keys.iter().map(|(id, name, prefix, last_used, created)| {
        KeyInfo {
            id: *id,
            name: name.clone(),
            prefix: prefix.clone(),
            last_used_at: *last_used,
            created_at: *created,
        }
    }).collect();

    Ok(Json(result))
}

pub async fn delete_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    axum::extract::Path(key_id): axum::extract::Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let result = sqlx::query(
        "DELETE FROM api_keys WHERE id = $1 AND org_id = $2"
    )
    .bind(key_id)
    .bind(auth.org_id)
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if result.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
