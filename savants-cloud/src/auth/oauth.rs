use axum::{
    extract::{Query, State},
    response::Redirect,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

// ---------- shared types ----------

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    org: String,
    exp: usize,
}

#[derive(Deserialize)]
pub struct OAuthCallbackParams {
    pub code: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Deserialize)]
pub struct OAuthStartParams {
    /// Optional user_code from device auth flow - gets forwarded through the OAuth state param
    #[serde(default)]
    pub user_code: Option<String>,
}

#[derive(Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GoogleUserInfo {
    email: String,
    name: Option<String>,
    picture: Option<String>,
    id: String,
}

#[derive(Deserialize)]
struct GitHubTokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
    id: i64,
    email: Option<String>,
}

#[derive(Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

// ---------- OAuth start endpoints (redirect to provider) ----------

pub async fn google_start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OAuthStartParams>,
) -> Redirect {
    let redirect_uri = format!("{}/auth/callback/google", state.base_url);
    // Pass user_code through the OAuth state param so it survives the redirect
    let oauth_state = params.user_code.unwrap_or_default();

    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
         client_id={}&\
         redirect_uri={}&\
         response_type=code&\
         scope=email%20profile&\
         access_type=offline&\
         state={}",
        urlencoded(&state.google_client_id),
        urlencoded(&redirect_uri),
        urlencoded(&oauth_state),
    );

    Redirect::temporary(&url)
}

pub async fn github_start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OAuthStartParams>,
) -> Redirect {
    let redirect_uri = format!("{}/auth/callback/github", state.base_url);
    let oauth_state = params.user_code.unwrap_or_default();

    let url = format!(
        "https://github.com/login/oauth/authorize?\
         client_id={}&\
         redirect_uri={}&\
         scope=user:email&\
         state={}",
        urlencoded(&state.github_client_id),
        urlencoded(&redirect_uri),
        urlencoded(&oauth_state),
    );

    Redirect::temporary(&url)
}

// ---------- OAuth callback endpoints ----------

pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OAuthCallbackParams>,
) -> Redirect {
    let result = google_callback_inner(&state, &params).await;
    match result {
        Ok(redirect) => redirect,
        Err(msg) => {
            tracing::error!("Google OAuth error: {}", msg);
            Redirect::temporary(&format!(
                "https://savants.cloud/activate?status=error&message={}",
                urlencoded(&msg)
            ))
        }
    }
}

async fn google_callback_inner(
    state: &AppState,
    params: &OAuthCallbackParams,
) -> Result<Redirect, String> {
    let redirect_uri = format!("{}/auth/callback/google", state.base_url);
    let client = reqwest::Client::new();

    // 1. Exchange code for token
    let token_resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", state.google_client_id.as_str()),
            ("client_secret", state.google_client_secret.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("token request failed: {}", e))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed: {}", body));
    }

    let token_data: GoogleTokenResponse = token_resp
        .json()
        .await
        .map_err(|e| format!("token parse failed: {}", e))?;

    // 2. Fetch user info
    let user_resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(&token_data.access_token)
        .send()
        .await
        .map_err(|e| format!("userinfo request failed: {}", e))?;

    if !user_resp.status().is_success() {
        return Err("userinfo fetch failed".to_string());
    }

    let user_info: GoogleUserInfo = user_resp
        .json()
        .await
        .map_err(|e| format!("userinfo parse failed: {}", e))?;

    // 3. Create/update user + org, approve pending device session
    let user_code = params.state.clone().unwrap_or_default();
    let redirect = finish_oauth(
        state,
        &user_info.email,
        user_info.name.as_deref(),
        user_info.picture.as_deref(),
        "google",
        &user_info.id,
        &user_code,
    )
    .await?;

    Ok(redirect)
}

pub async fn github_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OAuthCallbackParams>,
) -> Redirect {
    let result = github_callback_inner(&state, &params).await;
    match result {
        Ok(redirect) => redirect,
        Err(msg) => {
            tracing::error!("GitHub OAuth error: {}", msg);
            Redirect::temporary(&format!(
                "https://savants.cloud/activate?status=error&message={}",
                urlencoded(&msg)
            ))
        }
    }
}

async fn github_callback_inner(
    state: &AppState,
    params: &OAuthCallbackParams,
) -> Result<Redirect, String> {
    let client = reqwest::Client::new();

    // 1. Exchange code for token
    let token_resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("code", params.code.as_str()),
            ("client_id", state.github_client_id.as_str()),
            ("client_secret", state.github_client_secret.as_str()),
        ])
        .send()
        .await
        .map_err(|e| format!("token request failed: {}", e))?;

    if !token_resp.status().is_success() {
        let body = token_resp.text().await.unwrap_or_default();
        return Err(format!("token exchange failed: {}", body));
    }

    let token_data: GitHubTokenResponse = token_resp
        .json()
        .await
        .map_err(|e| format!("token parse failed: {}", e))?;

    // 2. Fetch user info
    let user_resp = client
        .get("https://api.github.com/user")
        .header("User-Agent", "savants-cloud")
        .bearer_auth(&token_data.access_token)
        .send()
        .await
        .map_err(|e| format!("user request failed: {}", e))?;

    if !user_resp.status().is_success() {
        return Err("user fetch failed".to_string());
    }

    let gh_user: GitHubUser = user_resp
        .json()
        .await
        .map_err(|e| format!("user parse failed: {}", e))?;

    // GitHub may not return email in the user endpoint - fetch from emails API
    let email = match gh_user.email {
        Some(ref e) if !e.is_empty() => e.clone(),
        _ => {
            let emails_resp = client
                .get("https://api.github.com/user/emails")
                .header("User-Agent", "savants-cloud")
                .bearer_auth(&token_data.access_token)
                .send()
                .await
                .map_err(|e| format!("emails request failed: {}", e))?;

            let emails: Vec<GitHubEmail> = emails_resp
                .json()
                .await
                .map_err(|e| format!("emails parse failed: {}", e))?;

            emails
                .iter()
                .find(|e| e.primary && e.verified)
                .or_else(|| emails.iter().find(|e| e.verified))
                .map(|e| e.email.clone())
                .ok_or_else(|| "no verified email found on GitHub account".to_string())?
        }
    };

    let name = gh_user.name.or_else(|| Some(gh_user.login.clone()));
    let provider_id = gh_user.id.to_string();

    // 3. Create/update user + org, approve pending device session
    let user_code = params.state.clone().unwrap_or_default();
    let redirect = finish_oauth(
        state,
        &email,
        name.as_deref(),
        gh_user.avatar_url.as_deref(),
        "github",
        &provider_id,
        &user_code,
    )
    .await?;

    Ok(redirect)
}

// ---------- shared post-OAuth logic ----------

/// Create or update user, create org if needed, approve any pending device session,
/// then redirect to the activate page with a success status.
async fn finish_oauth(
    state: &AppState,
    email: &str,
    name: Option<&str>,
    avatar_url: Option<&str>,
    provider: &str,
    provider_id: &str,
    user_code: &str,
) -> Result<Redirect, String> {
    let display_name = name.unwrap_or("Savants User");

    // Upsert user
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, name, avatar_url, auth_provider, auth_provider_id) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (email) DO UPDATE SET \
           name = EXCLUDED.name, \
           avatar_url = COALESCE(EXCLUDED.avatar_url, users.avatar_url) \
         RETURNING id",
    )
    .bind(email)
    .bind(display_name)
    .bind(avatar_url)
    .bind(provider)
    .bind(provider_id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| format!("user upsert failed: {}", e))?;

    // Find or create org
    let org_id: Uuid = match sqlx::query_scalar::<_, Uuid>(
        "SELECT org_id FROM memberships WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| format!("membership lookup failed: {}", e))?
    {
        Some(oid) => oid,
        None => {
            let slug = email
                .split('@')
                .next()
                .unwrap_or("user")
                .replace('.', "-");
            let org_id: Uuid = sqlx::query_scalar(
                "INSERT INTO orgs (slug, name, plan) VALUES ($1, $2, 'free') RETURNING id",
            )
            .bind(&slug)
            .bind(&format!("{}'s workspace", display_name))
            .fetch_one(&state.db)
            .await
            .map_err(|e| format!("org create failed: {}", e))?;

            // Add user as owner
            let _ = sqlx::query(
                "INSERT INTO memberships (user_id, org_id, role) VALUES ($1, $2, 'owner')",
            )
            .bind(user_id)
            .bind(org_id)
            .execute(&state.db)
            .await;

            // Create default graph scope
            let graph_name = format!("org_{}", org_id.to_string().replace('-', ""));
            let _ = sqlx::query(
                "INSERT INTO graph_scopes (org_id, scope_type, scope_name, falkordb_graph_name) \
                 VALUES ($1, 'default', 'main', $2) ON CONFLICT DO NOTHING",
            )
            .bind(org_id)
            .bind(&graph_name)
            .execute(&state.db)
            .await;

            org_id
        }
    };

    // If there is a pending device auth session (user_code from the state param), approve it
    if !user_code.is_empty() {
        let updated = sqlx::query(
            "UPDATE device_auth_sessions \
             SET status = 'approved', user_id = $1, org_id = $2 \
             WHERE user_code = $3 AND status = 'pending' AND expires_at > NOW()",
        )
        .bind(user_id)
        .bind(org_id)
        .bind(user_code)
        .execute(&state.db)
        .await;

        match updated {
            Ok(result) => {
                if result.rows_affected() > 0 {
                    tracing::info!(
                        "Approved device session for user_code={} user={}",
                        user_code,
                        user_id
                    );
                } else {
                    tracing::warn!(
                        "No pending device session found for user_code={}",
                        user_code
                    );
                }
            }
            Err(e) => {
                tracing::error!("Failed to approve device session: {}", e);
                // Don't fail the whole flow - user is still signed in
            }
        }
    }

    // Generate a JWT for the redirect
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
    .map_err(|e| format!("jwt encode failed: {}", e))?;

    // Redirect back to activate page with success
    let redirect_url = if !user_code.is_empty() {
        format!(
            "https://savants.cloud/activate?status=success&user_code={}&token={}",
            urlencoded(user_code),
            urlencoded(&token),
        )
    } else {
        format!(
            "https://savants.cloud/activate?status=success&token={}",
            urlencoded(&token),
        )
    };

    Ok(Redirect::temporary(&redirect_url))
}

/// Minimal percent-encoding for URL query values.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}
