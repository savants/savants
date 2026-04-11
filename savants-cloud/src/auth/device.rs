use axum::{extract::State, http::StatusCode, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Serialize)]
pub struct CodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u32,
    pub expires_in: u32,
}

#[derive(Deserialize)]
pub struct PollRequest {
    pub device_code: String,
}

#[derive(Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub org_id: Uuid,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    org: String,
    exp: usize,
}

pub async fn request_code(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CodeResponse>, StatusCode> {
    let (device_code, user_code) = {
        let mut rng = rand::thread_rng();

        // 32 hex chars
        let device_code: String = (0..16)
            .map(|_| format!("{:02x}", rng.gen::<u8>()))
            .collect();

        // 8 alphanumeric uppercase
        let user_code: String = (0..8)
            .map(|_| {
                let idx = rng.gen_range(0..36u8);
                if idx < 10 {
                    (b'0' + idx) as char
                } else {
                    (b'A' + idx - 10) as char
                }
            })
            .collect();

        (device_code, user_code)
    };

    let id = Uuid::new_v4();
    let expires_at = Utc::now() + Duration::seconds(900);

    sqlx::query(
        r#"INSERT INTO device_auth_sessions (id, device_code, user_code, status, expires_at, created_at)
           VALUES ($1, $2, $3, 'pending', $4, NOW())"#,
    )
    .bind(id)
    .bind(&device_code)
    .bind(&user_code)
    .bind(expires_at)
    .execute(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(CodeResponse {
        device_code,
        user_code,
        verification_uri: "https://savants.cloud/activate".to_string(),
        interval: 5,
        expires_in: 900,
    }))
}

pub async fn poll_token(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PollRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let session = sqlx::query_as::<_, crate::db::DeviceAuthSession>(
        "SELECT * FROM device_auth_sessions WHERE device_code = $1",
    )
    .bind(&body.device_code)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "internal_error".to_string(),
            }),
        )
    })?;

    let session = match session {
        Some(s) => s,
        None => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "invalid_device_code".to_string(),
                }),
            ));
        }
    };

    if session.expires_at < Utc::now() {
        return Err((
            StatusCode::GONE,
            Json(ErrorResponse {
                error: "expired_token".to_string(),
            }),
        ));
    }

    match session.status.as_str() {
        "pending" => Err((
            StatusCode::PRECONDITION_REQUIRED,
            Json(ErrorResponse {
                error: "authorization_pending".to_string(),
            }),
        )),
        "denied" => Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "access_denied".to_string(),
            }),
        )),
        "approved" => {
            let user_id = session.user_id.ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "missing_user".to_string(),
                    }),
                )
            })?;
            let org_id = session.org_id.ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "missing_org".to_string(),
                    }),
                )
            })?;

            let exp = (Utc::now() + Duration::hours(24)).timestamp() as usize;
            let claims = Claims {
                sub: user_id.to_string(),
                org: org_id.to_string(),
                exp,
            };

            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
            )
            .map_err(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "token_generation_failed".to_string(),
                    }),
                )
            })?;

            Ok(Json(serde_json::json!({
                "access_token": token,
                "token_type": "Bearer",
                "org_id": org_id,
            })))
        }
        _ => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "unknown_status".to_string(),
            }),
        )),
    }
}
