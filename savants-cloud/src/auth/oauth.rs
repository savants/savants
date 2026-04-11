use axum::http::StatusCode;

pub async fn google_callback() -> (StatusCode, &'static str) {
    (StatusCode::NOT_IMPLEMENTED, "Google OAuth not implemented yet")
}

pub async fn github_callback() -> (StatusCode, &'static str) {
    (StatusCode::NOT_IMPLEMENTED, "GitHub OAuth not implemented yet")
}
