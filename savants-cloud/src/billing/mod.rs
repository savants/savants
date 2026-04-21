use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::middleware::AuthUser;
use crate::AppState;

// ---------------------------------------------------------------------------
// GET /api/v1/billing - current billing info for the authenticated org
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct BillingInfo {
    pub plan: String,
    pub stripe_customer_id: Option<String>,
    pub status: String,
}

pub async fn get_billing(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<BillingInfo>, StatusCode> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT plan, stripe_customer_id FROM orgs WHERE id = $1"
    )
    .bind(auth.org_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(BillingInfo {
        plan: row.0.clone(),
        stripe_customer_id: row.1.clone(),
        status: "active".to_string(),
    }))
}

// ---------------------------------------------------------------------------
// POST /api/v1/billing/checkout - create a Stripe Checkout Session for PAYG
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub url: String,
}

/// Stripe API response subset we care about
#[derive(Deserialize)]
struct StripeCheckoutSession {
    id: String,
    url: Option<String>,
}

pub async fn create_checkout(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<CheckoutResponse>, StatusCode> {
    if state.stripe_secret_key.is_empty() {
        tracing::error!("STRIPE_SECRET_KEY not configured");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Check if already on a paid plan
    let plan: String = sqlx::query_scalar("SELECT plan FROM orgs WHERE id = $1")
        .bind(auth.org_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if plan != "free" {
        return Err(StatusCode::CONFLICT); // already upgraded
    }

    // Look up org slug for metadata
    let org_slug: String = sqlx::query_scalar("SELECT slug FROM orgs WHERE id = $1")
        .bind(auth.org_id)
        .fetch_one(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Check if org already has a stripe_customer_id, reuse it
    let existing_customer: Option<String> = sqlx::query_scalar(
        "SELECT stripe_customer_id FROM orgs WHERE id = $1"
    )
    .bind(auth.org_id)
    .fetch_one(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let success_url = format!("{}/billing/success?session_id={{CHECKOUT_SESSION_ID}}", state.base_url);
    let cancel_url = format!("{}/billing", state.base_url);

    // Build the form body for Stripe Checkout Session creation
    let mut params = vec![
        ("mode".to_string(), "subscription".to_string()),
        ("line_items[0][price]".to_string(), state.stripe_payg_price_id.clone()),
        ("line_items[0][quantity]".to_string(), "1".to_string()),
        ("success_url".to_string(), success_url),
        ("cancel_url".to_string(), cancel_url),
        ("metadata[org_id]".to_string(), auth.org_id.to_string()),
        ("metadata[org_slug]".to_string(), org_slug),
        ("subscription_data[metadata][org_id]".to_string(), auth.org_id.to_string()),
    ];

    if let Some(ref cid) = existing_customer {
        params.push(("customer".to_string(), cid.clone()));
    } else {
        // Let Stripe create a new customer; we'll store the ID from the webhook
        params.push(("customer_creation".to_string(), "always".to_string()));
    }

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .header("Authorization", format!("Bearer {}", state.stripe_secret_key))
        .form(&params)
        .send()
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "stripe checkout request failed");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(
            stripe_status = %status,
            body = %body,
            "stripe checkout session creation failed"
        );
        return Err(StatusCode::BAD_GATEWAY);
    }

    let session: StripeCheckoutSession = resp.json().await.map_err(|e| {
        tracing::error!(error = %e, "failed to parse stripe checkout response");
        StatusCode::BAD_GATEWAY
    })?;

    let checkout_url = session.url.unwrap_or_else(|| {
        format!("https://checkout.stripe.com/c/pay/{}", session.id)
    });

    // Log billing event
    let _ = sqlx::query(
        "INSERT INTO billing_events (org_id, event_type, payload) VALUES ($1, 'checkout_created', $2)"
    )
    .bind(auth.org_id)
    .bind(serde_json::json!({
        "session_id": session.id,
        "price_id": state.stripe_payg_price_id,
    }))
    .execute(&state.db)
    .await;

    Ok(Json(CheckoutResponse { url: checkout_url }))
}

// ---------------------------------------------------------------------------
// POST /webhooks/stripe - Stripe webhook handler
// ---------------------------------------------------------------------------

/// Minimal Stripe event envelope
#[derive(Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    data: StripeEventData,
}

#[derive(Deserialize)]
struct StripeEventData {
    object: serde_json::Value,
}

pub async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> StatusCode {
    tracing::info!(bytes = body.len(), "received stripe webhook");

    // Verify webhook signature if secret is configured
    if !state.stripe_webhook_secret.is_empty() {
        let sig_header = headers
            .get("stripe-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !verify_stripe_signature(&body, sig_header, &state.stripe_webhook_secret) {
            tracing::warn!("stripe webhook signature verification failed");
            return StatusCode::BAD_REQUEST;
        }
    }

    let event: StripeEvent = match serde_json::from_str(&body) {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "failed to parse stripe event");
            return StatusCode::BAD_REQUEST;
        }
    };

    tracing::info!(
        event_id = %event.id,
        event_type = %event.event_type,
        "processing stripe event"
    );

    match event.event_type.as_str() {
        "checkout.session.completed" => {
            handle_checkout_completed(&state, &event.data.object).await
        }
        "customer.subscription.deleted" => {
            handle_subscription_deleted(&state, &event.data.object).await
        }
        _ => {
            tracing::debug!(event_type = %event.event_type, "ignoring unhandled stripe event");
            StatusCode::OK
        }
    }
}

/// Handle checkout.session.completed: upgrade org from free to payg
async fn handle_checkout_completed(
    state: &Arc<AppState>,
    object: &serde_json::Value,
) -> StatusCode {
    let org_id_str = object
        .pointer("/metadata/org_id")
        .and_then(|v| v.as_str());

    let org_id = match org_id_str.and_then(|s| uuid::Uuid::parse_str(s).ok()) {
        Some(id) => id,
        None => {
            tracing::error!("checkout.session.completed missing metadata.org_id");
            return StatusCode::BAD_REQUEST;
        }
    };

    let customer_id = object
        .get("customer")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let subscription_id = object
        .get("subscription")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Update org: plan -> payg, store stripe IDs
    let result = sqlx::query(
        "UPDATE orgs SET plan = 'payg', stripe_customer_id = $2, stripe_subscription_id = $3 WHERE id = $1"
    )
    .bind(org_id)
    .bind(customer_id)
    .bind(subscription_id)
    .execute(&state.db)
    .await;

    match result {
        Ok(r) => {
            if r.rows_affected() == 0 {
                tracing::error!(org_id = %org_id, "checkout completed but org not found");
                return StatusCode::NOT_FOUND;
            }
            tracing::info!(
                org_id = %org_id,
                customer_id = %customer_id,
                "org upgraded to payg"
            );
        }
        Err(e) => {
            tracing::error!(error = %e, org_id = %org_id, "failed to update org plan");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }

    // Ensure the org has a graph scope provisioned (idempotent)
    let _ = sqlx::query_scalar::<_, String>("SELECT ensure_default_graph_scope($1)")
        .bind(org_id)
        .fetch_one(&state.db)
        .await;

    // Log billing event
    let _ = sqlx::query(
        "INSERT INTO billing_events (org_id, event_type, stripe_event_id, payload) \
         VALUES ($1, 'checkout_completed', $2, $3) \
         ON CONFLICT (stripe_event_id) DO NOTHING"
    )
    .bind(org_id)
    .bind(format!("checkout_{}", customer_id))
    .bind(serde_json::json!({
        "customer_id": customer_id,
        "subscription_id": subscription_id,
    }))
    .execute(&state.db)
    .await;

    StatusCode::OK
}

/// Handle customer.subscription.deleted: downgrade org back to free
async fn handle_subscription_deleted(
    state: &Arc<AppState>,
    object: &serde_json::Value,
) -> StatusCode {
    // The subscription object has metadata.org_id
    let org_id_str = object
        .pointer("/metadata/org_id")
        .and_then(|v| v.as_str());

    let org_id = match org_id_str.and_then(|s| uuid::Uuid::parse_str(s).ok()) {
        Some(id) => id,
        None => {
            tracing::warn!("subscription.deleted missing metadata.org_id, ignoring");
            return StatusCode::OK;
        }
    };

    let _ = sqlx::query(
        "UPDATE orgs SET plan = 'free', stripe_subscription_id = NULL WHERE id = $1"
    )
    .bind(org_id)
    .execute(&state.db)
    .await;

    tracing::info!(org_id = %org_id, "org downgraded to free (subscription deleted)");
    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Stripe webhook signature verification (v1 scheme)
// ---------------------------------------------------------------------------

fn verify_stripe_signature(payload: &str, sig_header: &str, secret: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // Parse the signature header: t=<timestamp>,v1=<signature>
    let mut timestamp = "";
    let mut signature = "";

    for part in sig_header.split(',') {
        let part = part.trim();
        if let Some(t) = part.strip_prefix("t=") {
            timestamp = t;
        } else if let Some(s) = part.strip_prefix("v1=") {
            signature = s;
        }
    }

    if timestamp.is_empty() || signature.is_empty() {
        return false;
    }

    // Check timestamp is within 5 minutes
    if let Ok(ts) = timestamp.parse::<i64>() {
        let now = chrono::Utc::now().timestamp();
        if (now - ts).abs() > 300 {
            tracing::warn!(
                delta_secs = now - ts,
                "stripe webhook timestamp too old"
            );
            return false;
        }
    } else {
        return false;
    }

    // Compute expected signature: HMAC-SHA256(secret, timestamp.payload)
    let signed_payload = format!("{}.{}", timestamp, payload);
    let mut mac = match Hmac::<Sha256>::new_from_slice(secret.as_bytes()) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(signed_payload.as_bytes());

    let expected = hex::encode(mac.finalize().into_bytes());
    constant_time_eq(expected.as_bytes(), signature.as_bytes())
}

/// Constant-time comparison to prevent timing attacks
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}
