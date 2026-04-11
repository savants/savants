use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

#[derive(Serialize)]
pub struct BillingInfo {
    pub plan: String,
    pub status: String,
}

#[derive(Serialize)]
pub struct CheckoutResponse {
    pub url: String,
}

pub async fn get_billing(
    State(_state): State<Arc<AppState>>,
) -> Json<BillingInfo> {
    // TODO: look up Stripe subscription for the org
    Json(BillingInfo {
        plan: "free".to_string(),
        status: "active".to_string(),
    })
}

pub async fn create_checkout(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<CheckoutResponse>, StatusCode> {
    // TODO: create Stripe checkout session
    Ok(Json(CheckoutResponse {
        url: "https://checkout.stripe.com/placeholder".to_string(),
    }))
}

pub async fn stripe_webhook(
    State(_state): State<Arc<AppState>>,
    body: String,
) -> StatusCode {
    tracing::info!(bytes = body.len(), "received stripe webhook");
    // TODO: verify signature, process event
    StatusCode::OK
}
