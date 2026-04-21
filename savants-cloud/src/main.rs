use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod auth;
mod api;
mod billing;
mod db;

pub struct AppState {
    pub db: sqlx::PgPool,
    pub redis: redis::Client,
    pub jwt_secret: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "savants_cloud=info,tower_http=debug".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://savants:savants@localhost:5432/savants".to_string());
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:16379".to_string());
    let jwt_secret = std::env::var("JWT_SECRET")
        .unwrap_or_else(|_| "savants-dev-secret-change-in-production".to_string());

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("Failed to connect to PostgreSQL");

    // Run migrations
    sqlx::migrate!("./migrations")
        .run(&db)
        .await
        .expect("Failed to run migrations");

    let redis = redis::Client::open(redis_url)
        .expect("Failed to connect to Redis");

    let state = Arc::new(AppState {
        db,
        redis,
        jwt_secret,
    });

    let app = Router::new()
        // Health
        .route("/health", get(health))
        // Auth
        .route("/auth/device/code", post(auth::device::request_code))
        .route("/auth/device/token", post(auth::device::poll_token))
        .route("/auth/callback/google", get(auth::oauth::google_callback))
        .route("/auth/callback/github", get(auth::oauth::github_callback))
        // Tools (the core API - metered PAYG endpoints)
        .route("/api/v1/tools", get(api::v1::tools::list_tools))
        .route("/api/v1/tools/call", post(api::v1::tools::call_tool))
        // Ingest
        .route("/api/v1/ingest/delta", post(api::v1::ingest::push_delta))
        // Query
        .route("/api/v1/query", post(api::v1::query::run_query))
        .route("/api/v1/graphs", get(api::v1::graphs::list_graphs))
        // Org
        .route("/api/v1/org", get(api::v1::org::get_org))
        .route("/api/v1/org/members", get(api::v1::org::list_members))
        .route("/api/v1/org/members/invite", post(api::v1::org::invite_member))
        // API Keys
        .route("/api/v1/org/keys", get(api::v1::org::list_api_keys))
        .route("/api/v1/org/keys", post(api::v1::org::create_api_key))
        .route("/api/v1/org/keys/:key_id", axum::routing::delete(api::v1::org::delete_api_key))
        // Usage
        .route("/api/v1/usage", get(api::v1::usage::get_usage))
        // Billing
        .route("/api/v1/billing", get(billing::get_billing))
        .route("/api/v1/billing/checkout", post(billing::create_checkout))
        // Webhooks
        .route("/webhooks/stripe", post(billing::stripe_webhook))
        .route("/webhooks/slack", post(api::v1::webhooks::slack::handle_event))
        .route("/webhooks/github", post(api::v1::webhooks::github::handle_event))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    tracing::info!("savants.cloud listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health() -> &'static str {
    "ok"
}
