//! OAuth 2.0 Device Authorization Grant flow for savants.cloud.
//!
//! Same pattern as `tailscale up`, `gh auth login`, `az login`:
//!
//! 1. CLI requests a device code from savants.cloud
//! 2. User opens a URL in their browser and signs in
//! 3. CLI polls until the browser session completes
//! 4. Device token is saved to ~/.savants/config.toml
//!
//! The user never copies a token. The CLI handles everything.

use colored::*;
use crate::config::{State, CLOUD_ENDPOINT};

/// Run the device authorization flow.
pub async fn run() {
    println!("{}", "Connecting to savants.cloud...".bold());
    println!();

    let state = State::load();

    // Check if already connected
    if state.is_cloud_authenticated() {
        let org = state.cloud_org.as_deref().unwrap_or("unknown");
        println!("  {} Already connected to org: {}", "●".green(), org.cyan());
        println!("  Run {} to disconnect.", "savants disconnect".dimmed());
        return;
    }

    // Step 1: Request device code
    println!("Requesting device authorization...");
    println!();

    // In production, this would be an HTTP POST to:
    //   POST {CLOUD_ENDPOINT}/auth/device/code
    //   → { device_code, user_code, verification_uri, interval, expires_in }
    //
    // For now, show what the flow WILL look like when savants.cloud is built.
    // The actual HTTP calls are stubbed until the cloud backend exists.

    let verification_url = format!("{}/auth/device", CLOUD_ENDPOINT);

    println!("To authenticate, visit:");
    println!();
    println!("    {}", verification_url.cyan().bold().underline());
    println!();
    println!("Waiting for authentication...");
    println!("{}", "(This will complete automatically once you sign in)".dimmed());
    println!();

    // Step 2: Poll for completion
    // In production, this polls:
    //   POST {CLOUD_ENDPOINT}/auth/device/token
    //   { device_code, grant_type: "urn:ietf:params:oauth:grant-type:device_code" }
    //
    // Response states:
    //   - authorization_pending → keep polling
    //   - slow_down → increase interval
    //   - access_denied → user denied
    //   - expired_token → device code expired, restart flow
    //   - success → { access_token, org, device_id }

    // STUB: In the real implementation, this would loop polling the token endpoint.
    // For now, we print what happens and exit gracefully.
    println!("{}", "savants.cloud is not yet live. This flow will work when the cloud tier launches.".yellow());
    println!();
    println!("What will happen:");
    println!("  1. You open the URL above in your browser");
    println!("  2. Sign in with Google, GitHub, or your company's SSO");
    println!("  3. The CLI automatically receives a device token");
    println!("  4. Your local graphs start syncing to savants.cloud");
    println!("  5. Your team can see cross-cluster and cross-repo queries");
    println!();
    println!("Want to be notified when savants.cloud launches?");
    println!("  Visit {}", "https://savants.dev/cloud".cyan());
}

/// Connect Slack for alerts and interactive RCA.
pub fn slack(webhook: Option<String>, bot_token: Option<String>, channel: Option<String>) {
    println!("{}", "Connecting Slack...".bold());
    println!();

    let savants_home = dirs::home_dir().unwrap_or_default().join(".savants");
    let config_path = savants_home.join("slack.toml");

    if webhook.is_none() && bot_token.is_none() {
        // Show current config
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path).unwrap_or_default();
            println!("  {} Slack is configured", "●".green());
            println!("  Config: {}", config_path.display());
            if content.contains("webhook_url") {
                println!("  Mode: Incoming webhook (alerts only)");
            }
            if content.contains("bot_token") {
                println!("  Mode: Bot API (alerts + interactive)");
            }
            println!();
            println!("To reconfigure:");
            println!("  {} {}", "savants connect slack --webhook".dimmed(), "<url>".cyan());
            println!("  {} {}", "savants connect slack --bot-token".dimmed(), "<token> --channel <channel>".cyan());
        } else {
            println!("Slack is not configured.");
            println!();
            println!("{}", "Option 1: Incoming Webhook (alerts only, 2 minutes)".bold());
            println!("  1. Go to https://api.slack.com/apps → Create New App → From Scratch");
            println!("  2. Incoming Webhooks → Activate → Add New Webhook to Workspace");
            println!("  3. Choose your #ops-alerts channel");
            println!("  4. Copy the webhook URL, then run:");
            println!("     {} {}", "savants connect slack --webhook".cyan(), "https://hooks.slack.com/services/T.../B.../xxx".dimmed());
            println!();
            println!("{}", "Option 2: Bot Token (alerts + interactive RCA, 5 minutes)".bold());
            println!("  1. Go to https://api.slack.com/apps → Create New App → From Scratch");
            println!("  2. OAuth & Permissions → Bot Token Scopes → Add:");
            println!("     chat:write, channels:history, channels:read, app_mentions:read");
            println!("  3. Install to Workspace → Copy Bot User OAuth Token");
            println!("  4. Run:");
            println!("     {} {}", "savants connect slack --bot-token".cyan(), "xoxb-... --channel #ops-alerts".dimmed());
        }
        return;
    }

    // Save config
    let mut config_lines = Vec::new();
    if let Some(ref wh) = webhook {
        config_lines.push(format!("webhook_url = \"{}\"", wh));
    }
    if let Some(ref bt) = bot_token {
        config_lines.push(format!("bot_token = \"{}\"", bt));
    }
    if let Some(ref ch) = channel {
        config_lines.push(format!("channel = \"{}\"", ch));
    }
    let config_content = config_lines.join("\n") + "\n";
    if let Err(e) = std::fs::write(&config_path, &config_content) {
        eprintln!("{}: Failed to write config: {}", "Error".red(), e);
        return;
    }

    // Test the connection
    println!("  Testing connection...");
    let test_payload = serde_json::json!({
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": "✅ *Savants connected!*\nAlerts will appear in this channel.\n\nTry: `savants story` to see your infrastructure narrative."
                }
            }
        ]
    });

    let client = reqwest::blocking::Client::new();
    let result = if let Some(ref wh) = webhook {
        client.post(wh).json(&test_payload).timeout(std::time::Duration::from_secs(10)).send()
    } else if let Some(ref bt) = bot_token {
        let ch = channel.as_deref().unwrap_or("#general");
        let mut payload = test_payload;
        payload["channel"] = serde_json::json!(ch);
        client.post("https://slack.com/api/chat.postMessage")
            .header("Authorization", format!("Bearer {}", bt))
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
    } else {
        unreachable!()
    };

    match result {
        Ok(resp) if resp.status().is_success() => {
            println!("  {} Slack connected! Check your channel for a test message.", "✅".green());
            println!();
            println!("To use with the daemon, set these env vars before starting:");
            if let Some(ref wh) = webhook {
                println!("  export SAVANTS_SLACK_WEBHOOK_URL=\"{}\"", wh);
            }
            if let Some(ref bt) = bot_token {
                println!("  export SAVANTS_SLACK_BOT_TOKEN=\"{}\"", bt);
            }
            if let Some(ref ch) = channel {
                println!("  export SAVANTS_SLACK_CHANNEL=\"{}\"", ch);
            }
            println!();
            println!("Then restart the daemon:");
            println!("  {} && {}", "savants daemon stop".dimmed(), "savants daemon start".cyan());
        }
        Ok(resp) => {
            eprintln!("  {} Slack returned HTTP {}", "✗".red(), resp.status());
            eprintln!("  Check your webhook URL or bot token.");
        }
        Err(e) => {
            eprintln!("  {} Failed to reach Slack: {}", "✗".red(), e);
        }
    }
}

/// Disconnect from savants.cloud.
pub fn disconnect() {
    let mut state = State::load();

    if !state.is_cloud_authenticated() {
        println!("Not connected to savants.cloud.");
        return;
    }

    state.cloud_device_token = None;
    state.cloud_device_id = None;
    state.cloud_org = None;
    if let Err(e) = state.save() {
        eprintln!("{}: {}", "Error".red(), e);
        return;
    }

    println!("{}", "Disconnected from savants.cloud.".green());
    println!("Your local graphs are unaffected. Federation sync has stopped.");
}
