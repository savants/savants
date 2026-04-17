use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    org: String,
    exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub auth_method: AuthMethod,
}

#[derive(Debug, Clone)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
    AgentKey,
}

#[axum::async_trait]
impl FromRequestParts<std::sync::Arc<AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // API key auth: sk_live_<key>
        if token.starts_with("sk_live_") {
            return Self::auth_api_key(token, state).await;
        }

        // Agent key auth: svt_agent_<key>
        if token.starts_with("svt_agent_") {
            return Self::auth_agent_key(token, state).await;
        }

        // JWT auth (default)
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let user_id = Uuid::parse_str(&token_data.claims.sub)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let org_id = Uuid::parse_str(&token_data.claims.org)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        Ok(AuthUser { user_id, org_id, auth_method: AuthMethod::Jwt })
    }
}

impl AuthUser {
    async fn auth_api_key(
        token: &str,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, StatusCode> {
        // Look up by prefix (first 12 chars) then verify hash
        let prefix = &token[..std::cmp::min(12, token.len())];

        let row = sqlx::query_as::<_, (Uuid, Uuid, Uuid, String)>(
            "SELECT id, org_id, user_id, key_hash FROM api_keys WHERE prefix = $1"
        )
        .bind(prefix)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

        let (_key_id, org_id, user_id, key_hash) = row;

        // Verify the full key against the stored hash
        bcrypt::verify(token, &key_hash)
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .then_some(())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // Update last_used_at
        let _ = sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE prefix = $1")
            .bind(prefix)
            .execute(&state.db)
            .await;

        Ok(AuthUser { user_id, org_id, auth_method: AuthMethod::ApiKey })
    }

    async fn auth_agent_key(
        token: &str,
        state: &std::sync::Arc<AppState>,
    ) -> Result<Self, StatusCode> {
        let prefix = &token[..std::cmp::min(14, token.len())];

        let row = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT id, org_id, key_hash FROM agent_keys WHERE prefix = $1 AND revoked_at IS NULL"
        )
        .bind(prefix)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;

        let (_key_id, org_id, key_hash) = row;

        bcrypt::verify(token, &key_hash)
            .map_err(|_| StatusCode::UNAUTHORIZED)?
            .then_some(())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let _ = sqlx::query("UPDATE agent_keys SET last_used_at = now() WHERE prefix = $1")
            .bind(prefix)
            .execute(&state.db)
            .await;

        // Agent keys don't have a user_id, use a nil UUID
        Ok(AuthUser { user_id: Uuid::nil(), org_id, auth_method: AuthMethod::AgentKey })
    }
}
