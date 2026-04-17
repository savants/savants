use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

#[derive(Deserialize)]
pub struct SlackEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub challenge: Option<String>,
    pub event: Option<SlackEventPayload>,
}

#[derive(Deserialize)]
pub struct SlackEventPayload {
    #[serde(rename = "type")]
    pub event_type: String,
    pub text: Option<String>,
    pub user: Option<String>,
    pub channel: Option<String>,
    pub ts: Option<String>,
}

#[derive(Serialize)]
pub struct SlackResponse {
    pub challenge: Option<String>,
}

pub async fn handle_event(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SlackEvent>,
) -> Result<Json<SlackResponse>, StatusCode> {
    // URL verification challenge
    if body.event_type == "url_verification" {
        return Ok(Json(SlackResponse { challenge: body.challenge }));
    }

    // Handle events
    if let Some(event) = body.event {
        match event.event_type.as_str() {
            "app_mention" => {
                // @savants was mentioned - extract the command and run it
                if let Some(text) = &event.text {
                    tracing::info!(
                        channel = event.channel.as_deref().unwrap_or("?"),
                        user = event.user.as_deref().unwrap_or("?"),
                        text = %text,
                        "slack @savants mention"
                    );
                    // TODO: parse command from text, call the appropriate tool,
                    // post the result back to the channel via Slack API
                }
            }
            "message" => {
                // Message in a channel - ingest into the graph if configured
                tracing::debug!("slack message event");
            }
            _ => {
                tracing::debug!(event_type = %event.event_type, "unhandled slack event");
            }
        }
    }

    Ok(Json(SlackResponse { challenge: None }))
}
